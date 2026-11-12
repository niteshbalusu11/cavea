//! The server's licence (12 §7.3, D-80, D-81): where it comes from, which
//! one counts, and its state, computed from the clock on every read.
//!
//! A licence comes from `CAVEA_LICENCE` (a file path, or the file's contents)
//! or from an upload through the console, stored in the `licence` table so
//! that every node reads the same one. When both are there, the better one
//! counts: valid over grace over expired over invalid over missing, then the
//! later expiry, then the upload. Each node reads both at start and again
//! every [`REFRESH`], so an upload reaches every node within that bound (the
//! node that took it at once), and a file replaced on disk without a restart.
//!
//! **Nothing on the seat path reads this module** (D-20): holding, booking,
//! releasing, hold tokens and the public state and stream never consult the
//! licence. The public event endpoint reads [`Licences::public`] and the
//! console's routes [`Licences::status`], both from memory, infallibly, in
//! constant time; the only refusal is the console's writes to charts and
//! events while expired ([`Licences::read_only`], `http::staff`).
//!
//! The log carries the state, the licence's id, its source and an invalid
//! file's reason, at start and whenever one of them changes; never the
//! file's contents or its signature.

use std::path::PathBuf;
use std::sync::{Arc, PoisonError, RwLock};
use std::time::Duration;

use cavea_engine::Clock as _;
use cavea_licence::{Licence, MAX_BYTES, Standing, Verifier};
use serde_json::{Value, json};
use sqlx::PgPool;
use tracing::Instrument as _;

use crate::seats::ServerClock;

/// How often each node reads the licence again.
pub const REFRESH: Duration = Duration::from_secs(30);

/// `CAVEA_LICENCE` (08 §13): the file's contents when it starts with `{`,
/// else a path to the file.
#[derive(Clone, PartialEq, Eq)]
pub enum LicenceSource {
    Path(PathBuf),
    Contents(Vec<u8>),
}

impl std::str::FromStr for LicenceSource {
    type Err = &'static str;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let trimmed = s.trim();
        if trimmed.is_empty() {
            return Err("must be a licence file's path or its contents");
        }
        Ok(if trimmed.starts_with('{') {
            Self::Contents(s.as_bytes().to_vec())
        } else {
            Self::Path(PathBuf::from(trimmed))
        })
    }
}

impl std::fmt::Debug for LicenceSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Path(path) => write!(f, "Path({})", path.display()),
            Self::Contents(_) => f.write_str("Contents(..)"),
        }
    }
}

/// The licence's state (12 §7.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Valid,
    Grace,
    Expired,
    Missing,
    Invalid,
}

impl State {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Valid => "valid",
            Self::Grace => "grace",
            Self::Expired => "expired",
            Self::Missing => "missing",
            Self::Invalid => "invalid",
        }
    }

    fn rank(self) -> u8 {
        match self {
            Self::Valid => 4,
            Self::Grace => 3,
            Self::Expired => 2,
            Self::Invalid => 1,
            Self::Missing => 0,
        }
    }
}

/// Where the licence that counts came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// `CAVEA_LICENCE`.
    Setting,
    /// Uploaded through the console.
    Upload,
}

impl Source {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Setting => "setting",
            Self::Upload => "upload",
        }
    }
}

/// What one source holds.
#[derive(Debug, Clone)]
pub enum Found {
    Missing,
    /// The file is there but is no licence; why (`cavea_licence::Invalid`'s
    /// reasons, or `unreadable`).
    Invalid(&'static str),
    Licence(Arc<Licence>),
}

impl Found {
    /// Verify `bytes`.
    #[must_use]
    pub fn of(verifier: &Verifier, bytes: &[u8]) -> Self {
        match verifier.verify(bytes) {
            Ok(licence) => Self::Licence(Arc::new(licence)),
            Err(invalid) => Self::Invalid(invalid.as_str()),
        }
    }

