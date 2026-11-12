//! Seat state wiring (08 §3, §4.7, §14): the server's clock, the chart
//! provider that builds each version's [`ChartView`] from its stored full
//! artefact, and [`Seats`], the actor registry over [`PgStore`] that the HTTP
//! handlers send commands through.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use cavea_engine::{
    ActorConfig, ChartProvider, ChartVersionId, ChartView, Clock, EventId, Idx, Registry,
    RegistryConfig, StoreError, Timestamp,
};
use cavea_format::full_label::{full_label, seat_label};
use cavea_format::{Artefact, NONE, ObjectKind};
use sqlx::PgPool;
use tracing::Instrument as _;

use crate::config::Config;
use crate::store::PgStore;

/// The engine's clock: the server's wall clock in Unix milliseconds. Tests may
/// use a [`cavea_engine::ManualClock`] instead, to drive hold expiry.
#[derive(Debug, Clone)]
pub enum ServerClock {
    System,
    #[cfg(test)]
    Manual(cavea_engine::ManualClock),
    /// The system clock, but reading it panics while the flag is set, on a
    /// task outside any request: inside an event actor.
    #[cfg(test)]
    Faulty(Arc<std::sync::atomic::AtomicBool>),
}

fn wall_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// Re-read after each sleep: the wall clock may have moved.
async fn wall_sleep_until(t: Timestamp) {
    loop {
        let now = wall_ms();
        if now >= t.0 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(t.0 - now)).await;
    }
}

impl Clock for ServerClock {
    fn now(&self) -> Timestamp {
        match self {
            Self::System => Timestamp(wall_ms()),
            #[cfg(test)]
            Self::Manual(clock) => clock.now(),
            #[cfg(test)]
            Self::Faulty(fail) => {
                let armed = fail.load(std::sync::atomic::Ordering::Relaxed);
                assert!(
                    !armed || cavea_telemetry::request_id().is_some(),
                    "injected fault in the actor"
                );
                Timestamp(wall_ms())
            }
        }
    }

    fn sleep_until(&self, t: Timestamp) -> impl Future<Output = ()> + Send {
        let clock = self.clone();
        async move {
            match clock {
                Self::System => wall_sleep_until(t).await,
                #[cfg(test)]
                Self::Manual(clock) => clock.sleep_until(t).await,
                #[cfg(test)]
                Self::Faulty(_) => wall_sleep_until(t).await,
            }
        }
    }
}

/// The objects of the cached chart versions, summed, above which the least
/// recently used versions are dropped from the cache. A view costs about 100
/// bytes per object, so this is about 400 MB. Actors keep their own reference,
/// so dropping a version from the cache never takes it from a live event.
pub const CHART_CACHE_OBJECTS: usize = 4_000_000;

/// A chart version as the server uses it: the engine's view, each `idx`'s
/// full label for responses, and the decoded full artefact without its
/// geometry, for what a backend needs of an object ([`Chart::details`]).
#[derive(Debug)]
pub struct Chart {
    pub view: Arc<ChartView>,
    labels: Box<[Box<str>]>,
    parts: cavea_format::Chart,
}

/// What a backend needs to price and print an object without another call
/// (08 §6.2): its kind, its category as the chart defines it, and the parts
/// its full label was composed from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Details<'c> {
    pub kind: &'static str,
    /// The category's key and label.
    pub category: (&'c str, &'c str),
    /// `None` outside any section, or in a section without a label.
    pub section: Option<&'c str>,
    /// A seat's row label; `None` for any other object.
    pub row: Option<&'c str>,
    /// A seat's own label; `None` for any other object.
    pub seat: Option<cavea_core::label::LabelBuf>,
}

impl Chart {
    #[must_use]
    pub fn label(&self, idx: Idx) -> Option<&str> {
        self.labels.get(idx as usize).map(|l| &**l)
    }

