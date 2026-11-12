//! The seat stores: one conformance suite run against `MemoryStore` and
//! `PgStore`, the migrations, the engine's actor end to end on PostgreSQL, and
//! measurements.
//!
//! Database tests are `#[ignore]`d so that `cargo test` without PostgreSQL
//! reports them as ignored. To run them against a server (each test creates
//! and drops its own database):
//!
//! ```text
//! CAVEA_TEST_DATABASE_URL=postgres://cavea:cavea@127.0.0.1:5432/cavea \
//!   cargo test -p cavea-server --test store -- --include-ignored --skip measure_
//! ```
//!
//! Measurements: the same with `--release`, `measure_ --ignored --nocapture
//! --test-threads=1`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test helpers outside #[test] functions; a panic is a test failure"
)]

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use cavea_engine::{
    ActorConfig, Authority, Batch, BookDetail, Booking, ChartVersionId, ChartView, Clock, Command,
    EventId, EventState, Failure, HoldId, HoldRow, Idempotency, IdempotencyRecord, LoadedEvent,
    LogRecord, MEMORY_LEASE_MS, ManualClock, MemoryCharts, MemoryStore, Missing, ObjectFailure,
    ObjectReason, ObjectRef, ObjectRow, Op, Outcome, Registry, RegistryConfig, Request, SeatStore,
    Status, StoreError, Timestamp, TokenHash, actor,
};
use cavea_server::store::{self, MigrationError, PgStore};
use serde_json::Value;
use sqlx::postgres::{PgConnectOptions, PgConnection, PgPool};
use sqlx::{AssertSqlSafe, ConnectOptions as _, Connection as _, Executor as _};

// ---- A database per test ----

/// Run `body` against a new, uniquely named database, and drop the database
/// afterwards, also when `body` panics.
async fn with_db<F, Fut>(body: F)
where
    F: FnOnce(PgConnectOptions) -> Fut,
    Fut: Future<Output = ()> + 'static,
{
    let url = std::env::var("CAVEA_TEST_DATABASE_URL")
        .expect("database tests need CAVEA_TEST_DATABASE_URL, a PostgreSQL server URL");
    let admin = url
        .parse::<PgConnectOptions>()
        .unwrap()
        .disable_statement_logging();
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    let name = format!(
        "cavea_test_{}_{}_{}",
        std::process::id(),
        nanos.subsec_nanos(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    );
    let mut conn = PgConnection::connect_with(&admin).await.unwrap();
    // The name is generated above, never input.
    conn.execute(AssertSqlSafe(format!("CREATE DATABASE {name}")))
        .await
        .unwrap();
    // A task of its own, so that a panic comes back here as an error. Local:
    // the suite's futures are not `Send`.
    let body = body(admin.clone().database(&name));
    let local = tokio::task::LocalSet::new();
    let result = local
        .run_until(async move { tokio::task::spawn_local(body).await })
        .await;
    conn.execute(AssertSqlSafe(format!("DROP DATABASE {name} WITH (FORCE)")))
        .await
        .unwrap();
    if let Err(e) = result {
        std::panic::resume_unwind(e.into_panic());
    }
}

async fn migrated_pool(options: &PgConnectOptions) -> PgPool {
    store::migrate(options).await.unwrap();
    store::connect(options.clone(), 8).await.unwrap()
}

/// The lease both stores grant.
const LEASE: Duration = Duration::from_millis(MEMORY_LEASE_MS);

fn pg_store(pool: &PgPool, node: &str) -> PgStore {
    PgStore::new(pool.clone(), node, LEASE)
}

fn hex(event: EventId) -> String {
    format!("{:032x}", event.0)
}

/// The rows an event needs: a workspace, a chart and a chart version.
async fn insert_event(pool: &PgPool, event: EventId, epoch: u64) {
    sqlx::query(
        "WITH ws AS (
           INSERT INTO workspaces (id, key, name) VALUES (gen_random_uuid(), $1, 'Test')
           RETURNING id
         ), ch AS (
           INSERT INTO charts (id, workspace_id, key, name, draft)
           SELECT gen_random_uuid(), id, 'chart', 'Chart', '{}' FROM ws RETURNING id
         ), cv AS (
           INSERT INTO chart_versions
             (id, chart_id, number, hash, full_hash, source, public_ccf, public_ccf_br,
              full_ccf, stats)
           SELECT gen_random_uuid(), id, 1, '\\x00', '\\x00', '{}', '', '', '', '{}'
             FROM ch RETURNING id
         )
         INSERT INTO events (id, workspace_id, key, chart_version_id, owner_epoch)
         SELECT $1::uuid, ws.id, 'event', cv.id, $2 FROM ws, cv",
    )
    .bind(hex(event))
    .bind(i64::try_from(epoch).unwrap())
    .execute(pool)
    .await
    .unwrap();
}

// ---- The conformance suite ----

/// `(seq, op, actor, hold_id, oids)` of one stored log row.
type LogRow = (u64, u8, String, Option<u64>, Vec<u32>);

trait Harness {
    type Store: SeatStore;
    fn store(&self) -> &Self::Store;
    async fn create_event(&self, event: EventId, epoch: u64);
    /// Another node taking the event over.
    async fn set_epoch(&self, event: EventId, epoch: u64);
    /// The stored log, in `seq` order.
    async fn log(&self, event: EventId) -> Vec<LogRow>;
    /// A second node's store on the same data.
    fn peer(&self) -> Self::Store;
    /// Let the event's lease run out, by the store's clock.
    async fn lapse(&self, event: EventId);
    /// Milliseconds left on the event's lease, by the store's clock.
    async fn lease_left(&self, event: EventId) -> Option<i64>;
    async fn chart_version(&self, event: EventId) -> ChartVersionId;
}