    fn state(&self, now_ms: u64) -> State {
        match self {
            Self::Missing => State::Missing,
            Self::Invalid(_) => State::Invalid,
            Self::Licence(licence) => match licence.standing(now_ms) {
                Standing::Valid => State::Valid,
                Standing::Grace => State::Grace,
                Standing::Expired => State::Expired,
            },
        }
    }
}

/// The licence that counts, at an instant.
#[derive(Debug, Clone)]
pub struct Status {
    pub state: State,
    pub source: Option<Source>,
    /// When `state` is `invalid`: why.
    pub reason: Option<&'static str>,
    /// When `state` is `valid`, `grace` or `expired`.
    pub licence: Option<Arc<Licence>>,
}

impl Status {
    /// Whether the console is read-only but for seat operations (12 §7.3).
    #[must_use]
    pub fn read_only(&self) -> bool {
        self.state == State::Expired
    }
}

#[derive(Debug)]
struct Current {
    setting: Found,
    upload: Found,
    /// What was last logged: the state and the licence id.
    logged: Option<(State, Option<String>)>,
}

/// The licence of this node. Cheap to clone.
#[derive(Debug, Clone)]
pub struct Licences(Arc<Inner>);

#[derive(Debug)]
struct Inner {
    verifier: Verifier,
    source: Option<LicenceSource>,
    current: RwLock<Current>,
}

impl Licences {
    /// Nothing read yet: `missing` until [`Self::refresh`], except for a
    /// licence given as contents, verified here.
    #[must_use]
    pub fn new(verifier: Verifier, source: Option<LicenceSource>) -> Self {
        let setting = match &source {
            Some(LicenceSource::Contents(bytes)) => Found::of(&verifier, bytes),
            _ => Found::Missing,
        };
        Self(Arc::new(Inner {
            verifier,
            source,
            current: RwLock::new(Current {
                setting,
                upload: Found::Missing,
                logged: None,
            }),
        }))
    }

    pub fn verifier(&self) -> &Verifier {
        &self.0.verifier
    }

    /// The licence that counts at `now_ms`.
    #[must_use]
    pub fn status(&self, now_ms: u64) -> Status {
        let current = self
            .0
            .current
            .read()
            .unwrap_or_else(PoisonError::into_inner);
        best(&current.setting, &current.upload, now_ms)
    }

    /// Whether the console is read-only at `now_ms` (`expired`).
    #[must_use]
    pub fn read_only(&self, now_ms: u64) -> bool {
        self.status(now_ms).read_only()
    }

    /// What the public event endpoint carries for the renderer (12 §7.4):
    /// `{"status": "licensed" | "expired" | "unlicensed", "domains": [..]}`,
    /// the domains only while licensed. Nothing else of the licence: not its
    /// customer, plan, id, dates, or whether it is in grace.
    #[must_use]
    pub fn public(&self, now_ms: u64) -> Value {
        let status = self.status(now_ms);
        let (word, domains) = match (status.state, &status.licence) {
            (State::Valid | State::Grace, Some(licence)) => ("licensed", licence.domains.clone()),
            (State::Expired, _) => ("expired", Vec::new()),
            _ => ("unlicensed", Vec::new()),
        };
        json!({ "status": word, "domains": domains })
    }

    /// Take a licence just uploaded (and committed), before the next refresh.
    pub fn set_upload(&self, found: Found, now_ms: u64) {
        let mut current = self
            .0
            .current
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        current.upload = found;
        log_change(&mut current, now_ms);
    }

    /// Read `CAVEA_LICENCE`'s file and the uploaded licence again, then log
    /// the state if it changed. A database that cannot be read keeps the
    /// upload last read.
    pub async fn refresh(&self, db: &PgPool, now_ms: u64) {
        let setting = match &self.0.source {
            Some(LicenceSource::Path(path)) => Some(self.read_file(path.clone()).await),
            _ => None,
        };
        let upload = match read_upload(db).await {
            Ok(Some(bytes)) => Some(Found::of(&self.0.verifier, &bytes)),
            Ok(None) => Some(Found::Missing),
            Err(error) => {
                tracing::warn!(%error, "cannot read the uploaded licence; keeping the last one read");
                None
            }
        };
        let mut current = self
            .0
            .current
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(setting) = setting {
            current.setting = setting;
        }
        if let Some(upload) = upload {
            current.upload = upload;
        }
        log_change(&mut current, now_ms);
    }

