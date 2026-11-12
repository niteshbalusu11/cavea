//! The reconciler (13 §3.3) against PostgreSQL: events written by the real
//! engine and `PgStore` verify clean, each kind of corruption made with SQL is
//! reported precisely, and verifying while commits land reports nothing.

use std::time::Duration;

use cavea_engine::{
    Authority, BookDetail, Command, EventId, EventState, ObjectRef, Request, SeatStore, Status,
    Timestamp, TokenHash,
};

use super::*;
use crate::seats::{PgCharts, event_id};
use crate::store::PgStore;
use crate::verify::{self, EventReport, Finding};

/// SplitMix64: a seeded command sequence, the same on every run. Token hashes
/// are unique across events (`holds.token_hash`), so each event salts its own.
struct Gen(u64, u64);

impl Gen {
    fn token(&self, n: u8) -> TokenHash {
        let mut hash = [n; 32];
        hash[..8].copy_from_slice(&self.1.to_le_bytes());
        TokenHash(hash)
    }
}

impl Gen {
    fn below(&mut self, n: u64) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        (z ^ (z >> 31)) % n
    }

    /// Well-formed commands on 12 contended seats and 4 tokens: every command
    /// kind, holds that expire, bookings with and without details.
    fn command(&mut self) -> Command {
        let n = self.below(4) as u8 + 1;
        let token = self.token(n);
        let i = self.below(12) as u32;
        let objects: Vec<ObjectRef> = (0..=self.below(2) as u32)
            .map(|k| ObjectRef::Idx((i + k) % 12))
            .collect();
        match self.below(10) {
            0 | 1 => Command::CreateHold {
                token,
                ttl_secs: [60, 120, 900][self.below(3) as usize],
                max_objects: [None, Some(2)][self.below(2) as usize],
            },
            2 | 3 => Command::Hold { token, objects },
            4 => Command::Release {
                token,
                objects: if self.below(3) == 0 { vec![] } else { objects },
            },
            5 => Command::ExtendHold {
                token,
                ttl_secs: 120,
            },
            6 if self.below(2) == 0 => Command::Book {
                token: Some(token),
                objects: vec![],
                order_ref: format!("order-{}", self.below(1_000)).into(),
                details: vec![],
            },
            6 => Command::Book {
                token: None,
                details: vec![BookDetail {
                    object: objects[0].clone(),
                    ticket_type: Some("adult".into()),
                    extra: Some(r#"{"row": 1, "note": "é"}"#.into()),
                }],
                objects,
                order_ref: format!("order-{}", self.below(1_000)).into(),
            },
            7 => Command::ReleaseBooked { objects },
            _ => Command::ChangeStatus {
                objects,
                status: Status::from_code([0, 3][self.below(2) as usize]).unwrap(),
            },
        }
    }
}

fn request(command: Command) -> Request {
    Request {
        actor: "key_test".into(),
        authority: Authority::Secret,
        command,
        idempotency: None,
    }
}

async fn event_of(pool: &PgPool, key: &str) -> EventId {
    let hex: String =
        sqlx::query_scalar("SELECT replace(id::text, '-', '') FROM events WHERE key = $1")
            .bind(key)
            .fetch_one(pool)
            .await
            .unwrap();
    event_id(&hex).unwrap()
}

/// Take the event over and load it as the engine does.
async fn open(pool: &PgPool, charts: &PgCharts, event: EventId) -> (PgStore, EventState) {
    let store = PgStore::new(pool.clone(), "verify-test", Duration::from_secs(60));
    let acquired = store.acquire(event).await.unwrap();
    let chart = charts
        .get(acquired.chart_version)
        .await
        .unwrap()
        .view
        .clone();
    let loaded = store.load_event(event).await.unwrap();
    (store, EventState::restore(event, chart, loaded).unwrap())
}

/// Apply `steps` commands from `seed`, time moving on by 0 to 15 s each, and
/// commit every one to three; then leave a live hold behind.
async fn fill(pool: &PgPool, charts: &PgCharts, event: EventId, seed: u64, steps: usize) {
    let (store, mut state) = open(pool, charts, event).await;
    let mut g = Gen(seed, event.0 as u64);
    let mut now = Timestamp(1_800_000_000_000);
    for _ in 0..steps {
        now.0 += g.below(4) * 5_000;
        // Refusals are part of the traffic; they take no `seq`.
        let _reply = state.apply(&request(g.command()), now).unwrap();
        if g.below(3) == 0
            && let Some(batch) = state.take_batch().unwrap()
        {
            store.commit_batch(batch).await.unwrap();
        }
    }
    let token = g.token(9);
    let live = [
        Command::CreateHold {
            token,
            ttl_secs: 900,
            max_objects: Some(3),
        },
        Command::Hold {
            token,
            objects: vec![ObjectRef::Idx(40)],
        },
    ];
    for c in live {
        state.apply(&request(c), now).unwrap().unwrap();
    }
    if let Some(batch) = state.take_batch().unwrap() {
        store.commit_batch(batch).await.unwrap();
    }
}