impl Harness for MemoryStore {
    type Store = Self;
    fn store(&self) -> &Self {
        self
    }
    async fn create_event(&self, event: EventId, epoch: u64) {
        MemoryStore::create_event(self, event, epoch, ChartVersionId(event.0 + 1));
    }
    async fn set_epoch(&self, event: EventId, epoch: u64) {
        self.set_owner_epoch(event, epoch);
    }
    async fn log(&self, event: EventId) -> Vec<LogRow> {
        let batches = self.committed();
        let records = batches
            .iter()
            .filter(|b| b.event == event)
            .flat_map(|b| &b.log);
        records
            .map(|r| {
                (
                    r.seq,
                    r.op as u8,
                    r.actor.to_string(),
                    r.hold_id.map(|h| h.0),
                    r.oids.clone(),
                )
            })
            .collect()
    }
    fn peer(&self) -> Self {
        MemoryStore::peer(self, "node-2")
    }
    async fn lapse(&self, _: EventId) {
        self.clock().advance_ms(MEMORY_LEASE_MS + 1);
    }
    async fn lease_left(&self, event: EventId) -> Option<i64> {
        let until = self.lease(event).unwrap().1?;
        Some(until.0 as i64 - self.clock().now().0 as i64)
    }
    async fn chart_version(&self, event: EventId) -> ChartVersionId {
        ChartVersionId(event.0 + 1)
    }
}

struct Pg {
    store: PgStore,
    pool: PgPool,
}

impl Harness for Pg {
    type Store = PgStore;
    fn store(&self) -> &PgStore {
        &self.store
    }
    async fn create_event(&self, event: EventId, epoch: u64) {
        insert_event(&self.pool, event, epoch).await;
    }
    async fn set_epoch(&self, event: EventId, epoch: u64) {
        sqlx::query("UPDATE events SET owner_epoch = $2 WHERE id = $1::uuid")
            .bind(hex(event))
            .bind(i64::try_from(epoch).unwrap())
            .execute(&self.pool)
            .await
            .unwrap();
    }
    async fn log(&self, event: EventId) -> Vec<LogRow> {
        type Row = (i64, i16, String, Option<i64>, Vec<i32>);
        let rows: Vec<Row> = sqlx::query_as(
            "SELECT seq, op, actor, hold_id, oids FROM event_log
              WHERE event_id = $1::uuid ORDER BY seq",
        )
        .bind(hex(event))
        .fetch_all(&self.pool)
        .await
        .unwrap();
        rows.into_iter()
            .map(|(seq, op, actor, hold, oids)| {
                let oids = oids.into_iter().map(|o| o as u32).collect();
                (seq as u64, op as u8, actor, hold.map(|h| h as u64), oids)
            })
            .collect()
    }
    fn peer(&self) -> PgStore {
        pg_store(&self.pool, "node-2")
    }
    async fn lapse(&self, event: EventId) {
        sqlx::query("UPDATE events SET lease_until = now() - interval '1 ms' WHERE id = $1::uuid")
            .bind(hex(event))
            .execute(&self.pool)
            .await
            .unwrap();
    }
    async fn lease_left(&self, event: EventId) -> Option<i64> {
        sqlx::query_scalar(
            "SELECT (extract(epoch FROM lease_until - now()) * 1000)::int8
               FROM events WHERE id = $1::uuid",
        )
        .bind(hex(event))
        .fetch_one(&self.pool)
        .await
        .unwrap()
    }
    async fn chart_version(&self, event: EventId) -> ChartVersionId {
        chart_version(&self.pool, event).await
    }
}

async fn chart_version(pool: &PgPool, event: EventId) -> ChartVersionId {
    let id: String = sqlx::query_scalar(
        "SELECT replace(chart_version_id::text, '-', '') FROM events WHERE id = $1::uuid",
    )
    .bind(hex(event))
    .fetch_one(pool)
    .await
    .unwrap();
    ChartVersionId(u128::from_str_radix(&id, 16).unwrap())
}

/// Unix milliseconds now: the PostgreSQL store ages idempotency records by the
/// database clock.
fn now_ms() -> u64 {
    let since = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    u64::try_from(since.as_millis()).unwrap()
}

fn tok(n: u8) -> TokenHash {
    TokenHash([n; 32])
}

fn record(seq: u64, op: Op) -> LogRecord {
    LogRecord {
        seq,
        op,
        actor: "sk_test".into(),
        hold_id: None,
        oids: vec![],
        expires_at: None,
        max_objects: None,
        order_ref: None,
        status: None,
        details: vec![],
    }
}

/// `b` with its status counts, for a store whose objects are `before`: what
/// the engine computes, and the memory store checks.
fn counted(mut b: Batch, before: &[ObjectRow]) -> Batch {
    let was = |oid| {
        before
            .iter()
            .find(|o: &&ObjectRow| o.oid == oid)
            .map_or(0, |o| o.status.code())
    };
    let set = b.objects_set.iter().map(|o| (o.oid, o.status.code()));
    let mut counts = [0; 16];
    for (oid, now) in set.chain(b.objects_freed.iter().map(|&o| (o, 0))) {
        if was(oid) != now {
            counts[usize::from(was(oid))] -= 1;
            counts[usize::from(now)] += 1;
        }
    }
    b.counts = counts;
    b
}

fn batch(event: EventId, seq: u64, log: Vec<LogRecord>) -> Batch {
    Batch {
        event,
        epoch: 1,
        seq,
        holds_created: vec![],
        holds_extended: vec![],
        holds_deleted: vec![],
        objects_set: vec![],
        objects_freed: vec![],
        counts: [0; 16],
        log,
        idempotency: vec![],
    }
}

fn key_record(
    key: &str,
    fingerprint: u8,
    at: u64,
    reply: Result<Outcome, Failure>,
) -> IdempotencyRecord {
    IdempotencyRecord {
        key: key.into(),
        fingerprint: [fingerprint; 32],
        created_at: Timestamp(at),
        reply,
    }
}

/// Rows in a comparable order.
fn sorted(mut e: LoadedEvent) -> LoadedEvent {
    e.holds.sort_by_key(|h| h.id);
    e.objects.sort_by_key(|o| o.oid);
    e.idempotency.sort_by(|a, b| a.key.cmp(&b.key));
    e
}

async fn load<H: Harness>(h: &H, event: EventId) -> LoadedEvent {
    sorted(h.store().load_event(event).await.unwrap())
}