    async fn read_file(&self, path: PathBuf) -> Found {
        let read = tokio::task::spawn_blocking(move || {
            use std::io::Read as _;
            let file = std::fs::File::open(&path)?;
            let mut bytes = Vec::new();
            file.take(u64::try_from(MAX_BYTES).unwrap_or(u64::MAX) + 1)
                .read_to_end(&mut bytes)?;
            Ok::<_, std::io::Error>(bytes)
        })
        .await;
        match read {
            Ok(Ok(bytes)) => Found::of(&self.0.verifier, &bytes),
            Ok(Err(error)) => {
                tracing::warn!(error = %error.kind(), "cannot read the licence file (CAVEA_LICENCE)");
                Found::Invalid("unreadable")
            }
            Err(_) => Found::Invalid("unreadable"),
        }
    }

    /// Refresh every [`REFRESH`], for ever; `serve` aborts it at shutdown.
    pub async fn watch(self, db: PgPool, clock: ServerClock) {
        loop {
            tokio::time::sleep(REFRESH).await;
            self.refresh(&db, clock.now().0).await;
        }
    }
}

const READ_UPLOAD: &str = "SELECT document FROM licence WHERE id = 1";

async fn read_upload(db: &PgPool) -> Result<Option<Vec<u8>>, sqlx::Error> {
    sqlx::query_scalar(READ_UPLOAD)
        .fetch_optional(db)
        .instrument(cavea_telemetry::db_span("read_licence"))
        .await
}

/// The better of the two (module doc).
fn best(setting: &Found, upload: &Found, now_ms: u64) -> Status {
    let key = |found: &Found, upload: bool| {
        let expires = match found {
            Found::Licence(l) => Some(l.expires_at),
            _ => None,
        };
        (found.state(now_ms).rank(), expires, upload)
    };
    let (found, source) = if key(upload, true) > key(setting, false) {
        (upload, Source::Upload)
    } else {
        (setting, Source::Setting)
    };
    let state = found.state(now_ms);
    Status {
        state,
        source: (state != State::Missing).then_some(source),
        reason: match found {
            Found::Invalid(reason) => Some(reason),
            _ => None,
        },
        licence: match found {
            Found::Licence(licence) => Some(licence.clone()),
            _ => None,
        },
    }
}

/// Log the state when it or the licence differs from what was last logged.
fn log_change(current: &mut Current, now_ms: u64) {
    let status = best(&current.setting, &current.upload, now_ms);
    let id = status.licence.as_ref().map(|l| l.id.clone());
    let now = (status.state, id);
    if current.logged.as_ref() == Some(&now) {
        return;
    }
    let state = status.state.as_str();
    let licence = now.1.as_deref();
    let source = status.source.map(Source::as_str);
    let early = status
        .licence
        .as_ref()
        .is_some_and(|l| l.before_issue(now_ms));
    match status.state {
        State::Valid | State::Grace => {
            tracing::info!(state, licence, source, before_issue = early, "licence");
        }
        State::Expired | State::Missing | State::Invalid => tracing::warn!(
            state,
            licence,
            source,
            reason = status.reason,
            "licence; seat operations are unaffected"
        ),
    }
    current.logged = Some(now);
}

/// The state transitions against the clock, and which source counts.
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "tests")]

    use cavea_licence::Date;
    use cavea_licence::sign::SigningKey;

    use super::*;

    pub(crate) fn issue(key: &SigningKey, id: &str, issued: &str, expires: &str) -> Vec<u8> {
        let payload = format!(
            r#"{{"licence_id":"{id}","customer":"Test","plan":"starter","features":[],"domains":["tickets.example.com"],"max_workspaces":1,"issued_at":"{issued}","expires_at":"{expires}","grace_days":30}}"#
        );
        key.issue(payload.as_bytes()).unwrap().1
    }

    fn day(s: &str) -> u64 {
        Date::parse(s).unwrap().start_ms()
    }

    #[test]
    fn the_better_licence_counts_and_the_clock_moves_it() {
        let key = SigningKey::generate().unwrap().0;
        let verifier = Verifier::new([key.public()]);
        let old = issue(&key, "lic_old", "2026-01-01", "2027-01-10");
        let new = issue(&key, "lic_new", "2027-01-01", "2028-01-10");
        let set = Licences::new(verifier.clone(), Some(LicenceSource::Contents(old.clone())));
        let status = |at: &str| {
            let s = set.status(day(at));
            (s.state, s.source, s.licence.map(|l| l.id.clone()))
        };
        assert_eq!(
            status("2027-01-10"),
            (State::Valid, Some(Source::Setting), Some("lic_old".into()))
        );
        assert_eq!(status("2027-01-11").0, State::Grace);
        assert_eq!(status("2027-02-09").0, State::Grace);
        assert_eq!(status("2027-02-10").0, State::Expired);
        assert!(set.read_only(day("2027-02-10")));
        assert!(!set.read_only(day("2027-02-09")));
        // A newer upload wins; an invalid one never does.
        set.set_upload(Found::of(&verifier, &new), day("2027-02-10"));
        assert_eq!(
            status("2027-02-10"),
            (State::Valid, Some(Source::Upload), Some("lic_new".into()))
        );
        set.set_upload(Found::Invalid("signature"), day("2027-02-10"));
        assert_eq!(status("2027-02-10").0, State::Expired);
        // Garbage in the setting, nothing uploaded: invalid, with its reason.
        let garbage = Licences::new(
            verifier,
            Some(LicenceSource::Contents(b"{garbage".to_vec())),
        );
        let s = garbage.status(0);
        assert_eq!(
            (s.state, s.reason, s.source),
            (State::Invalid, Some("malformed"), Some(Source::Setting))
        );
        let none = Licences::new(Verifier::new([]), None);
        let s = none.status(0);
        assert_eq!((s.state, s.source), (State::Missing, None));
    }

    #[test]
    fn the_public_view_says_only_what_the_renderer_needs() {
        let key = SigningKey::generate().unwrap().0;
        let file = issue(&key, "lic_1", "2027-01-01", "2028-01-10");
        let set = Licences::new(
            Verifier::new([key.public()]),
            Some(LicenceSource::Contents(file.clone())),
        );
        let licensed = json!({ "status": "licensed", "domains": ["tickets.example.com"] });
        assert_eq!(set.public(day("2027-06-01")), licensed);
        assert_eq!(set.public(day("2028-01-20")), licensed);
        assert_eq!(
            set.public(day("2028-03-01")),
            json!({ "status": "expired", "domains": [] })
        );
        let untrusted = Licences::new(Verifier::new([]), Some(LicenceSource::Contents(file)));
        assert_eq!(
            untrusted.public(0),
            json!({ "status": "unlicensed", "domains": [] })
        );
    }

    #[test]
    fn the_setting_is_a_path_or_contents() {
        assert_eq!(
            "/etc/cavea/licence.json".parse::<LicenceSource>(),
            Ok(LicenceSource::Path("/etc/cavea/licence.json".into()))
        );
        assert_eq!(
            " {\"a\":1}".parse::<LicenceSource>(),
            Ok(LicenceSource::Contents(b" {\"a\":1}".to_vec()))
        );
        assert!("  ".parse::<LicenceSource>().is_err());
        assert_eq!(
            format!("{:?}", LicenceSource::Contents(b"{}".to_vec())),
            "Contents(..)"
        );
    }
}
