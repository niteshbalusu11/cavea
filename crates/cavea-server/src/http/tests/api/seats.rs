//! Events, hold tokens and seat operations against PostgreSQL, through the
//! real registry, `PgStore` and chart provider (08 §4.7, §6, §7, §14).

use std::collections::BTreeMap;
use std::time::Duration;

use cavea_compiler::document::{LabelScheme, Node, SeatDirection, SeatLabels};
use cavea_compiler::{ChartDocument, Expansion, write_full_label};
use cavea_engine::{ManualClock, ObjectRef, Timestamp};
use cavea_format::Artefact;

use super::*;
use crate::seats::{PgCharts, event_id};

async fn post(app: &Router, secret: &str, uri: &str, body: Value) -> (Response<Body>, Value) {
    call(app, Method::POST, uri, Some(secret), Some(body)).await
}

const IDEMPOTENCY_KEY: header::HeaderName = header::HeaderName::from_static("idempotency-key");

/// POST a JSON body with an `Idempotency-Key`.
async fn post_keyed(
    app: &Router,
    secret: &str,
    uri: &str,
    body: Value,
    key: &str,
) -> (Response<Body>, Value) {
    let headers = [(IDEMPOTENCY_KEY, key)];
    call_with(app, Method::POST, uri, Some(secret), &headers, Some(body)).await
}

async fn get(app: &Router, secret: &str, uri: &str) -> (Response<Body>, Value) {
    call(app, Method::GET, uri, Some(secret), None).await
}

/// A new hold token on the event.
async fn token(app: &Router, t: &Tenant, body: Value) -> String {
    let mut body = body;
    body["event"] = json!(t.event);
    let (response, created) = post(app, &t.secret, "/v1/hold-tokens", body).await;
    assert_eq!(response.status(), 201, "{created}");
    created["hold_token"].as_str().unwrap().to_owned()
}

fn by_idx(t: &Tenant, idx: &[u32]) -> Value {
    json!({ "version": t.hash, "idx": idx })
}

fn labels(objects: &Value) -> Vec<String> {
    objects
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["label"].as_str().unwrap().to_owned())
        .collect()
}

async fn scalar(pool: &PgPool, sql: &'static str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(pool).await.unwrap()
}