fn booked(order_ref: &str, ticket_type: Option<&str>, extra: Option<&str>) -> Option<Booking> {
    Some(Booking {
        order_ref: order_ref.into(),
        ticket_type: ticket_type.map(Into::into),
        extra: extra.map(Into::into),
    })
}

/// Every kind of batch content survives commit then load.
async fn round_trip<H: Harness>(h: &H) {
    let ev = EventId(0x1001);
    h.create_event(ev, 1).await;
    let t = now_ms();
    let h1 = HoldRow {
        id: HoldId(1),
        token_hash: tok(1),
        expires_at: Timestamp(t + 900_000),
        max_objects: Some(10),
    };
    let h2 = HoldRow {
        id: HoldId(2),
        token_hash: tok(2),
        expires_at: Timestamp(t + 600_000),
        max_objects: None,
    };
    // Valid JSON that `jsonb` would reject.
    let extra = r#"{"b": [1,2],  "a":"é\u0000"}"#;
    let object = |oid, status, hold_id: Option<u64>, booking, seq| ObjectRow {
        oid,
        status,
        hold_id: hold_id.map(HoldId),
        booking,
        seq,
    };
    let custom = Status::from_code(5).unwrap();
    let mut b1 = batch(
        ev,
        5,
        vec![
            LogRecord {
                hold_id: Some(HoldId(1)),
                expires_at: Some(h1.expires_at),
                ..record(1, Op::CreateHold)
            },
            LogRecord {
                hold_id: Some(HoldId(2)),
                expires_at: Some(h2.expires_at),
                ..record(2, Op::CreateHold)
            },
            LogRecord {
                hold_id: Some(HoldId(1)),
                oids: vec![100],
                ..record(3, Op::Hold)
            },
            LogRecord {
                oids: vec![101, 102],
                order_ref: Some("77120".into()),
                details: vec![(101, Some("adult".into()), Some(extra.into()))],
                ..record(4, Op::Book)
            },
            LogRecord {
                oids: vec![u32::MAX],
                status: Some(Status::BLOCKED),
                ..record(5, Op::ChangeStatus)
            },
        ],
    );
    b1.holds_created = vec![h1.clone(), h2.clone()];
    b1.objects_set = vec![
        object(100, Status::HELD, Some(1), None, 3),
        object(
            101,
            Status::BOOKED,
            None,
            booked("77120", Some("adult"), Some(extra)),
            4,
        ),
        object(102, Status::BOOKED, None, booked("77120", None, None), 4),
        object(u32::MAX, Status::BLOCKED, None, None, 5),
    ];
    let created = Ok(Outcome::HoldCreated {
        seq: 1,
        hold: HoldId(1),
        expires_at: h1.expires_at,
    });
    let refused = Err(Failure::ObjectNotFree(vec![ObjectFailure {
        idx: 3,
        reason: ObjectReason::NotFree,
    }]));
    // A refusal echoes the caller's labels, which may hold any character.
    let missing = Err(Failure::NotFound(Missing::Objects(vec![ObjectRef::Label(
        "A\0B".into(),
    )])));
    b1.idempotency = vec![
        key_record("k-created", 1, t, created),
        key_record("k-missing", 3, t, missing),
        key_record("k-refused", 2, t, refused),
    ];
    let b1 = counted(b1, &[]);
    h.store().commit_batch(b1.clone()).await.unwrap();
    let expected = sorted(LoadedEvent {
        epoch: 1,
        seq: 5,
        holds: b1.holds_created.clone(),
        objects: b1.objects_set.clone(),
        idempotency: b1.idempotency.clone(),
    });
    assert_eq!(load(h, ev).await, expected);

    // Extend, delete, free, change; the log continues.
    let mut b2 = batch(
        ev,
        8,
        vec![
            LogRecord {
                hold_id: Some(HoldId(1)),
                oids: vec![100],
                ..record(6, Op::Expire)
            },
            LogRecord {
                hold_id: Some(HoldId(2)),
                expires_at: Some(Timestamp(t + 1_200_000)),
                ..record(7, Op::ExtendHold)
            },
            LogRecord {
                oids: vec![102, u32::MAX],
                status: Some(custom),
                ..record(8, Op::ChangeStatus)
            },
        ],
    );
    b2.holds_extended = vec![(HoldId(2), Timestamp(t + 1_200_000))];
    b2.holds_deleted = vec![HoldId(1)];
    b2.objects_freed = vec![100, 102];
    b2.objects_set = vec![object(u32::MAX, custom, None, None, 8)];
    let b2 = counted(b2, &b1.objects_set);
    h.store().commit_batch(b2.clone()).await.unwrap();
    let expected = LoadedEvent {
        seq: 8,
        holds: vec![HoldRow {
            expires_at: Timestamp(t + 1_200_000),
            ..h2
        }],
        objects: vec![
            expected.objects[1].clone(),
            object(u32::MAX, custom, None, None, 8),
        ],
        ..expected
    };
    assert_eq!(load(h, ev).await, expected);

    let log: Vec<LogRow> = b1
        .log
        .iter()
        .chain(&b2.log)
        .map(|r| {
            (
                r.seq,
                r.op as u8,
                r.actor.to_string(),
                r.hold_id.map(|h| h.0),
                r.oids.clone(),
            )
        })
        .collect();
    assert_eq!(h.log(ev).await, log);
}

/// A live record is never overwritten; one 24 hours old is. A batch of only
/// records leaves `seq` as it is.
async fn idempotency_records<H: Harness>(h: &H) {
    let ev = EventId(0x1002);
    h.create_event(ev, 1).await;
    let t0 = now_ms() - 3_600_000;
    let original = Ok(Outcome::Extended {
        seq: 1,
        expires_at: Timestamp(t0),
    });
    let mut b = batch(ev, 1, vec![record(1, Op::ExtendHold)]);
    b.idempotency = vec![key_record("k", 1, t0, original.clone())];
    h.store().commit_batch(b).await.unwrap();

    let mut only = batch(ev, 1, vec![]);
    only.idempotency = vec![key_record("k", 2, t0 + 60_000, Err(Failure::Forbidden))];
    h.store().commit_batch(only).await.unwrap();
    let loaded = load(h, ev).await;
    assert_eq!(loaded.seq, 1);
    assert_eq!(loaded.idempotency, vec![key_record("k", 1, t0, original)]);

    let day = 24 * 3_600_000;
    let mut only = batch(ev, 1, vec![]);
    only.idempotency = vec![key_record("k", 3, t0 + day, Err(Failure::Forbidden))];
    h.store().commit_batch(only).await.unwrap();
    let loaded = load(h, ev).await;
    assert_eq!(loaded.seq, 1);
    assert_eq!(
        loaded.idempotency,
        vec![key_record("k", 3, t0 + day, Err(Failure::Forbidden))]
    );
}