    /// Object `idx`'s details, read from memory: a few binary searches over
    /// the rows and segments for a seat, two array reads for its category.
    /// `None` for an `idx` the chart does not have.
    #[must_use]
    pub fn details(&self, idx: Idx) -> Option<Details<'_>> {
        let c = &self.parts;
        let text = |i: u32| (i != NONE).then(|| c.strings.get(i)).flatten();
        let category = |i: usize| {
            let category = c.categories.get(i)?;
            Some((
                text(category.key)?,
                text(category.label).unwrap_or_default(),
            ))
        };
        let section = |i: u32| {
            c.sections
                .get(i as usize)
                .and_then(|s| text(s.label))
                .filter(|l| !l.is_empty())
        };
        if idx < c.meta.seat_count {
            let parts = seat_label(c, idx).ok()?;
            let category = category(usize::from(*c.seat_category.get(idx as usize)?))?;
            return Some(Details {
                kind: "seat",
                category,
                section: Some(parts.section).filter(|s| !s.is_empty()),
                row: Some(parts.row),
                seat: Some(parts.seat),
            });
        }
        let object = c.objects.get((idx - c.meta.seat_count) as usize)?;
        Some(Details {
            kind: match object.kind {
                ObjectKind::Table => "table",
                ObjectKind::Booth => "booth",
                ObjectKind::Area => "area",
            },
            category: category(object.category as usize)?,
            section: section(object.section),
            row: None,
            seat: None,
        })
    }
}

/// [`ChartProvider`] on PostgreSQL: reads `chart_versions.full_ccf`, decodes it
/// on a blocking thread and caches the result per version, bounded by
/// [`CHART_CACHE_OBJECTS`]. Two loads of one uncached version at the same
/// moment both decode; the second result replaces the first.
pub struct PgCharts {
    pool: PgPool,
    cache: Mutex<Cache>,
    limit: usize,
}

#[derive(Default)]
struct Cache {
    map: HashMap<ChartVersionId, (Arc<Chart>, u64)>,
    objects: usize,
    tick: u64,
}

impl fmt::Debug for PgCharts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PgCharts")
    }
}

impl PgCharts {
    #[must_use]
    pub fn new(pool: PgPool, limit: usize) -> Self {
        Self {
            pool,
            cache: Mutex::default(),
            limit,
        }
    }

    /// The version's chart, from the cache or the database.
    pub async fn get(&self, version: ChartVersionId) -> Result<Arc<Chart>, StoreError> {
        {
            let mut cache = self.cache.lock().unwrap_or_else(PoisonError::into_inner);
            cache.tick += 1;
            let tick = cache.tick;
            if let Some((chart, used)) = cache.map.get_mut(&version) {
                *used = tick;
                return Ok(chart.clone());
            }
        }
        let row: Option<(Vec<u8>, Vec<u8>)> =
            sqlx::query_as("SELECT full_ccf, hash FROM chart_versions WHERE id = $1::uuid")
                .bind(format!("{:032x}", version.0))
                .fetch_optional(&self.pool)
                .instrument(cavea_telemetry::db_span("read_chart_version"))
                .await
                .map_err(|e| StoreError::Unavailable(e.to_string()))?;
        let (bytes, hash) =
            row.ok_or_else(|| StoreError::Inconsistent("unknown chart version".into()))?;
        let hash = <[u8; 16]>::try_from(hash)
            .map_err(|_| StoreError::Inconsistent("chart version hash is not 16 bytes".into()))?;
        let chart = tokio::task::spawn_blocking(move || build(&bytes, hash))
            .await
            .map_err(|e| StoreError::Inconsistent(e.to_string()))?
            .map_err(|why| {
                tracing::error!(chart_version = %format!("cv_{:032x}", version.0), %why,
                    "stored chart version is unreadable");
                StoreError::Inconsistent(format!("stored chart version is unreadable: {why}"))
            })?;
        let chart = Arc::new(chart);
        let mut cache = self.cache.lock().unwrap_or_else(PoisonError::into_inner);
        cache.tick += 1;
        let tick = cache.tick;
        if let Some((old, _)) = cache.map.insert(version, (chart.clone(), tick)) {
            cache.objects -= old.labels.len();
        }
        cache.objects += chart.labels.len();
        // Least recently used first; the version just loaded stays.
        while cache.objects > self.limit {
            let Some(oldest) = cache
                .map
                .iter()
                .filter(|(v, _)| **v != version)
                .min_by_key(|(_, (_, used))| *used)
                .map(|(v, _)| *v)
            else {
                break;
            };
            if let Some((old, _)) = cache.map.remove(&oldest) {
                cache.objects -= old.labels.len();
            }
        }
        Ok(chart)
    }