async fn verify(pool: &PgPool, event: EventId) -> EventReport {
    let charts = PgCharts::new(pool.clone(), 10_000);
    verify::verify_event(pool, &charts, event, false)
        .await
        .unwrap()
}

/// The test database's URL, for `verify::run`.
async fn url(pool: &PgPool) -> String {
    let name: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(pool)
        .await
        .unwrap();
    let server = std::env::var("CAVEA_TEST_DATABASE_URL").unwrap();
    let (base, _) = server.rsplit_once('/').unwrap();
    format!("{base}/{name}")
}

async fn run(args: &[&str], url: &str) -> (u8, String, String) {
    let args: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = verify::run(&args, Some(url), &mut out, &mut err).await;
    (
        code,
        String::from_utf8(out).unwrap(),
        String::from_utf8(err).unwrap(),
    )
}

async fn sql(pool: &PgPool, statement: &'static str, event: EventId) {
    sqlx::query(statement)
        .bind(format!("{:032x}", event.0))
        .execute(pool)
        .await
        .unwrap();
}

async fn scalar_for(pool: &PgPool, query: &'static str, event: EventId) -> i64 {
    sqlx::query_scalar(query)
        .bind(format!("{:032x}", event.0))
        .fetch_one(pool)
        .await
        .unwrap()
}

fn only(report: &EventReport) -> &Finding {
    assert_eq!(report.findings.len(), 1, "{:#?}", report.findings);
    &report.findings[0]
}

/// A row's status changed behind the engine's back: its finding, and
/// `events.counts` no longer the rows' count.
fn with_counts(report: &EventReport) -> &Finding {
    assert_eq!(report.findings.len(), 2, "{:#?}", report.findings);
    let counts = report
        .findings
        .iter()
        .filter(|f| f.check == "counts")
        .count();
    assert_eq!(counts, 1, "{:#?}", report.findings);
    report
        .findings
        .iter()
        .find(|f| f.check != "counts")
        .unwrap()
}

