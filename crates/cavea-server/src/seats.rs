//! Seat state wiring (08 §3, §4.7, §14): the server's clock, the chart
//! provider that builds each version's [`ChartView`] from its stored full
//! artefact, and [`Seats`], the actor registry over [`PgStore`] that the HTTP
//! handlers send commands through.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use cavea_core::label::LabelRule;
use cavea_engine::{
    ActorConfig, ChartProvider, ChartVersionId, ChartView, Clock, EventId, Idx, Registry,
    RegistryConfig, StoreError, Timestamp,
};
use cavea_format::{Artefact, NONE};
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
}

fn wall_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

impl Clock for ServerClock {
    fn now(&self) -> Timestamp {
        match self {
            Self::System => Timestamp(wall_ms()),
            #[cfg(test)]
            Self::Manual(clock) => clock.now(),
        }
    }

    fn sleep_until(&self, t: Timestamp) -> impl Future<Output = ()> + Send {
        let clock = self.clone();
        async move {
            match clock {
                // Re-read after each sleep: the wall clock may have moved.
                Self::System => loop {
                    let now = wall_ms();
                    if now >= t.0 {
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(t.0 - now)).await;
                },
                #[cfg(test)]
                Self::Manual(clock) => clock.sleep_until(t).await,
            }
        }
    }
}

/// The objects of the cached chart versions, summed, above which the least
/// recently used versions are dropped from the cache. A view costs about 100
/// bytes per object, so this is about 400 MB. Actors keep their own reference,
/// so dropping a version from the cache never takes it from a live event.
pub const CHART_CACHE_OBJECTS: usize = 4_000_000;

/// A chart version as the server uses it: the engine's view, and each `idx`'s
/// full label for responses.
#[derive(Debug)]
pub struct Chart {
    pub view: Arc<ChartView>,
    labels: Box<[Box<str>]>,
}

impl Chart {
    #[must_use]
    pub fn label(&self, idx: Idx) -> Option<&str> {
        self.labels.get(idx as usize).map(|l| &**l)
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
        let bytes: Option<Vec<u8>> =
            sqlx::query_scalar("SELECT full_ccf FROM chart_versions WHERE id = $1::uuid")
                .bind(format!("{:032x}", version.0))
                .fetch_optional(&self.pool)
                .instrument(cavea_telemetry::db_span("read_chart_version"))
                .await
                .map_err(|e| StoreError::Unavailable(e.to_string()))?;
        let bytes =
            bytes.ok_or_else(|| StoreError::Inconsistent("unknown chart version".into()))?;
        let chart = tokio::task::spawn_blocking(move || build(&bytes))
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

/// Decode a full artefact and compose every object's full label from the
/// chart-level template, the section and row labels and the seat's rule or
/// exception, as the compiler composed them (03 §3.1 step 6). Every input is
/// treated as hostile: a reference out of range, a label that cannot be
/// written, a missing or duplicate label or `oid` is an error, never a panic.
fn build(bytes: &[u8]) -> Result<Chart, String> {
    let c = cavea_format::decode(bytes, Artefact::Full).map_err(|e| e.to_string())?;
    let full = c.full.as_ref().ok_or("no full-artefact data")?;
    let s = |i: u32| -> Result<&str, String> {
        if i == NONE {
            Ok("")
        } else {
            c.strings
                .get(i)
                .ok_or_else(|| format!("string {i} out of range"))
        }
    };
    let seats = c.meta.seat_count as usize;
    let total = seats
        .checked_add(c.objects.len())
        .filter(|&n| n == full.oids.len())
        .ok_or("object count does not match the oids")?;
    let mut customs = Vec::with_capacity(c.label_rules.len());
    for l in &c.label_rules {
        let list = (0..l.custom_count)
            .map(|k| {
                s(l.custom_first
                    .checked_add(k)
                    .ok_or("custom label overflow")?)
                .map(str::to_owned)
            })
            .collect::<Result<Vec<_>, String>>()?;
        customs.push(list);
    }
    let exceptions: HashMap<u32, u32> = c
        .label_exceptions
        .iter()
        .map(|x| (x.idx, x.label))
        .collect();
    let template = s(c.meta.label_template)?;
    let mut labels: Vec<Option<Box<str>>> = vec![None; total];
    let mut seat = String::new();
    for g in &c.segments {
        let row = c
            .rows
            .get(g.row as usize)
            .ok_or("segment row out of range")?;
        let section = match row.section {
            NONE => "",
            i => s(c
                .sections
                .get(i as usize)
                .ok_or("row section out of range")?
                .label)?,
        };
        for k in 0..u32::from(g.seat_count) {
            let idx = g.seat_first.checked_add(k).ok_or("seat index overflow")?;
            seat.clear();
            if let Some(&x) = exceptions.get(&idx) {
                seat.push_str(s(x)?);
            } else {
                let l = c
                    .label_rules
                    .get(row.label_rule as usize)
                    .ok_or("seat without a label")?;
                let rule = LabelRule {
                    scheme: l.scheme,
                    start: l.start,
                    step: l.step,
                    overflow: l.overflow,
                    prefix: s(l.prefix)?,
                    suffix: s(l.suffix)?,
                    custom: customs
                        .get(row.label_rule as usize)
                        .map_or(&[], Vec::as_slice),
                };
                let position = u32::from(g.position) + k;
                let within = cavea_core::label::seat_ordinal(
                    row.direction.into(),
                    0,
                    row.positions.into(),
                    position,
                );
                let ordinal = row
                    .ordinal_base
                    .checked_add(within)
                    .ok_or("ordinal overflow")?;
                rule.write(ordinal, &mut seat).map_err(|e| e.to_string())?;
            }
            let mut label = String::new();
            cavea_compiler::write_full_label(template, section, s(row.label)?, &seat, &mut label)
                .map_err(|e| e.to_string())?;
            let slot = labels
                .get_mut(idx as usize)
                .ok_or("seat index out of range")?;
            if slot.replace(label.into()).is_some() {
                return Err(format!("seat {idx} labelled twice"));
            }
        }
    }
    for (o, slot) in c.objects.iter().zip(labels.iter_mut().skip(seats)) {
        *slot = Some(s(o.label)?.into());
    }
    let labels = labels
        .into_iter()
        .enumerate()
        .map(|(idx, l)| l.ok_or_else(|| format!("object {idx} has no label")))
        .collect::<Result<Box<[_]>, _>>()?;
    let view = ChartView::new(full.oids.iter().copied().zip(labels.iter().cloned()))
        .map_err(|e| e.to_string())?;
    Ok(Chart {
        view: Arc::new(view),
        labels,
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
    pool: PgPool,
}

impl fmt::Debug for Seats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Seats")
    }
}

impl Seats {
    #[must_use]
    pub fn new(pool: PgPool, config: &Config, clock: ServerClock) -> Self {
        let lease = Duration::from_secs(config.lease_secs.get().into());
        let store = PgStore::new(pool.clone(), &config.node_id, lease);
        let charts = Arc::new(PgCharts::new(pool.clone(), CHART_CACHE_OBJECTS));
        let registry_config = RegistryConfig {
            actor: ActorConfig {
                heartbeat_ms: u64::from(config.heartbeat_secs.get()) * 1000,
                ..ActorConfig::default()
            },
            ..RegistryConfig::default()
        };
        let registry = Registry::new(Arc::new(store), charts.clone(), clock, registry_config);
        Self {
            registry,
            charts,
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