    /// Versions in the cache.
    #[must_use]
    pub fn cached(&self) -> usize {
        let cache = self.cache.lock().unwrap_or_else(PoisonError::into_inner);
        cache.map.len()
    }
}

impl ChartProvider for PgCharts {
    async fn chart(&self, version: ChartVersionId) -> Result<Arc<ChartView>, StoreError> {
        Ok(self.get(version).await?.view.clone())
    }
}

/// Decode a full artefact and take every object's full label: seats through
/// `cavea_format::full_label`, composed as the compiler composed them (03
/// §3.1 step 6), other objects as stored. Every input is treated as hostile:
/// a label that cannot be composed, or a missing or duplicate label or `oid`,
/// is an error, never a panic. `hash` is the version's public content hash,
/// which HELLO carries (08 §8.1).
fn build(bytes: &[u8], hash: [u8; 16]) -> Result<Chart, String> {
    let c = cavea_format::decode(bytes, Artefact::Full).map_err(|e| e.to_string())?;
    let full = c.full.as_ref().ok_or("no full-artefact data")?;
    let total = (c.meta.seat_count as usize)
        .checked_add(c.objects.len())
        .filter(|&n| n == full.oids.len())
        .ok_or("object count does not match the oids")?;
    let mut labels: Vec<Box<str>> = Vec::with_capacity(total);
    for idx in 0..c.meta.seat_count {
        let label = full_label(&c, idx).map_err(|e| format!("seat {idx}: {e}"))?;
        labels.push(label.as_str().into());
    }
    for o in &c.objects {
        let label = c.strings.get(o.label).filter(|_| o.label != NONE);
        labels.push(label.ok_or("object without a label")?.into());
    }
    let view = ChartView::new(full.oids.iter().copied().zip(labels.iter().cloned()))
        .map_err(|e| e.to_string())?
        .with_hash(hash);
    // Kept for `Chart::details`: categories, sections, rows, segments, label
    // rules and exceptions, strings, per-seat categories and the objects,
    // every reference in range (`decode` checked them). What only drawing or
    // the view needs is dropped.
    let mut parts = c;
    parts.seat_pos = Vec::new();
    parts.seat_flags = Vec::new();
    parts.chunks = Vec::new();
    parts.mesh = cavea_format::Mesh::default();
    parts.texts = Vec::new();
    parts.icons = Vec::new();
    parts.images = Vec::new();
    parts.full = None;
    Ok(Chart {
        view: Arc::new(view),
        labels: labels.into(),
        parts,
    })
}

/// The registry type the server runs.
pub type Engine = Registry<PgStore, PgCharts, ServerClock>;

/// Every live event actor of this node, and the charts they use. Cheap to
/// clone. Built once at start-up and held in `AppState`.
#[derive(Clone)]
pub struct Seats {
    pub registry: Engine,
    pub charts: Arc<PgCharts>,
    pub clock: ServerClock,
    pool: PgPool,
}

impl fmt::Debug for Seats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Seats")
    }
}