#[tokio::test]
#[ignore = "needs PostgreSQL: set CAVEA_TEST_DATABASE_URL, pass --include-ignored"]
async fn verify_reports_each_corruption_precisely() {
    with_db(|pool| async move {
        let (app, seats) = api_with(&pool, Some(ADMIN), ServerClock::System, "test");
        let t = Tenant::create(&app, "v").await;
        seats.shutdown().await;
        let charts = PgCharts::new(pool.clone(), 10_000);
        // One event per corruption, each written by the same sequence.
        const KINDS: [&str; 23] = [
            "clean", "gap", "repeat", "reorder", "flip", "unbook", "double", "expiry", "limit",
            "unhold", "orphan", "seq", "order", "rehold", "badop", "ghost", "tail", "negseq",
            "zeroseq", "minus", "loose", "detail", "counts",
        ];
        let mut events = Vec::new();
        for kind in KINDS {
            let body = json!({ "key": kind, "chart": t.chart });
            let (response, created) = call(&app, Method::POST, "/v1/events", Some(&t.secret), Some(body)).await;
            assert_eq!(response.status(), 201, "{created}");
            let event = event_of(&pool, kind).await;
            fill(&pool, &charts, event, 7, 400).await;
            events.push(event);
        }
        let e = |kind: &str| events[KINDS.iter().position(|k| *k == kind).unwrap()];

        // Clean, and the sequence reached every command and an expiry.
        let clean = verify(&pool, e("clean")).await;
        assert!(clean.clean && clean.state_compared, "{:#?}", clean.findings);
        assert!(clean.log_records > 100 && clean.object_rows > 0 && clean.hold_rows > 0, "{clean:?}");
        let ops: Vec<i16> = sqlx::query_scalar(
            "SELECT DISTINCT op FROM event_log WHERE event_id = $1::uuid ORDER BY op",
        )
        .bind(format!("{:032x}", e("clean").0))
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(ops, [1, 2, 3, 4, 5, 6, 7]);
        // The tenant's own event, written through the HTTP API, is clean too.
        assert!(verify(&pool, event_of(&pool, &t.event).await).await.clean);

        // A log row deleted: a gap, and the state is not compared.
        sql(&pool, "DELETE FROM event_log WHERE event_id = $1::uuid AND seq = 5", e("gap")).await;
        let r = verify(&pool, e("gap")).await;
        let f = only(&r);
        assert_eq!((f.check, f.invariant, f.seq), ("seq", Some("INV-5"), Some(6)));
        assert_eq!(f.detail.as_deref(), Some("seq 5 missing"));
        assert!(!r.state_compared && r.replayed_to == 4);

        // A log row duplicated.
        sql(
            &pool,
            "INSERT INTO event_log (event_id, seq, at, op, actor, hold_id, order_ref, oids, detail)
             SELECT event_id, seq, at + interval '1 ms', op, actor, hold_id, order_ref, oids, detail
               FROM event_log WHERE event_id = $1::uuid AND seq = 7",
            e("repeat"),
        )
        .await;
        let f = only(&verify(&pool, e("repeat")).await).clone();
        assert_eq!((f.check, f.seq, f.detail.as_deref()), ("seq", Some(7), Some("repeated")));

        // Two rows' seq swapped: a hold used before it is created.
        let first_hold = scalar_for(
            &pool,
            "SELECT min(seq) FROM event_log l WHERE event_id = $1::uuid AND op = 2",
            e("reorder"),
        )
        .await;
        let created = scalar_for(
            &pool,
            "SELECT hold_id FROM event_log WHERE event_id = $1::uuid AND op = 2 ORDER BY seq LIMIT 1",
            e("reorder"),
        )
        .await;
        sqlx::query(
            "UPDATE event_log SET seq = CASE seq WHEN $2 THEN $3 ELSE $2 END
              WHERE event_id = $1::uuid AND seq IN ($2, $3)",
        )
        .bind(format!("{:032x}", e("reorder").0))
        .bind(created)
        .bind(first_hold)
        .execute(&pool)
        .await
        .unwrap();
        let f = only(&verify(&pool, e("reorder")).await).clone();
        assert_eq!((f.check, f.invariant), ("transition", Some("INV-6")));
        assert_eq!(f.seq, Some(created as u64));

        // A booked object's status flipped to blocked.
        let booked = scalar_for(
            &pool,
            "SELECT min(oid)::int8 FROM object_state WHERE event_id = $1::uuid AND status = 2",
            e("flip"),
        )
        .await;
        sql(
            &pool,
            "UPDATE object_state SET status = 3 WHERE event_id = $1::uuid AND oid =
               (SELECT min(oid) FROM object_state WHERE event_id = $1::uuid AND status = 2)",
            e("flip"),
        )
        .await;
        let f = with_counts(&verify(&pool, e("flip")).await).clone();
        assert_eq!((f.check, f.oid, f.field), ("object", Some(booked as u32), Some("status")));
        assert_eq!((f.stored.as_deref(), f.replayed.as_deref()), (Some("blocked"), Some("booked")));
        assert!(f.label.is_some());

        // A booked object freed without a log row.
        sql(
            &pool,
            "DELETE FROM object_state WHERE event_id = $1::uuid AND oid =
               (SELECT min(oid) FROM object_state WHERE event_id = $1::uuid AND status = 2)",
            e("unbook"),
        )
        .await;
        let f = with_counts(&verify(&pool, e("unbook")).await).clone();
        assert_eq!((f.check, f.oid, f.field), ("object", Some(booked as u32), Some("missing")));
        assert_eq!(f.replayed.as_deref(), Some("booked"));

        // A second booking of a booked object, logged as the next command.
        sql(
            &pool,
            "WITH last AS (SELECT seq FROM events WHERE id = $1::uuid),
                  target AS (SELECT min(oid) AS oid FROM object_state
                              WHERE event_id = $1::uuid AND status = 2),
                  logged AS (
                    INSERT INTO event_log (event_id, seq, op, actor, order_ref, oids)
                    SELECT $1::uuid, last.seq + 1, 5, 'key_rogue', 'second-sale', ARRAY[target.oid]
                      FROM last, target)
             UPDATE events SET seq = seq + 1 WHERE id = $1::uuid",
            e("double"),
        )
        .await;
        let r = verify(&pool, e("double")).await;
        let f = only(&r);
        assert_eq!((f.check, f.invariant), ("transition", Some("INV-10")));
        assert_eq!(f.seq, Some(r.log_last_seq));
        assert_eq!(f.detail, Some(format!("books {booked} again without a release")));

        // A hold's expiry, then a hold's limit, changed; a hold row removed.
        sql(
            &pool,
            "UPDATE holds SET expires_at = expires_at + interval '1 minute'
              WHERE event_id = $1::uuid AND id = (SELECT max(id) FROM holds WHERE event_id = $1::uuid)",
            e("expiry"),
        )
        .await;
        let f = only(&verify(&pool, e("expiry")).await).clone();
        assert_eq!((f.check, f.field), ("hold", Some("expires_at")));
        let gap = f.stored.unwrap().parse::<u64>().unwrap() - f.replayed.unwrap().parse::<u64>().unwrap();
        assert_eq!(gap, 60_000);
        sql(
            &pool,
            "UPDATE holds SET max_objects = 7
              WHERE event_id = $1::uuid AND id = (SELECT max(id) FROM holds WHERE event_id = $1::uuid)",
            e("limit"),
        )
        .await;
        let f = only(&verify(&pool, e("limit")).await).clone();
        assert_eq!((f.check, f.field), ("hold", Some("max_objects")));
        assert_eq!((f.stored.as_deref(), f.replayed.as_deref()), (Some("7"), Some("3")));
        sql(
            &pool,
            "DELETE FROM holds
              WHERE event_id = $1::uuid AND id = (SELECT max(id) FROM holds WHERE event_id = $1::uuid)",
            e("unhold"),
        )
        .await;
        let f = only(&verify(&pool, e("unhold")).await).clone();
        assert_eq!((f.check, f.field), ("hold", Some("missing")));

        // A row for a free object; `events.seq` moved.
        let (_, state) = open(&pool, &charts, e("orphan")).await;
        let free = state.chart().oid(100).unwrap();
        sqlx::query("INSERT INTO object_state (event_id, oid, status, seq) VALUES ($1::uuid, $2, 3, 1)")
            .bind(format!("{:032x}", e("orphan").0))
            .bind(free as i32)
            .execute(&pool)
            .await
            .unwrap();
        let f = with_counts(&verify(&pool, e("orphan")).await).clone();
        assert_eq!((f.check, f.oid, f.field), ("object", Some(free), Some("unexpected")));
        assert_eq!(f.stored.as_deref(), Some("blocked"));
        sql(&pool, "UPDATE events SET seq = seq + 1 WHERE id = $1::uuid", e("seq")).await;
        let r = verify(&pool, e("seq")).await;
        let f = only(&r);
        assert_eq!((f.check, f.invariant), ("events_seq", Some("INV-5")));
        assert_eq!(f.stored, Some((r.log_last_seq + 1).to_string()));

        // `events.counts` off by one: the rows are right, the count is not.
        sql(
            &pool,
            "UPDATE events SET counts[2] = counts[2] + 1 WHERE id = $1::uuid",
            e("counts"),
        )
        .await;
        let f = only(&verify(&pool, e("counts")).await).clone();
        assert_eq!(f.check, "counts");
        let (stored, counted) = (f.stored.unwrap(), f.replayed.unwrap());
        assert_ne!(stored, counted);

        // An order reference changed: named, its values withheld unless asked.
        sql(
            &pool,
            "UPDATE object_state SET order_ref = 'tampered' WHERE event_id = $1::uuid AND oid =
               (SELECT min(oid) FROM object_state WHERE event_id = $1::uuid AND status = 2)",
            e("order"),
        )
        .await;
        let f = only(&verify(&pool, e("order")).await).clone();
        assert_eq!((f.field, f.stored.as_deref()), (Some("order_ref"), None));
        let url = url(&pool).await;
        let (code, out, _) = run(&["--workspace", "v", "--event", "order", "--json"], &url).await;
        assert_eq!(code, verify::EXIT_DIFFERENCES);
        assert!(!out.contains("tampered") && !out.contains("order-"), "{out}");
        let (code, out, _) = run(
            &["--workspace", "v", "--event", "order", "--include-booking-data"],
            &url,
        )
        .await;
        assert_eq!(code, verify::EXIT_DIFFERENCES);
        assert!(out.contains("stored tampered log order-"), "{out}");

        // A held object pointed at a hold other than the one the log gave it.
        let (held, holder) = sqlx::query_as::<_, (i32, i64)>(
            "SELECT oid, hold_id FROM object_state
              WHERE event_id = $1::uuid AND status = 1 ORDER BY oid LIMIT 1",
        )
        .bind(format!("{:032x}", e("rehold").0))
        .fetch_one(&pool)
        .await
        .unwrap();
        sql(
            &pool,
            "UPDATE object_state SET hold_id = hold_id + 1000000 WHERE event_id = $1::uuid AND oid =
               (SELECT min(oid) FROM object_state WHERE event_id = $1::uuid AND status = 1)",
            e("rehold"),
        )
        .await;
        let f = only(&verify(&pool, e("rehold")).await).clone();
        assert_eq!((f.check, f.oid, f.field), ("object", Some(held as u32), Some("hold_id")));
        assert_eq!(f.replayed, Some(holder.to_string()));
        assert_eq!(f.stored, Some((holder + 1_000_000).to_string()));

        // A log row with an `op` no version wrote: unreadable, state not compared.
        sql(&pool, "UPDATE event_log SET op = 42 WHERE event_id = $1::uuid AND seq = 3", e("badop")).await;
        let r = verify(&pool, e("badop")).await;
        let f = only(&r);
        assert_eq!((f.check, f.seq, f.detail.as_deref()), ("unreadable_record", Some(3), Some("unknown op 42")));
        assert!(!r.state_compared && r.replayed_to == 2);

        // A rogue row blocking an object the chart does not have: the object is
        // unknown, and the log needs a row the state does not have.
        sql(
            &pool,
            "WITH logged AS (
               INSERT INTO event_log (event_id, seq, op, actor, oids, detail)
               SELECT id, seq + 1, 6, 'key_rogue', ARRAY[7777777], '{\"status\":3}'
                 FROM events WHERE id = $1::uuid)
             UPDATE events SET seq = seq + 1 WHERE id = $1::uuid",
            e("ghost"),
        )
        .await;
        let r = verify(&pool, e("ghost")).await;
        let found: Vec<_> = r.findings.iter().map(|f| (f.check, f.oid, f.field)).collect();
        assert_eq!(found, [("unknown_object", Some(7_777_777), None), ("object", Some(7_777_777), Some("missing"))]);
        assert_eq!(r.findings[0].seq, Some(r.log_last_seq));

        // The log's tail lost (the last batch: the live hold's creation and its
        // hold) while the rows it wrote stay: `events.seq` is ahead, and the
        // hold and its object are unexplained.
        let tail = verify(&pool, e("tail")).await.log_last_seq;
        let live = scalar_for(
            &pool,
            "SELECT oid::int8 FROM object_state WHERE event_id = $1::uuid AND hold_id =
               (SELECT max(id) FROM holds WHERE event_id = $1::uuid)",
            e("tail"),
        )
        .await;
        sql(
            &pool,
            "DELETE FROM event_log WHERE event_id = $1::uuid
                AND seq > (SELECT seq - 2 FROM events WHERE id = $1::uuid)",
            e("tail"),
        )
        .await;
        let r = verify(&pool, e("tail")).await;
        let found: Vec<_> = r.findings.iter().map(|f| (f.check, f.oid, f.hold_id, f.field)).collect();
        assert_eq!(
            found,
            [
                ("events_seq", None, None, None),
                ("object", Some(live as u32), None, Some("unexpected")),
                ("hold", None, Some(tail - 1), Some("unexpected")),
            ]
        );
        assert!(r.state_compared && r.log_last_seq == tail - 2);

        // A row with a negative `seq`, and one with `seq` 0, ahead of the log.
        for (kind, seq, detail) in [("negseq", -2, None), ("zeroseq", 0, Some("out of order: expected 1"))] {
            sqlx::query(
                "INSERT INTO event_log (event_id, seq, at, op, actor, hold_id, order_ref, oids, detail)
                 SELECT event_id, $2, at, op, actor, hold_id, order_ref, oids, detail
                   FROM event_log WHERE event_id = $1::uuid AND seq = 1",
            )
            .bind(format!("{:032x}", e(kind).0))
            .bind(seq)
            .execute(&pool)
            .await
            .unwrap();
            let r = verify(&pool, e(kind)).await;
            let f = only(&r);
            match detail {
                None => assert_eq!((f.check, f.detail.as_deref()), ("unreadable_record", Some("negative seq -2"))),
                Some(d) => assert_eq!((f.check, f.seq, f.detail.as_deref()), ("seq", Some(0), Some(d))),
            }
            assert!(!r.state_compared, "{kind}");
        }

        // `events.seq` negative is reported as it is, not read as 0.
        sql(&pool, "UPDATE events SET seq = -1 WHERE id = $1::uuid", e("minus")).await;
        let f = only(&verify(&pool, e("minus")).await).clone();
        assert_eq!((f.check, f.stored.as_deref()), ("events_seq", Some("-1")));

        // A ticket type on a held object, with no booking to carry it.
        sql(
            &pool,
            "UPDATE object_state SET ticket_type = 'adult' WHERE event_id = $1::uuid AND oid =
               (SELECT min(oid) FROM object_state WHERE event_id = $1::uuid AND status = 1)",
            e("loose"),
        )
        .await;
        let f = only(&verify(&pool, e("loose")).await).clone();
        assert_eq!((f.check, f.field), ("object", Some("booking")));

        // A `detail` that does not decode: reported without the stored value.
        sql(
            &pool,
            "UPDATE event_log SET detail = '{\"expires_at\": \"private-value\"}'
              WHERE event_id = $1::uuid AND seq = (SELECT min(seq) FROM event_log
                                                    WHERE event_id = $1::uuid AND op = 1)",
            e("detail"),
        )
        .await;
        let f = only(&verify(&pool, e("detail")).await).clone();
        assert_eq!(f.check, "unreadable_record");
        let shown = f.detail.unwrap();
        assert!(shown.starts_with("detail: Data error at column") && !shown.contains("private"), "{shown}");

        // Every event: the JSON report's shape and the exit codes.
        let (code, out, err) = run(&["--all", "--json"], &url).await;
        assert_eq!((code, err.as_str()), (verify::EXIT_DIFFERENCES, ""));
        let report: Value = serde_json::from_str(&out).unwrap();
        assert_eq!((report["version"].as_u64(), report["clean"].as_bool()), (Some(1), Some(false)));
        let reports = report["events"].as_array().unwrap();
        assert_eq!(reports.len(), KINDS.len() + 1);
        let clean_count = reports.iter().filter(|r| r["clean"] == true).count();
        assert_eq!(clean_count, 2);
        for r in reports {
            for field in [
                "event_id", "workspace", "event", "chart_version", "log_records", "log_last_seq",
                "replayed_to", "events_seq", "object_rows", "hold_rows", "area_alloc_rows",
                "state_compared", "findings_total", "findings", "not_checked",
            ] {
                assert!(r.get(field).is_some(), "{field} missing from {r}");
            }
        }
        let (code, out, _) = run(&["--workspace", "v", "--event", "clean"], &url).await;
        assert_eq!(code, verify::EXIT_CLEAN, "{out}");
        assert!(out.contains("event v/clean") && out.contains(": clean"), "{out}");
        let (code, _, err) = run(&["--workspace", "v", "--event", "nope"], &url).await;
        assert_eq!((code, err.trim()), (verify::EXIT_COULD_NOT_RUN, "unknown event"));
        assert_eq!(run(&["--event", "clean"], &url).await.0, verify::EXIT_USAGE);
        assert_eq!(run(&["--all", "--bogus"], &url).await.0, verify::EXIT_USAGE);
        let unreachable = "postgres://cavea:cavea@127.0.0.1:1/cavea";
        let (code, _, err) = run(&["--all"], unreachable).await;
        assert_eq!(code, verify::EXIT_COULD_NOT_RUN);
        assert!(!err.contains("cavea:cavea"), "the URL must not be printed: {err}");
    })
    .await;
}

/// Verifying while another connection commits never reports a difference:
/// the log and the rows come from one snapshot.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs PostgreSQL: set CAVEA_TEST_DATABASE_URL, pass --include-ignored"]
async fn verify_while_commits_land() {
    with_db(|pool| async move {
        let (app, seats) = api_with(&pool, Some(ADMIN), ServerClock::System, "test");
        let t = Tenant::create(&app, "w").await;
        seats.shutdown().await;
        let event = event_of(&pool, &t.event).await;
        let writer = tokio::spawn({
            let pool = pool.clone();
            async move {
                let charts = PgCharts::new(pool.clone(), 10_000);
                fill(&pool, &charts, event, 11, 10_000).await;
            }
        });
        let mut runs = 0;
        while !writer.is_finished() {
            let r = verify(&pool, event).await;
            assert!(r.clean, "{:#?}", r.findings);
            runs += 1;
        }
        writer.await.unwrap();
        assert!(verify(&pool, event).await.clean);
        assert!(runs >= 3, "only {runs} verifications overlapped the writer");
    })
    .await;
}