/// A batch that does not fit the stored state is refused and writes nothing.
async fn refusals<H: Harness>(h: &H) {
    let ev = EventId(0x1003);
    h.create_event(ev, 1).await;
    let t = now_ms();
    let hold = HoldRow {
        id: HoldId(1),
        token_hash: tok(9),
        expires_at: Timestamp(t),
        max_objects: None,
    };
    let full = |seq: u64| {
        let mut b = batch(ev, seq, vec![record(seq, Op::CreateHold)]);
        b.holds_created = vec![HoldRow {
            id: HoldId(seq),
            token_hash: tok(seq as u8),
            ..hold.clone()
        }];
        b.objects_set = vec![ObjectRow {
            oid: 7,
            status: Status::BLOCKED,
            hold_id: None,
            booking: None,
            seq,
        }];
        b.idempotency = vec![key_record("r", 1, t, Err(Failure::Forbidden))];
        counted(b, &[])
    };
    let before = load(h, ev).await;
    let inconsistent = |r| matches!(r, Err(StoreError::Inconsistent(_)));

    // A gap, a repeat, and a log that does not end at `seq`.
    assert!(inconsistent(h.store().commit_batch(full(2)).await));
    let mut wrong_end = full(1);
    wrong_end.seq = 2;
    assert!(inconsistent(h.store().commit_batch(wrong_end).await));
    // Extending or deleting a hold the store does not have.
    let mut extend = full(1);
    extend.holds_extended = vec![(HoldId(99), Timestamp(t))];
    assert!(inconsistent(h.store().commit_batch(extend).await));
    let mut delete = full(1);
    delete.holds_deleted = vec![HoldId(98)];
    assert!(inconsistent(h.store().commit_batch(delete).await));
    // An empty batch.
    assert!(inconsistent(
        h.store().commit_batch(batch(ev, 0, vec![])).await
    ));
    assert_eq!(load(h, ev).await, before);

    h.store().commit_batch(full(1)).await.unwrap();
    let after_one = load(h, ev).await;
    assert!(
        inconsistent(h.store().commit_batch(full(1)).await),
        "a repeated seq"
    );

    // Another node took the event over.
    h.set_epoch(ev, 2).await;
    assert_eq!(
        h.store().commit_batch(full(2)).await,
        Err(StoreError::Fenced)
    );
    let mut fenced_only = batch(ev, 1, vec![]);
    fenced_only.idempotency = vec![key_record("f", 1, t, Err(Failure::Forbidden))];
    assert_eq!(
        h.store().commit_batch(fenced_only).await,
        Err(StoreError::Fenced)
    );
    assert_eq!(
        load(h, ev).await,
        LoadedEvent {
            epoch: 2,
            ..after_one
        }
    );
    assert_eq!(h.log(ev).await.len(), 1);

    let unknown = EventId(0xdead);
    assert_eq!(
        h.store().load_event(unknown).await,
        Err(StoreError::UnknownEvent)
    );
    let mut b = full(1);
    b.event = unknown;
    assert_eq!(
        h.store().commit_batch(b).await,
        Err(StoreError::UnknownEvent)
    );
}

/// Ownership (08 §4.7 step 1, §4.8): acquiring raises the epoch; a live lease
/// refuses another node; commits and heartbeats renew it by the store's clock;
/// once it lapses another node takes over and the old owner's commit,
/// heartbeat and release are fenced; a release frees the event at once.
async fn ownership<H: Harness>(h: &H) {
    let ev = EventId(0x1004);
    h.create_event(ev, 0).await;
    let (me, other) = (h.store(), h.peer());
    let t = now_ms();
    let keyed = |epoch, seq, log, key| {
        let mut b = batch(ev, seq, log);
        b.epoch = epoch;
        b.idempotency = vec![key_record(key, 1, t, Err(Failure::Forbidden))];
        b
    };

    let first = me.acquire(ev).await.unwrap();
    assert_eq!(first.epoch, 1);
    assert_eq!(first.chart_version, h.chart_version(ev).await);
    assert!(h.lease_left(ev).await.unwrap() > 14_000);
    assert_eq!(other.acquire(ev).await, Err(StoreError::Leased));
    assert_eq!(load(h, ev).await.epoch, 1, "a refusal changes nothing");

    // 08 §4.6: a commit of only idempotency records, timed out at epoch 1,
    // lands after the same node reloaded the event. Reloading raised the
    // epoch, so the late write is refused instead of slipping in unnoticed.
    assert_eq!(me.acquire(ev).await.unwrap().epoch, 2);
    let late = keyed(1, 0, vec![], "late");
    assert_eq!(me.commit_batch(late).await, Err(StoreError::Fenced));
    assert!(load(h, ev).await.idempotency.is_empty());

    // A lapsed lease that nobody took still commits, and the commit renews it.
    h.lapse(ev).await;
    let b1 = keyed(2, 1, vec![record(1, Op::ChangeStatus)], "k1");
    me.commit_batch(b1).await.unwrap();
    assert!(h.lease_left(ev).await.unwrap() > 14_000);
    h.lapse(ev).await;
    me.heartbeat(ev, 2).await.unwrap();
    assert!(h.lease_left(ev).await.unwrap() > 14_000);

    // Takeover after the lease lapses.
    h.lapse(ev).await;
    assert_eq!(other.acquire(ev).await.unwrap().epoch, 3);
    let b2 = keyed(2, 2, vec![record(2, Op::ChangeStatus)], "k2");
    assert_eq!(me.commit_batch(b2).await, Err(StoreError::Fenced));
    assert_eq!(me.heartbeat(ev, 2).await, Err(StoreError::Fenced));
    assert_eq!(me.release(ev, 2).await, Err(StoreError::Fenced));
    assert_eq!(me.acquire(ev).await, Err(StoreError::Leased));
    let b2 = keyed(3, 2, vec![record(2, Op::ChangeStatus)], "k2");
    other.commit_batch(b2).await.unwrap();
    let seqs: Vec<u64> = h.log(ev).await.iter().map(|r| r.0).collect();
    assert_eq!(seqs, [1, 2]);

    // A released event is free to any node at once.
    other.release(ev, 3).await.unwrap();
    assert_eq!(h.lease_left(ev).await, None);
    assert_eq!(me.acquire(ev).await.unwrap().epoch, 4);
    assert_eq!(
        me.acquire(EventId(0xdead)).await,
        Err(StoreError::UnknownEvent)
    );
}