/// Wait, at most five seconds, until `sql` counts zero rows.
async fn until_zero(pool: &PgPool, sql: &'static str) {
    for _ in 0..250 {
        if scalar(pool, sql).await == 0 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("still not zero: {sql}");
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Publish, create an event, hold, refuse a second holder, release by label
/// and by token, extend; every refusal maps to its code with labels; no hold
/// token or its hash is logged.
#[tokio::test]
#[ignore = "needs CAVEA_TEST_DATABASE_URL"]
async fn seat_flow() {
    let _serial = serial();
    let logs = Captured::default();
    let _guard = logs.install();
    let logs_in = logs.clone();
    with_db(async move |pool| {
        let logs = logs_in;
        let (app, seats) = api_with(&pool, Some(ADMIN), ServerClock::System, "test");
        let t = Tenant::create(&app, "a").await;
        let ev = |op: &str| format!("/v1/events/{}/{op}", t.event);

        let body = json!({ "event": t.event, "max_objects": 3 });
        let (response, created) = post(&app, &t.secret, "/v1/hold-tokens", body).await;
        assert_eq!(response.status(), 201, "{created}");
        let first = created["hold_token"].as_str().unwrap().to_owned();
        assert!(first.starts_with("ht_") && first.len() == 67);
        assert_eq!(created["max_objects"], 3);
        assert_eq!(created["event"], t.event.as_str());
        assert!(created["seq"].is_u64() && created["expires_at"].is_string());
        // Only the hash is stored.
        let stored: Vec<u8> =
            sqlx::query_scalar("SELECT token_hash FROM holds ORDER BY id DESC LIMIT 1")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(stored, sha256(&first));
        let second = token(&app, &t, json!({})).await;

        // Hold two seats by idx; the reply names them by label and oid.
        let body = json!({ "hold_token": first, "objects": by_idx(&t, &[5, 6]) });
        let (response, held) = post(&app, &t.secret, &ev("hold"), body).await;
        assert_eq!(response.status(), 200, "{held}");
        let seats_56 = labels(&held["objects"]);
        assert_eq!(seats_56.len(), 2);
        assert!(held["objects"][0]["oid"].is_u64() && held["expires_at"].is_string());
        let (_, read) = get(&app, &t.secret, &format!("/v1/hold-tokens/{first}")).await;
        assert_eq!(read["objects"], held["objects"]);
        assert_eq!(read["max_objects"], 3);
        assert_eq!(read["event"], t.event.as_str());

        // The second token cannot take them: 409 listing labels and oids.
        let body = json!({ "hold_token": second, "objects": seats_56 });
        let (response, refused) = post(&app, &t.secret, &ev("hold"), body).await;
        assert_problem(&response, &refused, Code::ObjectNotFree);
        let expected: Vec<Value> = held["objects"]
            .as_array()
            .unwrap()
            .iter()
            .map(|o| json!({ "label": o["label"], "oid": o["oid"], "reason": "not_free" }))
            .collect();
        assert_eq!(refused["details"]["objects"], json!(expected));

        // Refusals.
        let body = json!({ "hold_token": second, "objects": ["No-Such-Seat"] });
        let (response, p) = post(&app, &t.secret, &ev("hold"), body).await;
        assert_problem(&response, &p, Code::NotFound);
        assert_eq!(
            p["details"]["objects"],
            json!([{ "label": "No-Such-Seat" }])
        );
        let unknown = format!("ht_{}", "0".repeat(64));
        let body = json!({ "hold_token": unknown, "objects": by_idx(&t, &[9]) });
        let (response, p) = post(&app, &t.secret, &ev("hold"), body).await;
        assert_problem(&response, &p, Code::NotFound);
        assert_eq!(p["details"]["missing"], "hold_token");
        for path in [unknown.as_str(), "ht_short", "x"] {
            let (response, p) = get(&app, &t.secret, &format!("/v1/hold-tokens/{path}")).await;
            assert_problem(&response, &p, Code::NotFound);
        }
        let body = json!({ "hold_token": first, "objects": by_idx(&t, &[5, 6, 7, 8]) });
        let (response, p) = post(&app, &t.secret, &ev("hold"), body).await;
        assert_problem(&response, &p, Code::HoldLimitExceeded);
        assert_eq!(
            (&p["details"]["limit"], &p["details"]["requested"]),
            (&json!(3), &json!(4))
        );
        let many: Vec<String> = (0..1_001).map(|i| format!("S-{i}")).collect();
        let body = json!({ "hold_token": second, "objects": many });
        let (response, p) = post(&app, &t.secret, &ev("hold"), body).await;
        assert_problem(&response, &p, Code::InvalidRequest);
        assert_eq!(p["details"]["reason"], "too_many_objects");
        let stale = json!({ "version": "0".repeat(32), "idx": [9] });
        let body = json!({ "hold_token": second, "objects": stale });
        let (response, p) = post(&app, &t.secret, &ev("hold"), body).await;
        assert_problem(&response, &p, Code::VersionConflict);
        assert_eq!(p["details"]["version"], t.hash.as_str());
        let body = json!({ "event": t.event, "max_objects": 1_000_001 });
        let (response, p) = post(&app, &t.secret, "/v1/hold-tokens", body).await;
        assert_problem(&response, &p, Code::InvalidRequest);
        assert_eq!(p["details"]["field"], "max_objects");
        let body = json!({ "event": t.event, "ttl_seconds": 59 });
        let (response, p) = post(&app, &t.secret, "/v1/hold-tokens", body).await;
        assert_problem(&response, &p, Code::InvalidRequest);
        assert_eq!(p["details"]["reason"], "ttl_out_of_range");
        // A publishable key never reaches the engine on these routes.
        let body = json!({ "hold_token": first, "objects": seats_56 });
        let (response, p) = post(&app, &t.publishable, &ev("hold"), body).await;
        assert_problem(&response, &p, Code::Forbidden);
        assert_eq!(p["details"]["reason"], "key_kind");
        let (_, event) = get(&app, &t.secret, &format!("/v1/events/{}", t.event)).await;
        assert_eq!(event["counts"]["held"], 2, "{event}");
        assert_eq!(event["counts"]["free"], 2_000 - 2);

        // Release by label: only the token's own seats.
        let body = json!({ "hold_token": second, "objects": [&seats_56[0]] });
        let (response, p) = post(&app, &t.secret, &ev("release"), body).await;
        assert_problem(&response, &p, Code::ObjectNotHeldByToken);
        assert_eq!(p["details"]["objects"][0]["label"], seats_56[0].as_str());
        let body = json!({ "hold_token": first, "objects": [&seats_56[0]] });
        let (response, released) = post(&app, &t.secret, &ev("release"), body).await;
        assert_eq!(response.status(), 200, "{released}");
        assert_eq!(released["objects"], json!([held["objects"][0]]));
        let body = json!({ "hold_token": second, "objects": [&seats_56[0]] });
        let (response, _) = post(&app, &t.secret, &ev("hold"), body).await;
        assert_eq!(response.status(), 200);
        // An empty list is refused rather than read as "all".
        let body = json!({ "hold_token": first, "objects": [] });
        let (response, p) = post(&app, &t.secret, &ev("release"), body).await;
        assert_problem(&response, &p, Code::InvalidRequest);
        assert_eq!(p["details"]["field"], "objects");
        // Without objects: all of the token's; the token stays valid.
        let body = json!({ "hold_token": first });
        let (response, released) = post(&app, &t.secret, &ev("release"), body).await;
        assert_eq!(
            released["objects"],
            json!([held["objects"][1]]),
            "{released}"
        );
        assert_eq!(response.status(), 200);
        let (_, read) = get(&app, &t.secret, &format!("/v1/hold-tokens/{first}")).await;
        assert_eq!(read["objects"], json!([]));

        // Extend and release the second token.
        let uri = format!("/v1/hold-tokens/{second}/extend");
        let (response, extended) = post(&app, &t.secret, &uri, json!({ "ttl_seconds": 600 })).await;
        assert_eq!(response.status(), 200, "{extended}");
        assert!(extended["expires_at"].is_string());
        let uri = format!("/v1/hold-tokens/{second}");
        let (response, released) = call(&app, Method::DELETE, &uri, Some(&t.secret), None).await;
        assert_eq!(
            (response.status().as_u16(), &released["objects"]),
            (200, &json!([held["objects"][0]]))
        );

        seats.shutdown().await;
        let mut secrets = t.secrets().to_vec();
        let hashes = [hex(&sha256(&first)), hex(&sha256(&second))];
        secrets.extend([first.as_str(), &second, &hashes[0], &hashes[1]]);
        assert_no_secrets(&logs, &secrets);
    })
    .await;
}

/// Book with a token once under an idempotency key (a replay returns the
/// original reply; another body is a mismatch), release booked seats, block,
/// set a custom status and unblock; no order reference or `extra` is logged.
#[tokio::test]
#[ignore = "needs CAVEA_TEST_DATABASE_URL"]
async fn book_and_status() {
    let _serial = serial();
    let logs = Captured::default();
    let _guard = logs.install();
    let logs_in = logs.clone();
    with_db(async move |pool| {
        let logs = logs_in;
        let (app, seats) = api_with(&pool, Some(ADMIN), ServerClock::System, "test");
        let t = Tenant::create(&app, "a").await;
        let ev = |op: &str| format!("/v1/events/{}/{op}", t.event);
        let first = token(&app, &t, json!({})).await;
        let body = json!({ "hold_token": first, "objects": by_idx(&t, &[5, 6]) });
        let (response, held) = post(&app, &t.secret, &ev("hold"), body).await;
        assert_eq!(response.status(), 200, "{held}");
        let seats_56 = labels(&held["objects"]);

        // Booking needs a secret key: a publishable key never reaches the engine.
        let body = json!({ "hold_token": first, "order_ref": "x" });
        let (response, p) = post(&app, &t.publishable, &ev("book"), body).await;
        assert_problem(&response, &p, Code::Forbidden);

        let book = json!({
            "hold_token": first, "order_ref": "ORDREF-secret-77120",
            "ticket_types": { &seats_56[0]: "adult" },
            "extra": { &seats_56[1]: { "note": "EXTRA-secret-text" } },
        });
        let key = "book-77120";
        let (response, booked) = post_keyed(&app, &t.secret, &ev("book"), book.clone(), key).await;
        assert_eq!(response.status(), 200, "{booked}");
        assert_eq!(booked["objects"], held["objects"]);
        let (response, again) = post_keyed(&app, &t.secret, &ev("book"), book.clone(), key).await;
        assert_eq!((response.status().as_u16(), &again), (200, &booked));
        let mut other = book.clone();
        other["order_ref"] = json!("ORDREF-secret-other");
        let (response, p) = post_keyed(&app, &t.secret, &ev("book"), other, key).await;
        assert_problem(&response, &p, Code::IdempotencyMismatch);
        assert_eq!(
            scalar(&pool, "SELECT count(*) FROM event_log WHERE op = 5").await,
            1
        );
        let (ticket, extra): (Option<String>, Option<String>) = sqlx::query_as(
            "SELECT max(ticket_type), max(extra::text) FROM object_state WHERE status = 2",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(ticket.as_deref(), Some("adult"));
        assert_eq!(extra.as_deref(), Some(r#"{"note":"EXTRA-secret-text"}"#));
        let (_, event) = get(&app, &t.secret, &format!("/v1/events/{}", t.event)).await;
        assert_eq!(event["counts"]["booked"], 2, "{event}");
        // Booking its last objects deleted the token.
        let (response, _) = get(&app, &t.secret, &format!("/v1/hold-tokens/{first}")).await;
        assert_eq!(response.status(), 404);

        // Release the booked seats without a token; again is a conflict.
        let body = json!({ "objects": seats_56 });
        let (response, released) = post(&app, &t.secret, &ev("release"), body.clone()).await;
        assert_eq!(response.status(), 200, "{released}");
        assert_eq!(released["objects"], held["objects"]);
        let (response, p) = post(&app, &t.secret, &ev("release"), body).await;
        assert_problem(&response, &p, Code::ObjectStatusConflict);
        assert_eq!(p["details"]["objects"][0]["reason"], "not_booked");
        assert_eq!(p["details"]["objects"][0]["label"], seats_56[0].as_str());

        // Block, free, a custom status; held and booked cannot be set.
        let to = |status: Value| json!({ "objects": [&seats_56[0]], "status": status });
        for status in [json!("blocked"), json!("free"), json!(7)] {
            let (response, changed) = post(&app, &t.secret, &ev("change-status"), to(status)).await;
            assert_eq!(response.status(), 200, "{changed}");
        }
        let (_, event) = get(&app, &t.secret, &format!("/v1/events/{}", t.event)).await;
        assert_eq!(event["counts"]["custom"], json!({ "7": 1 }));
        for (status, reason) in [(json!("held"), "bad_status"), (json!(16), "invalid_field")] {
            let (response, p) = post(&app, &t.secret, &ev("change-status"), to(status)).await;
            assert_problem(&response, &p, Code::InvalidRequest);
            assert_eq!(p["details"]["reason"], reason);
        }

        seats.shutdown().await;
        let mut secrets = t.secrets().to_vec();
        secrets.extend([first.as_str(), "ORDREF-secret", "EXTRA-secret"]);
        assert_no_secrets(&logs, &secrets);
    })
    .await;
}

/// Version pinning, refusals at creation, settings, changes, the list, and
/// archiving.
#[tokio::test]
#[ignore = "needs CAVEA_TEST_DATABASE_URL"]
async fn event_rules() {
    let _serial = serial();
    let logs = Captured::default();
    let _guard = logs.install();
    with_db(async move |pool| {
        let (app, seats) = api_with(&pool, Some(ADMIN), ServerClock::System, "test");
        let t = Tenant::create(&app, "a").await;
        let sk = t.secret.as_str();
        let create = async |body: Value| post(&app, sk, "/v1/events", body).await;

        // A second version: new events pin it by default; old ones keep theirs.
        let uri = format!("/v1/charts/{}/draft", t.chart);
        let request = request(Method::PUT, &uri, Some(sk), &[(header::IF_MATCH, "*")], Some(tiny_document("a2")));
        assert_eq!(send(app.clone(), request).await.0.status(), 200);
        let uri = format!("/v1/charts/{}/publish", t.chart);
        let (response, v2) = call(&app, Method::POST, &uri, Some(sk), None).await;
        assert_eq!(response.status(), 201, "{v2}");
        let (response, e2) = create(json!({ "key": "e2", "chart": t.chart, "name": "Two" })).await;
        assert_eq!(response.status(), 201, "{e2}");
        assert_eq!((&e2["version"]["number"], &e2["version"]["url"]), (&json!(2), &v2["url"]));
        assert_eq!(e2["settings"], json!({ "hold_ttl_seconds": 900, "max_objects": 10 }));
        assert_eq!((&e2["name"], &e2["chart"]), (&json!("Two"), &json!(t.chart)));
        let (_, e1) = create(json!({ "key": "e1", "chart": t.chart, "version": 1 })).await;
        assert_eq!(e1["version"]["number"], 1);
        let (_, original) = get(&app, sk, &format!("/v1/events/{}", t.event)).await;
        assert_eq!(original["version"]["number"], 1);
        assert_eq!(original["counts"]["free"], 2_000);
        // A free object with a row (for its channel) is still free.
        sqlx::query("INSERT INTO object_state (event_id, oid, status, channel, seq) SELECT id, 7, 0, 1, 0 FROM events WHERE key = 'e1'")
            .execute(&pool)
            .await
            .unwrap();
        let (_, e1) = get(&app, sk, "/v1/events/e1").await;
        assert_eq!((&e1["counts"]["free"], &e1["counts"]["custom"]), (&json!(2_000), &json!({})));

        // Refusals.
        let (response, p) = create(json!({ "key": "e3", "chart": t.chart, "version": 9 })).await;
        assert_problem(&response, &p, Code::NotFound);
        assert_eq!(p["details"]["missing"], "version");
        let (response, p) = create(json!({ "key": "e3", "chart": "nope" })).await;
        assert_problem(&response, &p, Code::NotFound);
        assert_eq!(p["details"]["missing"], "chart");
        let (response, p) = create(json!({ "key": "e2", "chart": t.chart })).await;
        assert_problem(&response, &p, Code::AlreadyExists);
        let body = json!({ "key": "draft-only", "name": "d" });
        let (response, _) = post(&app, sk, "/v1/charts", body).await;
        assert_eq!(response.status(), 201);
        let (response, p) = create(json!({ "key": "e3", "chart": "draft-only" })).await;
        assert_problem(&response, &p, Code::InvalidRequest);
        assert_eq!(p["details"]["reason"], "unpublished");
        for (settings, field) in [
            (json!({ "hold_ttl_seconds": 59 }), "settings.hold_ttl_seconds"),
            (json!({ "hold_ttl_seconds": 7_201 }), "settings.hold_ttl_seconds"),
            (json!({ "max_objects": 0 }), "settings.max_objects"),
            (json!({ "max_objects": 101 }), "settings.max_objects"),
        ] {
            let (response, p) = create(json!({ "key": "e3", "chart": t.chart, "settings": settings })).await;
            assert_problem(&response, &p, Code::InvalidRequest);
            assert_eq!(p["details"]["field"], field);
        }
        let body = json!({ "key": "e3", "chart": t.chart, "settings": { "orphans": true } });
        let (response, p) = create(body).await;
        assert_problem(&response, &p, Code::InvalidRequest);

        // Settings: the hold lifetime is the tokens' default; PATCH replaces.
        let body = json!({ "key": "e3", "chart": t.chart, "settings": { "hold_ttl_seconds": 120 } });
        let (response, e3) = create(body).await;
        assert_eq!(response.status(), 201, "{e3}");
        assert_eq!(e3["settings"], json!({ "hold_ttl_seconds": 120, "max_objects": 10 }));
        let body = json!({ "event": "e3" });
        let (response, _) = post(&app, sk, "/v1/hold-tokens", body).await;
        assert_eq!(response.status(), 201);
        let ttl = scalar(&pool, "SELECT extract(epoch FROM max(expires_at) - now())::int8 FROM holds h JOIN events e ON e.id = h.event_id WHERE e.key = 'e3'").await;
        assert!((110..=120).contains(&ttl), "{ttl}");
        let body = json!({ "name": "Three", "settings": { "max_objects": 5 } });
        let (response, patched) = call(&app, Method::PATCH, "/v1/events/e3", Some(sk), Some(body)).await;
        assert_eq!(response.status(), 200, "{patched}");
        assert_eq!(patched["settings"], json!({ "hold_ttl_seconds": 900, "max_objects": 5 }));
        assert_eq!(patched["name"], "Three");
        let body = json!({ "settings": {} });
        let (_, kept) = call(&app, Method::PATCH, "/v1/events/e3", Some(sk), Some(body)).await;
        assert_eq!(kept["name"], "Three");
        let body = json!({ "name": null });
        let (_, cleared) = call(&app, Method::PATCH, "/v1/events/e3", Some(sk), Some(body)).await;
        assert_eq!(cleared["name"], Value::Null);

        // The list, a page at a time.
        let (_, page) = get(&app, sk, "/v1/events?limit=2").await;
        assert_eq!(keys_of(&page), [t.event.as_str(), "e2"]);
        let (_, page) = get(&app, sk, "/v1/events?limit=2&after=e2").await;
        assert_eq!(keys_of(&page), ["e1", "e3"]);
        assert_eq!(page["next"], Value::Null);
        assert!(page["data"][0].get("counts").is_none());

        // An archived event is read-only; its holds stay until they expire.
        let body = json!({ "hold_token": t.token, "objects": by_idx(&t, &[0]) });
        let uri = format!("/v1/events/{}/hold", t.event);
        let (response, held) = post(&app, sk, &uri, body).await;
        assert_eq!(response.status(), 200, "{held}");
        let uri = format!("/v1/events/{}", t.event);
        let (response, archived) = call(&app, Method::DELETE, &uri, Some(sk), None).await;
        assert_eq!(response.status(), 200, "{archived}");
        assert!(archived["archived_at"].is_string());
        let (_, again) = call(&app, Method::DELETE, &uri, Some(sk), None).await;
        assert_eq!(again["archived_at"], archived["archived_at"]);
        let refused = [
            (format!("{uri}/hold"), json!({ "hold_token": t.token, "objects": by_idx(&t, &[1]) })),
            (format!("{uri}/release"), json!({ "hold_token": t.token })),
            ("/v1/hold-tokens".to_owned(), json!({ "event": t.event })),
            (format!("/v1/hold-tokens/{}/extend", t.token), json!({})),
        ];
        for (uri, body) in refused {
            let (response, p) = post(&app, sk, &uri, body).await;
            assert_problem(&response, &p, Code::InvalidRequest);
            assert_eq!(p["details"]["reason"], "archived", "{uri}");
        }
        let (response, p) = call(&app, Method::PATCH, &uri, Some(sk), Some(json!({ "name": "x" }))).await;
        assert_problem(&response, &p, Code::InvalidRequest);
        let (_, token) = get(&app, sk, &format!("/v1/hold-tokens/{}", t.token)).await;
        assert_eq!(token["objects"], held["objects"]);
        let (response, p) = create(json!({ "key": t.event, "chart": t.chart })).await;
        assert_problem(&response, &p, Code::AlreadyExists);

        // An archived chart takes no new events; its events keep working.
        let (response, _) = call(&app, Method::DELETE, &format!("/v1/charts/{}", t.chart), Some(sk), None).await;
        assert_eq!(response.status(), 200);
        let (response, p) = create(json!({ "key": "e4", "chart": t.chart })).await;
        assert_problem(&response, &p, Code::InvalidRequest);
        assert_eq!(p["details"]["reason"], "archived");
        let held_e2 = json!({ "event": "e2" });
        let (response, created) = post(&app, sk, "/v1/hold-tokens", held_e2).await;
        assert_eq!(response.status(), 201, "{created}");
        let body = json!({ "hold_token": created["hold_token"], "objects": { "version": e2["version"]["hash"], "idx": [0] } });
        let (response, held) = post(&app, sk, "/v1/events/e2/hold", body).await;
        assert_eq!(response.status(), 200, "{held}");
        seats.shutdown().await;
    })
    .await;
}

fn keys_of(page: &Value) -> Vec<&str> {
    page["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["key"].as_str().unwrap())
        .collect()
}

fn now_ms() -> u64 {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap()
}

/// A token of the minimum lifetime expires and frees its seats; shutdown
/// releases every lease, so another node acquires at once; after a restart an
/// event with holds is loaded at start-up and its hold expires with no request,
/// and an idempotent booking still replays its original reply.
#[tokio::test]
#[ignore = "needs CAVEA_TEST_DATABASE_URL"]
async fn expiry_restart_and_shutdown() {
    let _serial = serial();
    let logs = Captured::default();
    let _guard = logs.install();
    with_db(async move |pool| {
        let clock = ManualClock::new(Timestamp(now_ms()));
        let manual = ServerClock::Manual(clock.clone());
        let (app, seats) = api_with(&pool, Some(ADMIN), manual.clone(), "node-1");
        let t = Tenant::create(&app, "a").await;
        let sk = t.secret.as_str();
        let ev = |op: &str| format!("/v1/events/{}/{op}", t.event);

        let short = token(&app, &t, json!({ "ttl_seconds": 60 })).await;
        let body = json!({ "hold_token": short, "objects": by_idx(&t, &[0, 1]) });
        let (response, _) = post(&app, sk, &ev("hold"), body).await;
        assert_eq!(response.status(), 200);

        let book = json!({ "objects": by_idx(&t, &[10]), "order_ref": "R-1" });
        let (response, booked) = post_keyed(&app, sk, &ev("book"), book.clone(), "restart").await;
        assert_eq!(response.status(), 200, "{booked}");

        clock.advance_ms(60_000);
        until_zero(&pool, "SELECT count(*) FROM object_state WHERE status = 1").await;
        let (response, _) = get(&app, sk, &format!("/v1/hold-tokens/{short}")).await;
        assert_eq!(response.status(), 404);
        let expired = scalar(
            &pool,
            "SELECT count(*) FROM event_log WHERE op = 7 AND actor = 'system'",
        )
        .await;
        assert_eq!(expired, 1);

        let long = token(&app, &t, json!({ "ttl_seconds": 600 })).await;
        let body = json!({ "hold_token": long, "objects": by_idx(&t, &[3]) });
        let (response, held) = post(&app, sk, &ev("hold"), body).await;
        assert_eq!(response.status(), 200);

        // Shutdown releases every lease: another node takes the event at once.
        seats.shutdown().await;
        assert_eq!(
            scalar(
                &pool,
                "SELECT count(*) FROM events WHERE lease_until IS NOT NULL"
            )
            .await,
            0
        );
        let (app, seats) = api_with(&pool, Some(ADMIN), manual, "node-2");
        seats.preload().await.unwrap();
        assert_eq!(seats.registry.loaded(), 1);
        let owner: String = sqlx::query_scalar("SELECT owner_node FROM events")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(owner, "node-2");
        let (_, read) = get(&app, sk, &format!("/v1/hold-tokens/{long}")).await;
        assert_eq!(read["objects"], held["objects"]);
        let (response, again) = post_keyed(&app, sk, &ev("book"), book, "restart").await;
        assert_eq!((response.status().as_u16(), &again), (200, &booked));

        // The preloaded actor expires the hold with no request arriving.
        clock.advance_ms(600_000);
        until_zero(&pool, "SELECT count(*) FROM object_state WHERE status = 1").await;
        seats.shutdown().await;
    })
    .await;
}

/// Every seat's full label, from the document through the compiler's
/// expansion: an independent path to what the provider decodes.
fn expected_labels(doc: &ChartDocument) -> BTreeMap<u32, String> {
    let mut out = BTreeMap::new();
    let mut visit = |node: &Node, section: &str| {
        let mut e = Expansion::new();
        match node {
            Node::RowBlock(b) if !b.hidden => {
                e.push_row_block(b, doc.settings.alpha_overflow).unwrap();
            }
            Node::Row(r) if !r.hidden => {
                e.push_row(r, doc.settings.alpha_overflow).unwrap();
            }
            _ => return,
        }
        for s in e.seats.iter().filter(|s| !s.removed) {
            let row = &e.rows[s.row as usize];
            let mut seat = String::new();
            row.write_seat_label(s, &mut seat).unwrap();
            let mut full = String::new();
            let template = &doc.settings.label_template;
            write_full_label(template, section, row.label.as_str(), &seat, &mut full).unwrap();
            assert!(out.insert(s.oid, full).is_none());
        }
    };
    for node in &doc.nodes {
        match node {
            Node::Section(s) if !s.hidden => {
                for child in &s.children {
                    visit(child, s.label.as_deref().unwrap_or(""));
                }
            }
            other => visit(other, ""),
        }
    }
    out
}

async fn version_id(pool: &PgPool, chart: &str) -> cavea_engine::ChartVersionId {
    let hex: String = sqlx::query_scalar(
        "SELECT replace(v.id::text, '-', '') FROM chart_versions v
           JOIN charts c ON c.id = v.chart_id WHERE c.key = $1",
    )
    .bind(chart)
    .fetch_one(pool)
    .await
    .unwrap();
    cavea_engine::ChartVersionId(event_id(&hex).unwrap().0)
}

/// Labels and oids agree with the compiler for every object; the cache is
/// bounded; a corrupt or hostile stored artefact is an error, never a panic,
/// and a seat command on it is `503 unavailable`.
#[tokio::test]
#[ignore = "needs CAVEA_TEST_DATABASE_URL"]
async fn chart_provider() {
    let _serial = serial();
    let logs = Captured::default();
    let _guard = logs.install();
    let logs_in = logs.clone();
    with_db(async move |pool| {
        let logs = logs_in;
        let (app, seats) = api_with(&pool, Some(ADMIN), ServerClock::System, "test");
        let a = Tenant::create(&app, "a").await;
        let b = Tenant::create(&app, "b").await;
        let (va, vb) = (
            version_id(&pool, &a.chart).await,
            version_id(&pool, &b.chart).await,
        );

        // The default rules, and a chart with custom, right-to-left and
        // centre-out continuous seat labels.
        let mut doc = cavea_testkit::synthetic::stadium(2_000, 1);
        doc.name = "labels".into();
        for (n, node) in doc.nodes.iter_mut().enumerate().take(3) {
            let Node::Section(section) = node else {
                continue;
            };
            let Some(Node::RowBlock(block)) = section.children.first_mut() else {
                continue;
            };
            block.seat_labels = match n {
                0 => SeatLabels {
                    scheme: LabelScheme::Custom,
                    custom: (1..=500).map(|i| format!("c{i}")).collect(),
                    prefix: "P".into(),
                    ..SeatLabels::default()
                },
                1 => SeatLabels {
                    scheme: LabelScheme::Alpha,
                    direction: SeatDirection::Rtl,
                    ..SeatLabels::default()
                },
                _ => SeatLabels {
                    direction: SeatDirection::CenterOut,
                    continuous: true,
                    suffix: "s".into(),
                    ..SeatLabels::default()
                },
            };
        }
        let body = json!({ "key": "labels", "name": "labels",
            "document": serde_json::from_slice::<Value>(&doc.to_json().unwrap()).unwrap() });
        let (response, created) = post(&app, &a.secret, "/v1/charts", body).await;
        assert_eq!(response.status(), 201, "{created}");
        let uri = "/v1/charts/labels/publish";
        let (response, v) = call(&app, Method::POST, uri, Some(&a.secret), None).await;
        assert_eq!(response.status(), 201, "{v}");
        let vl = version_id(&pool, "labels").await;
        let charts = PgCharts::new(pool.clone(), 2_000);
        let default = cavea_testkit::synthetic::stadium(2_000, 1);
        for (version, doc) in [(va, &default), (vl, &doc)] {
            let expected = expected_labels(doc);
            let chart = charts.get(version).await.unwrap();
            assert_eq!(chart.view.len(), expected.len());
            for (oid, label) in &expected {
                let idx = chart.view.resolve(&ObjectRef::Label(label.as_str().into()));
                let idx = idx.unwrap_or_else(|| panic!("{label} not found"));
                assert_eq!(
                    (chart.view.oid(idx), chart.label(idx)),
                    (Some(*oid), Some(label.as_str()))
                );
            }
        }
        assert!(expected_labels(&doc).values().any(|l| l.ends_with("-Pc1")));
        // Bounded: one 2,000-object chart fits; the older one goes.
        charts.get(vb).await.unwrap();
        assert_eq!(charts.cached(), 1);
        assert!(Arc::ptr_eq(
            &charts.get(vb).await.unwrap(),
            &charts.get(vb).await.unwrap()
        ));

        // Hostile artefacts: garbage, and a valid file with a duplicate oid.
        let full: Vec<u8> =
            sqlx::query_scalar("SELECT full_ccf FROM chart_versions WHERE id = $1::uuid")
                .bind(format!("{:032x}", vb.0))
                .fetch_one(&pool)
                .await
                .unwrap();
        let mut decoded = cavea_format::decode(&full, Artefact::Full).unwrap();
        let oids = &mut decoded.full.as_mut().unwrap().oids;
        oids[1] = oids[0];
        let duplicate = cavea_format::encode(&decoded, Artefact::Full).unwrap();
        for bytes in [vec![0u8; 40], full[..full.len() / 2].to_vec(), duplicate] {
            sqlx::query("UPDATE chart_versions SET full_ccf = $2 WHERE id = $1::uuid")
                .bind(format!("{:032x}", vb.0))
                .bind(&bytes)
                .execute(&pool)
                .await
                .unwrap();
            let fresh = PgCharts::new(pool.clone(), 2_000);
            assert!(fresh.get(vb).await.is_err());
        }
        // Through the API, from a node that has not loaded b's event yet.
        seats.shutdown().await;
        let (app, seats) = api_with(&pool, Some(ADMIN), ServerClock::System, "test");
        let body = json!({ "hold_token": b.token, "objects": by_idx(&b, &[0]) });
        let uri = format!("/v1/events/{}/hold", b.event);
        let (response, p) = post(&app, &b.secret, &uri, body).await;
        assert_problem(&response, &p, Code::Unavailable);
        assert!(logs.text().contains("stored chart version is unreadable"));
        seats.shutdown().await;
    })
    .await;
}