impl Seats {
    /// With `broadcast_on`, the realtime runtime, each actor's publications
    /// are broadcast by a task there (08 §8.3).
    #[must_use]
    pub fn new(
        pool: PgPool,
        config: &Config,
        clock: ServerClock,
        broadcast_on: Option<tokio::runtime::Handle>,
    ) -> Self {
        let lease = Duration::from_secs(config.lease_secs.get().into());
        let store = PgStore::new(pool.clone(), &config.node_id, lease);
        let charts = Arc::new(PgCharts::new(pool.clone(), CHART_CACHE_OBJECTS));
        let registry_config = RegistryConfig {
            actor: ActorConfig {
                heartbeat_ms: u64::from(config.heartbeat_secs.get()) * 1000,
                broadcast_on,
                ..ActorConfig::default()
            },
            ..RegistryConfig::default()
        };
        let registry = Registry::new(
            Arc::new(store),
            charts.clone(),
            clock.clone(),
            registry_config,
        );
        Self {
            registry,
            charts,
            clock,
            pool,
        }
    }

    /// Load every event that has outstanding holds, earliest expiry first, one
    /// at a time, so that their expiries fire after a restart (08 §4.7). The
    /// server runs it in the background after it starts listening: a request
    /// for an event not loaded yet loads it on demand, sharing the load.
    pub async fn preload(&self) -> Result<(), sqlx::Error> {
        let ids: Vec<String> = sqlx::query_scalar(
            "SELECT replace(event_id::text, '-', '') FROM holds
              GROUP BY event_id ORDER BY min(expires_at)",
        )
        .fetch_all(&self.pool)
        .instrument(cavea_telemetry::db_span("events_with_holds"))
        .await?;
        let events: Vec<EventId> = ids.iter().filter_map(|id| event_id(id)).collect();
        let started = std::time::Instant::now();
        let failed = self.registry.preload(events.iter().copied()).await;
        if let Some((event, error)) = failed.first() {
            tracing::warn!(failed = failed.len(), first_event_id = %format!("{:032x}", event.0),
                %error, "some events with holds did not load; they load on their next request");
        }
        tracing::info!(
            events = events.len(),
            loaded = events.len() - failed.len(),
            ms = started.elapsed().as_secs_f64() * 1e3,
            "events with holds loaded"
        );
        Ok(())
    }

    /// Stop every actor: each commits what it accepted and releases its lease
    /// (08 §14). Bounded by store timeouts; the caller adds its own deadline.
    pub async fn shutdown(&self) {
        self.registry.shutdown().await;
    }
}