async fn conformance<H: Harness>(h: &H) {
    round_trip(h).await;
    idempotency_records(h).await;
    refusals(h).await;
    ownership(h).await;
}

#[tokio::test]
async fn memory_store_conformance() {
    conformance(&MemoryStore::new()).await;
}

#[tokio::test]
#[ignore = "needs PostgreSQL: set CAVEA_TEST_DATABASE_URL, pass --include-ignored"]
async fn pg_store_conformance() {
    with_db(|options| async move {
        let pool = migrated_pool(&options).await;
        let pg = Pg {
            store: pg_store(&pool, "node-1"),
            pool,
        };
        conformance(&pg).await;

        // The log's order reference and `detail`, which only the log keeps.
        let rows: Vec<(i64, Option<String>, Option<String>)> = sqlx::query_as(
            "SELECT seq, order_ref, detail::text FROM event_log
              WHERE event_id = $1::uuid ORDER BY seq",
        )
        .bind(hex(EventId(0x1001)))
        .fetch_all(&pg.pool)
        .await
        .unwrap();
        let parse = |d: &Option<String>| {
            d.as_deref()
                .map(|d| serde_json::from_str::<Value>(d).unwrap())
        };
        let json = |s: &str| Some(serde_json::from_str::<Value>(s).unwrap());
        let t = |seq: usize| rows[seq - 1].clone();
        assert!(parse(&t(1).2).unwrap()["expires_at"].is_u64());
        assert_eq!(parse(&t(3).2), None);
        assert_eq!(t(4).1.as_deref(), Some("77120"));
        assert_eq!(
            parse(&t(4).2),
            json(r#"{"objects": [[101, "adult", {"a": "é\u0000", "b": [1, 2]}]]}"#)
        );
        assert_eq!(parse(&t(5).2), json(r#"{"status": 3}"#));
    })
    .await;
}

/// A statement failing after the fence has run (here the last one, two
/// records for one key) leaves nothing written.
#[tokio::test]
#[ignore = "needs PostgreSQL: set CAVEA_TEST_DATABASE_URL, pass --include-ignored"]
async fn pg_store_failed_commit_writes_nothing() {
    with_db(|options| async move {
        let pool = migrated_pool(&options).await;
        let store = pg_store(&pool, "node-1");
        let ev = EventId(0x2001);
        insert_event(&pool, ev, 1).await;
        let before = store.load_event(ev).await.unwrap();
        let mut b = batch(
            ev,
            1,
            vec![LogRecord {
                oids: vec![1],
                ..record(1, Op::ChangeStatus)
            }],
        );
        b.holds_created = vec![HoldRow {
            id: HoldId(1),
            token_hash: tok(1),
            expires_at: Timestamp(now_ms()),
            max_objects: None,
        }];
        b.objects_set = vec![ObjectRow {
            oid: 1,
            status: Status::BLOCKED,
            hold_id: None,
            booking: None,
            seq: 1,
        }];
        let r = key_record("dup", 1, now_ms(), Err(Failure::Forbidden));
        b.idempotency = vec![r.clone(), r];
        let b = counted(b, &[]);
        assert!(matches!(
            store.commit_batch(b).await,
            Err(StoreError::Inconsistent(_))
        ));
        assert_eq!(store.load_event(ev).await.unwrap(), before);
        let rows: i64 = sqlx::query_scalar(
            "SELECT (SELECT count(*) FROM event_log) + (SELECT count(*) FROM holds)
                  + (SELECT count(*) FROM object_state) + (SELECT count(*) FROM idempotency)",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(rows, 0);
    })
    .await;
}

// ---- Migrations ----

async fn migration_rows(pool: &PgPool) -> Vec<i64> {
    sqlx::query_scalar("SELECT version FROM cavea_migrations ORDER BY version")
        .fetch_all(pool)
        .await
        .unwrap()
}

/// A TCP forwarder to PostgreSQL that admits at most `limit` connections
/// and can be cut and restored: the database full, down, back. Returns its
/// port, the switch, and how many connections it forwards.
async fn proxy(
    target: (String, u16),
    limit: usize,
) -> (u16, tokio::sync::watch::Sender<bool>, Arc<AtomicU32>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (up, watch) = tokio::sync::watch::channel(true);
    let live = Arc::new(AtomicU32::new(0));
    let count = live.clone();
    tokio::spawn(async move {
        loop {
            let (mut client, _) = listener.accept().await.unwrap();
            if !*watch.borrow() || count.load(Ordering::SeqCst) as usize >= limit {
                continue; // dropped: the client reads the end of the stream
            }
            count.fetch_add(1, Ordering::SeqCst);
            let (mut down, count, target) = (watch.clone(), count.clone(), target.clone());
            tokio::spawn(async move {
                let mut server = tokio::net::TcpStream::connect(target).await.unwrap();
                tokio::select! {
                    _ = tokio::io::copy_bidirectional(&mut client, &mut server) => {}
                    _ = down.wait_for(|up| !*up) => {}
                }
                count.fetch_sub(1, Ordering::SeqCst);
            });
        }
    });
    (port, up, live)
}

/// The pool opens every connection at start and keeps them (13 §7.4): a pool
/// that grew on demand opened PostgreSQL connections inside the first burst
/// of requests, which put the snapshot route's p99 at 9 to 12 ms on a fresh
/// process. An unreachable database still stops the start; one that refuses
/// some connections does not; after an outage the pool serves again.
#[tokio::test]
#[ignore = "needs PostgreSQL: set CAVEA_TEST_DATABASE_URL, pass --include-ignored"]
async fn the_pool_opens_what_it_can_at_start_and_recovers() {
    with_db(|options| async move {
        let pool = store::connect(options.clone(), 3).await.unwrap();
        assert_eq!(pool.size(), 3);
        // Connections go back to the pool on a task of their own.
        while pool.num_idle() < 3 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }

        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = closed.local_addr().unwrap().port();
        drop(closed);
        let local = options.clone().host("127.0.0.1");
        assert!(store::connect(local.clone().port(port), 2).await.is_err());

        let target = (options.get_host().to_owned(), options.get_port());
        let (port, up, live) = proxy(target, 2).await;
        let pool = store::connect(local.port(port), 4).await.unwrap();
        assert_eq!(
            pool.size(),
            2,
            "the start goes on with what PostgreSQL allows"
        );

        up.send_replace(false);
        while live.load(Ordering::SeqCst) > 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(sqlx::query("SELECT 1").execute(&pool).await.is_err());
        up.send_replace(true);
        sqlx::query("SELECT 1").execute(&pool).await.unwrap();
    })
    .await;
}

#[tokio::test]
#[ignore = "needs PostgreSQL: set CAVEA_TEST_DATABASE_URL, pass --include-ignored"]
async fn migrations_apply_once_and_refuse_an_unknown_schema() {
    with_db(|options| async move {
        // Two runners at once on an empty database: both succeed, one applies.
        let (a, b) = tokio::join!(store::migrate(&options), store::migrate(&options));
        a.unwrap();
        b.unwrap();
        let pool = store::connect(options.clone(), 2).await.unwrap();
        assert_eq!(migration_rows(&pool).await, [1, 2, 3, 4, 5, 6, 7]);
        // Again: nothing to do.
        store::migrate(&options).await.unwrap();
        assert_eq!(migration_rows(&pool).await, [1, 2, 3, 4, 5, 6, 7]);

        // A migration from a newer server.
        sqlx::query(
            "INSERT INTO cavea_migrations (version, description, success, checksum, execution_time)
             VALUES (8, 'future', true, '\\x00', 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        let err = store::migrate(&options).await.unwrap_err();
        assert!(matches!(err, MigrationError::Newer(8)), "{err}");
        assert!(err.to_string().contains("newer"), "{err}");
        sqlx::query("DELETE FROM cavea_migrations WHERE version = 8")
            .execute(&pool)
            .await
            .unwrap();

        // An applied migration edited since.
        sqlx::query("UPDATE cavea_migrations SET checksum = '\\x00' WHERE version = 1")
            .execute(&pool)
            .await
            .unwrap();
        let err = store::migrate(&options).await.unwrap_err();
        assert!(matches!(err, MigrationError::Changed(1)), "{err}");
    })
    .await;
}

// ---- The actor on PostgreSQL ----

const EV: EventId = EventId(0x3001);

fn chart(n: u32) -> Arc<ChartView> {
    Arc::new(ChartView::new((0..n).map(|i| (100 + i, format!("S-{i}")))).unwrap())
}

fn labels(idxs: &[u32]) -> Vec<ObjectRef> {
    idxs.iter()
        .map(|i| ObjectRef::Label(format!("S-{i}").into()))
        .collect()
}

fn req(command: Command) -> Request {
    Request {
        actor: "sk_test".into(),
        authority: Authority::Secret,
        command,
        idempotency: None,
    }
}

fn keyed(command: Command, key: &str, fingerprint: u8, at: Timestamp) -> Request {
    Request {
        idempotency: Some(Idempotency {
            key: key.into(),
            fingerprint: [fingerprint; 32],
            checked_at: at,
        }),
        ..req(command)
    }
}

fn create(t: u8, max: Option<u32>) -> Command {
    Command::CreateHold {
        token: tok(t),
        ttl_secs: 900,
        max_objects: max,
    }
}

fn hold(t: u8, idxs: &[u32]) -> Command {
    Command::Hold {
        token: tok(t),
        objects: labels(idxs),
    }
}

fn book(t: Option<u8>, idxs: &[u32], order: &str, details: Vec<BookDetail>) -> Command {
    Command::Book {
        token: t.map(tok),
        objects: labels(idxs),
        order_ref: order.into(),
        details,
    }
}

type Fingerprint = (
    u64,
    Vec<(Option<Status>, Option<HoldId>, Option<Booking>)>,
    Vec<(HoldId, Timestamp, Option<u32>, Vec<u32>)>,
);

/// Everything observable about the state, in a comparable order.
fn fingerprint(s: &EventState) -> Fingerprint {
    let objects = (0..s.chart().len() as u32)
        .map(|i| (s.status(i), s.holder(i), s.booking(i).cloned()))
        .collect();
    let mut holds: Vec<_> = s
        .holds()
        .map(|h| {
            let mut o = h.objects.to_vec();
            o.sort_unstable();
            (h.id, h.expires_at, h.max_objects, o)
        })
        .collect();
    holds.sort();
    (s.seq(), objects, holds)
}

/// INV-6 and INV-7 end to end: every reply the actor released is durable, and
/// reloading from PostgreSQL restores exactly the live state, including the
/// idempotency records.
#[tokio::test]
#[ignore = "needs PostgreSQL: set CAVEA_TEST_DATABASE_URL, pass --include-ignored"]
async fn the_actor_on_postgres_reloads_to_its_live_state() {
    with_db(|options| async move {
        let pool = migrated_pool(&options).await;
        let store = Arc::new(pg_store(&pool, "node-1"));
        insert_event(&pool, EV, 1).await;
        let loaded = store.load_event(EV).await.unwrap();
        // `mirror` applies every command the actor gets, at the same time.
        let mut mirror = EventState::restore(EV, chart(20), loaded.clone()).unwrap();
        let state = EventState::restore(EV, chart(20), loaded).unwrap();
        let clock = ManualClock::new(Timestamp(now_ms()));
        let (handle, run) = actor(state, store.clone(), clock.clone(), ActorConfig::default());
        let task = tokio::spawn(run);

        let detail = BookDetail {
            object: ObjectRef::Idx(3),
            ticket_type: Some("adult".into()),
            extra: Some(r#"{"seat": "A-1",  "n": [1]}"#.into()),
        };
        let t0 = clock.now();
        let commands = [
            req(create(1, Some(4))),
            req(create(2, None)),
            req(create(3, None)),
            req(hold(1, &[0, 1, 2])),
            req(hold(2, &[3, 4])),
            req(hold(3, &[5])),
            keyed(book(Some(2), &[3, 4], "order-1", vec![detail]), "k1", 1, t0),
            req(book(None, &[6, 7], "order-2", vec![])),
            req(Command::Release {
                token: tok(1),
                objects: labels(&[0]),
            }),
            req(Command::ReleaseBooked {
                objects: labels(&[6]),
            }),
            req(Command::ChangeStatus {
                objects: labels(&[8, 9]),
                status: Status::BLOCKED,
            }),
            req(Command::ChangeStatus {
                objects: labels(&[10]),
                status: Status::from_code(5).unwrap(),
            }),
            req(Command::ExtendHold {
                token: tok(3),
                ttl_secs: 1_800,
            }),
            // Booking took token 2's last objects, so the token is gone.
            keyed(hold(2, &[11]), "k2", 2, t0),
        ];
        let mut replies = Vec::new();
        for command in commands {
            let expected = mirror.apply(&command, clock.now()).unwrap();
            let reply = handle.send(command).await;
            assert_eq!(reply, expected);
            replies.push(reply);
        }
        assert_eq!(replies[13], Err(Failure::NotFound(Missing::HoldToken)));

        // Hold 1 expires on the actor's clock; hold 3 was extended past it.
        clock.advance_ms(900_000);
        for command in [req(create(4, None)), req(hold(4, &[1]))] {
            let expected = mirror.apply(&command, clock.now()).unwrap();
            assert_eq!(handle.send(command).await, expected);
        }
        drop(handle);
        assert_eq!(task.await.unwrap(), Ok(()));

        let reloaded =
            EventState::restore(EV, chart(20), store.load_event(EV).await.unwrap()).unwrap();
        assert_eq!(fingerprint(&reloaded), fingerprint(&mirror));
        assert_eq!(reloaded.seq(), 16, "15 changing commands and one expiry");

        // Retries after the reload get the original replies.
        let (handle, run) = actor(
            reloaded,
            store.clone(),
            clock.clone(),
            ActorConfig::default(),
        );
        let task = tokio::spawn(run);
        let retry = keyed(
            book(Some(2), &[3, 4], "order-1", vec![]),
            "k1",
            1,
            clock.now(),
        );
        assert_eq!(handle.send(retry).await, replies[6]);
        assert_eq!(
            handle
                .send(keyed(hold(2, &[11]), "k2", 2, clock.now()))
                .await,
            replies[13]
        );
        let other = keyed(hold(2, &[11]), "k2", 9, clock.now());
        assert_eq!(handle.send(other).await, Err(Failure::IdempotencyMismatch));
        drop(handle);
        assert_eq!(task.await.unwrap(), Ok(()));
    })
    .await;
}

/// Two nodes' registries on one database (08 §4.8, §14): the second node is
/// refused while the first holds the lease, and takes the event at once when
/// the first shuts down and releases it, continuing its log.
#[tokio::test]
#[ignore = "needs PostgreSQL: set CAVEA_TEST_DATABASE_URL, pass --include-ignored"]
async fn registries_on_postgres_hand_an_event_over_at_shutdown() {
    with_db(|options| async move {
        let pool = migrated_pool(&options).await;
        insert_event(&pool, EV, 0).await;
        let charts = MemoryCharts::new();
        charts.insert(chart_version(&pool, EV).await, chart(20));
        let charts = Arc::new(charts);
        let clock = ManualClock::new(Timestamp(now_ms()));
        let node = |name| {
            let store = Arc::new(pg_store(&pool, name));
            Registry::new(
                store,
                charts.clone(),
                clock.clone(),
                RegistryConfig::default(),
            )
        };
        let (a, b) = (node("node-1"), node("node-2"));
        let owner = || async {
            sqlx::query_as::<_, (Option<String>, i64, bool)>(
                "SELECT owner_node, owner_epoch, lease_until IS NOT NULL FROM events",
            )
            .fetch_one(&pool)
            .await
            .unwrap()
        };

        assert!(a.send(EV, req(create(1, None))).await.is_ok());
        assert!(a.send(EV, req(hold(1, &[2]))).await.is_ok());
        assert_eq!(owner().await, (Some("node-1".into()), 1, true));
        assert_eq!(
            b.send(EV, req(create(2, None))).await,
            Err(Failure::Unavailable)
        );
        assert_eq!(owner().await.1, 1, "the refusal changed nothing");

        a.shutdown().await;
        assert_eq!(owner().await, (Some("node-1".into()), 1, false));
        // Past the pause B's refused load imposes on retries.
        clock.advance_ms(RegistryConfig::default().retry_ms);
        let reply = b.send(EV, req(create(2, None))).await;
        assert!(
            matches!(reply, Ok(Outcome::HoldCreated { seq: 3, .. })),
            "{reply:?}"
        );
        assert_eq!(owner().await, (Some("node-2".into()), 2, true));
        b.shutdown().await;

        let pg = Pg {
            store: pg_store(&pool, "node-3"),
            pool: pool.clone(),
        };
        let ops: Vec<(u64, u8)> = pg.log(EV).await.iter().map(|r| (r.0, r.1)).collect();
        let expected = [Op::CreateHold, Op::Hold, Op::CreateHold].map(|op| op as u8);
        assert_eq!(ops, [(1, expected[0]), (2, expected[1]), (3, expected[2])]);
    })
    .await;
}

// ---- Measurements (print, assert nothing) ----

fn percentile(sorted: &[Duration], p: f64) -> Duration {
    let i = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[i]
}

/// Commit latency for batches of 1, 100 and 2,000 commands (hold-shaped:
/// create a hold, then hold one seat under it).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "measurement; needs PostgreSQL"]
async fn measure_commit_latency() {
    with_db(|options| async move {
        let pool = migrated_pool(&options).await;
        let store = pg_store(&pool, "node-1");
        insert_event(&pool, EV, 1).await;
        let seats = 200_000;
        let mut s = EventState::restore(EV, chart(seats), store.load_event(EV).await.unwrap()).unwrap();
        let now = Timestamp(now_ms());
        let mut times = Vec::new();
        for _ in 0..200 {
            let start = Instant::now();
            sqlx::query("SELECT 1").execute(&pool).await.unwrap();
            times.push(start.elapsed());
        }
        times.sort();
        println!("round trip (SELECT 1): median {:.2?}", percentile(&times, 0.5));
        let (mut token, mut seat) = (0u32, 0u32);
        for (size, rounds) in [(1usize, 200), (100, 100), (2_000, 30)] {
            let mut times = Vec::new();
            for _ in 0..rounds {
                for n in 0..size {
                    let mut hash = [0; 32];
                    hash[..4].copy_from_slice(&token.to_le_bytes());
                    let command = if n % 2 == 0 {
                        Command::CreateHold { token: TokenHash(hash), ttl_secs: 900, max_objects: None }
                    } else {
                        seat += 1;
                        let c = Command::Hold { token: TokenHash(hash), objects: vec![ObjectRef::Idx(seat % seats)] };
                        token += 1;
                        c
                    };
                    assert!(s.apply(&req(command), now).unwrap().is_ok());
                }
                if size == 1 {
                    token += 1;
                }
                let batch = s.take_batch().unwrap().unwrap();
                let start = Instant::now();
                store.commit_batch(batch).await.unwrap();
                times.push(start.elapsed());
            }
            times.sort();
            println!(
                "commit of {size:>5} commands: median {:>8.2?}  p99 {:>8.2?}  max {:>8.2?}  ({rounds} commits)",
                percentile(&times, 0.5),
                percentile(&times, 0.99),
                times[times.len() - 1]
            );
        }
    })
    .await;
}

/// Sustained commands per second for one event through the real actor, with
/// many commands in flight.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "measurement; needs PostgreSQL"]
async fn measure_actor_throughput() {
    with_db(|options| async move {
        let pool = migrated_pool(&options).await;
        let store = Arc::new(pg_store(&pool, "node-1"));
        insert_event(&pool, EV, 1).await;
        let seats = 100_000u32;
        let state =
            EventState::restore(EV, chart(seats), store.load_event(EV).await.unwrap()).unwrap();
        let clock = ManualClock::new(Timestamp(now_ms()));
        let (handle, run) = actor(state, store.clone(), clock, ActorConfig::default());
        let task = tokio::spawn(run);
        let total = 2 * seats as usize;
        let start = Instant::now();
        let mut pending = std::collections::VecDeque::new();
        for i in 0..seats {
            let mut hash = [0; 32];
            hash[..4].copy_from_slice(&i.to_le_bytes());
            for command in [
                Command::CreateHold {
                    token: TokenHash(hash),
                    ttl_secs: 900,
                    max_objects: None,
                },
                Command::Hold {
                    token: TokenHash(hash),
                    objects: vec![ObjectRef::Idx(i)],
                },
            ] {
                let request = req(command);
                loop {
                    match handle.submit(request.clone()) {
                        Ok(reply) => {
                            pending.push_back(reply);
                            break;
                        }
                        Err(Failure::Overloaded) => {
                            pending.pop_front().unwrap().await.unwrap();
                        }
                        Err(e) => panic!("{e:?}"),
                    }
                }
            }
        }
        for reply in pending {
            reply.await.unwrap();
        }
        let elapsed = start.elapsed();
        drop(handle);
        task.await.unwrap().unwrap();
        println!(
            "actor: {total} commands in {elapsed:.2?}: {:.0} commands per second",
            total as f64 / elapsed.as_secs_f64()
        );
    })
    .await;
}

/// Loading an event with 100,000 non-default objects: half booked with an
/// order reference and ticket type, half blocked.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "measurement; needs PostgreSQL"]
async fn measure_load() {
    with_db(|options| async move {
        let pool = migrated_pool(&options).await;
        let store = pg_store(&pool, "node-1");
        insert_event(&pool, EV, 1).await;
        sqlx::query(
            "INSERT INTO object_state (event_id, oid, status, order_ref, ticket_type, seq)
             SELECT $1::uuid, 100 + g, CASE WHEN g % 2 = 0 THEN 2 ELSE 3 END,
                    CASE WHEN g % 2 = 0 THEN 'order-' || g END,
                    CASE WHEN g % 2 = 0 THEN 'adult' END, g + 1
               FROM generate_series(0, 99999) g",
        )
        .bind(hex(EV))
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("UPDATE events SET seq = 100000").execute(&pool).await.unwrap();
        sqlx::query("ANALYZE").execute(&pool).await.unwrap();
        let chart = chart(100_000);
        let mut loads = Vec::new();
        let mut restores = Vec::new();
        for _ in 0..10 {
            let start = Instant::now();
            let loaded = store.load_event(EV).await.unwrap();
            loads.push(start.elapsed());
            assert_eq!(loaded.objects.len(), 100_000);
            let start = Instant::now();
            let _ = EventState::restore(EV, chart.clone(), loaded).unwrap();
            restores.push(start.elapsed());
        }
        loads.sort();
        restores.sort();
        println!(
            "load of 100,000 objects: median {:.2?} (max {:.2?}); restore into memory median {:.2?}",
            percentile(&loads, 0.5),
            loads[loads.len() - 1],
            percentile(&restores, 0.5)
        );
    })
    .await;
}