/// An event id from 32 hex digits.
#[must_use]
pub fn event_id(hex: &str) -> Option<EventId> {
    u128::from_str_radix(hex, 16).ok().map(EventId)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "tests")]

    use cavea_compiler::document::{ChartDocument, Node};
    use cavea_compiler::write_full_label as compose;
    use cavea_format::{Bounds, Object, Point};

    use super::*;

    fn chart(document: &ChartDocument) -> Chart {
        let compiled = cavea_compiler::compile(document).unwrap();
        build(&compiled.full, compiled.public_hash).unwrap()
    }

    /// The stadium, its categories labelled apart from their keys: each
    /// section's category is the document's truth for its seats.
    fn stadium() -> ChartDocument {
        let mut document = cavea_testkit::synthetic::stadium(600, 7);
        for category in &mut document.categories {
            category.label = format!("{} seats", category.key);
        }
        document
    }

    /// Every seat: its parts compose its full label by the chart's template,
    /// and its category is the one its section has in the document.
    fn check_seats(chart: &Chart, document: &ChartDocument, sections: bool) {
        let template = chart
            .parts
            .strings
            .get(chart.parts.meta.label_template)
            .unwrap();
        let by_section: HashMap<String, String> = document
            .nodes
            .iter()
            .filter_map(|node| match node {
                Node::Section(s) => Some((s.label.clone()?, s.category.clone()?)),
                _ => None,
            })
            .collect();
        for idx in 0..u32::try_from(chart.view.len()).unwrap() {
            let d = chart.details(idx).unwrap();
            assert_eq!(d.kind, "seat");
            let seat = d.seat.as_ref().unwrap();
            let mut composed = String::new();
            let section = d.section.unwrap_or_default();
            compose(
                template,
                section,
                d.row.unwrap(),
                seat.as_str(),
                &mut composed,
            )
            .unwrap();
            assert_eq!(Some(composed.as_str()), chart.label(idx));
            let key = if sections {
                by_section[d.section.unwrap()].clone()
            } else {
                assert_eq!(d.section, None);
                "standard".to_owned()
            };
            assert_eq!(d.category.0, key);
            assert_eq!(d.category.1, format!("{key} seats"));
        }
    }

    #[test]
    fn details_follow_the_chart() {
        let document = stadium();
        check_seats(&chart(&document), &document, true);

        // No sections: one block at the top level, `standard`; labels leave
        // the empty section out with its dash (`A-1`, D-96), and `section`
        // is null.
        let mut flat = stadium();
        flat.nodes = flat
            .nodes
            .into_iter()
            .take(1)
            .flat_map(|node| match node {
                Node::Section(s) => s.children,
                other => vec![other],
            })
            .map(|node| match node {
                Node::RowBlock(mut block) => {
                    block.category = Some("standard".to_owned());
                    Node::RowBlock(block)
                }
                other => other,
            })
            .collect();
        let flat_chart = chart(&flat);
        let d = flat_chart.details(0).unwrap();
        let (row, seat) = (d.row.unwrap(), d.seat.unwrap());
        assert_eq!(flat_chart.label(0), Some(format!("{row}-{seat}").as_str()));
        check_seats(&flat_chart, &flat, false);
    }

    /// Tables booked whole, booths and areas: not compiled yet, so added to a
    /// compiled chart by hand, as a later compiler will write them.
    #[test]
    fn other_kinds_have_a_category_and_section_but_no_row_or_seat() {
        let compiled = cavea_compiler::compile(&stadium()).unwrap();
        let mut c = cavea_format::decode(&compiled.full, Artefact::Full).unwrap();
        let first_oid = c.full.as_ref().unwrap().oids.iter().max().unwrap() + 1;
        let kinds = [
            (ObjectKind::Table, "Table 1", 0, 3),
            (ObjectKind::Booth, "Box A", NONE, 1),
            (ObjectKind::Area, "Standing", 1, 2),
        ];
        for (oid, (kind, label, section, category)) in (first_oid..).zip(kinds) {
            let label = c.strings.push(label).unwrap();
            c.objects.push(Object {
                kind,
                label,
                category,
                capacity: if kind == ObjectKind::Area { 500 } else { 1 },
                floor: 0,
                section,
                mesh_first: 0,
                mesh_count: 0,
                bounds: Bounds::default(),
                label_anchor: Point::default(),
            });
            let full = c.full.as_mut().unwrap();
            full.oids.push(oid);
            full.ranks.push(0);
        }
        let bytes = cavea_format::encode(&c, Artefact::Full).unwrap();
        let chart = build(&bytes, compiled.public_hash).unwrap();
        let first = c.meta.seat_count;
        let section = |i: usize| c.strings.get(c.sections[i].label).unwrap();
        let category = |i: usize| {
            let cat = &c.categories[i];
            (
                c.strings.get(cat.key).unwrap(),
                c.strings.get(cat.label).unwrap(),
            )
        };
        let expected = [
            ("table", "Table 1", Some(section(0)), category(3)),
            ("booth", "Box A", None, category(1)),
            ("area", "Standing", Some(section(1)), category(2)),
        ];
        for (i, (kind, label, section, category)) in expected.into_iter().enumerate() {
            let idx = first + u32::try_from(i).unwrap();
            assert_eq!(chart.label(idx), Some(label));
            let d = chart.details(idx).unwrap();
            assert_eq!(
                (d.kind, d.section, d.category, d.row, d.seat),
                (kind, section, category, None, None)
            );
        }
        assert_eq!(chart.details(first + 3), None);
    }
}
