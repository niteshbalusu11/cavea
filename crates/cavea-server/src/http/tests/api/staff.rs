//! What the console's manager and the designer need of the server, against
//! PostgreSQL: the staff state (`GET /v1/events/{event}/state`) against the
//! committed rows, the events' counts, the events list's search, filters and
//! order, a publishable key's origins, the chart editing lock, versions'
//! thumbnails and authors, and each chart's count of events.

use cavea_engine::{ManualClock, Timestamp};
use cavea_format::wire;

use super::*;

async fn get(app: &Router, secret: &str, uri: &str) -> (Response<Body>, Value) {
    call(app, Method::GET, uri, Some(secret), None).await
}

async fn post(app: &Router, secret: &str, uri: &str, body: Value) -> (Response<Body>, Value) {
    call(app, Method::POST, uri, Some(secret), Some(body)).await
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

/// A staff client applying manager frames, as the console would.
#[derive(Default)]
struct Staff {
    seq: u64,
    codes: Vec<u8>,
    snapshots: usize,
    deltas: usize,
}

impl Staff {
    /// Poll from its `seq`; `true` if a frame came.
    async fn poll(&mut self, app: &Router, t: &Tenant) -> bool {
        let uri = format!("/v1/events/{}/state?after_seq={}", t.event, self.seq);
        let ((response, _), bytes) = send_raw(
            app.clone(),
            request(Method::GET, &uri, Some(&t.secret), &[], None),
        )
        .await;
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        if response.status() == 204 {
            assert!(bytes.is_empty());
            return false;
        }
        assert_eq!(response.status(), 200);
        self.apply(&bytes);
        true
    }

    fn apply(&mut self, bytes: &[u8]) {
        // A manager decoder refuses encoding 2 and codes above 15.
        match wire::decode_server(bytes, wire::View::Manager).unwrap() {
            wire::ServerFrame::Snapshot(s) => {
                self.codes = vec![0; s.objects as usize];
                s.write_into(&mut self.codes).unwrap();
                self.seq = s.seq;
                self.snapshots += 1;
            }
            wire::ServerFrame::Delta(d) => {
                assert_eq!(d.from_seq, self.seq, "a delta from another seq");
                for c in d.changes(self.codes.len() as u32).unwrap() {
                    self.codes[c.idx as usize] = c.code;
                }
                self.seq = d.to_seq;
                self.deltas += 1;
            }
            other => panic!("not a state frame: {other:?}"),
        }
    }
}

/// The domain code of an object as the objects query shows it.
fn code(object: &Value) -> u8 {
    match &object["status"] {
        Value::String(s) => match s.as_str() {
            "free" => 0,
            "held" => 1,
            "booked" => 2,
            "blocked" => 3,
            other => panic!("{other}"),
        },
        n => u8::try_from(n.as_u64().unwrap()).unwrap(),
    }
}

/// Every object's status by the objects query, in `idx` order.
async fn committed(app: &Router, t: &Tenant) -> Vec<u8> {
    let mut codes = Vec::new();
    let mut after = String::new();
    loop {
        let uri = format!("/v1/events/{}/objects?limit=100{after}", t.event);
        let (response, page) = get(app, &t.secret, &uri).await;
        assert_eq!(response.status(), 200, "{page}");
        codes.extend(page["data"].as_array().unwrap().iter().map(code));
        match page["next"].as_str() {
            Some(next) => after = format!("&after={next}"),
            None => return codes,
        }
    }
}

/// `{free, held, booked, blocked, custom}` of a list of codes.
fn tally(codes: &[u8]) -> Value {
    let n = |c: u8| codes.iter().filter(|&&x| x == c).count();
    let custom: serde_json::Map<String, Value> = (4..=15)
        .filter(|&c| n(c) > 0)
        .map(|c| (c.to_string(), n(c).into()))
        .collect();
    json!({ "free": n(0), "held": n(1), "booked": n(2), "blocked": n(3), "custom": custom })
}

/// A small deterministic generator.
struct Rng(u64);

impl Rng {
    fn below(&mut self, n: u64) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 % n
    }
}

/// The staff state is the committed state: a seeded run of every seat
/// command, hold expiries included, applied by a client that polls from its
/// `seq` (deltas, merged when several are buffered) reproduces the objects
/// query at every check, and the event's counts (detail and list) are its
/// tally. A `seq` the deltas no longer reach gets the snapshot; the current
/// one `204`. The public state of the same moment is the reduction to 0 and
/// 1: held, booked, blocked and custom look alike there.
#[tokio::test]
#[ignore = "needs CAVEA_TEST_DATABASE_URL"]
async fn staff_state_is_the_committed_state() {
    let _serial = serial();
    with_db(async move |pool| {
        let clock = ManualClock::new(Timestamp(now_ms()));
        let (app, _seats) = api_with(
            &pool,
            Some(ADMIN),
            ServerClock::Manual(clock.clone()),
            "test",
        );
        let t = Tenant::create(&app, "a").await;
        let sk = t.secret.as_str();
        let ev = |op: &str| format!("/v1/events/{}/{op}", t.event);
        let objects = json!({ "version": t.hash });
        let by_idx = |idx: &[u64]| {
            let mut o = objects.clone();
            o["idx"] = json!(idx);
            o
        };
        let new_token = async |ttl: u64| {
            let body = json!({ "event": t.event, "ttl_seconds": ttl });
            let (response, created) = post(&app, sk, "/v1/hold-tokens", body).await;
            assert_eq!(response.status(), 201, "{created}");
            created["hold_token"].as_str().unwrap().to_owned()
        };
        // Publication waits on the engine's clock: move it on, then read
        // until the state reaches the committed `seq`.
        let sync = async |staff: &mut Staff| {
            for _ in 0..200 {
                clock.advance_ms(100);
                let (_, event) = get(&app, sk, &format!("/v1/events/{}", t.event)).await;
                staff.poll(&app, &t).await;
                if staff.seq == event["seq"].as_u64().unwrap() {
                    return event;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            panic!("the staff state never reached the committed seq");
        };

        let mut staff = Staff::default();
        sync(&mut staff).await;
        assert_eq!(staff.snapshots, 1);
        let first_seq = staff.seq;
        let mut rng = Rng(0x5eed_cafe);
        let mut tokens = vec![new_token(60).await, new_token(600).await];
        let mut checks = 0;
        let mut seen = std::collections::BTreeSet::new();
        for step in 0..240 {
            let pick = |rng: &mut Rng, n: u64| -> Vec<u64> {
                let mut v: Vec<u64> = (0..n).map(|_| rng.below(60)).collect();
                v.sort_unstable();
                v.dedup();
                v
            };
            let token = tokens[rng.below(tokens.len() as u64) as usize].clone();
            let (op, body) = match rng.below(9) {
                0 | 1 => ("hold", json!({ "hold_token": token, "objects": by_idx(&pick(&mut rng, 3)) })),
                2 => ("release", json!({ "hold_token": token })),
                3 => ("book", json!({ "objects": by_idx(&pick(&mut rng, 2)), "order_ref": format!("o-{step}") })),
                4 => ("book", json!({ "hold_token": token, "order_ref": format!("t-{step}") })),
                5 => ("change-status", json!({ "objects": by_idx(&pick(&mut rng, 2)), "status": "blocked" })),
                6 => ("change-status", json!({ "objects": by_idx(&pick(&mut rng, 2)), "status": 5 })),
                7 => ("change-status", json!({ "objects": by_idx(&pick(&mut rng, 3)), "status": "free" })),
                _ => ("release", json!({ "objects": by_idx(&pick(&mut rng, 2)) })),
            };
            let (response, reply) = post(&app, sk, &ev(op), body).await;
            // Refusals (not free, not held, nothing to book) change nothing.
            assert!(
                response.status().is_success() || response.status().is_client_error(),
                "{op}: {reply}"
            );
            if step % 40 == 39 {
                // The short token expires: its seats go back.
                clock.advance_ms(61_000);
                tokens = vec![new_token(60).await, tokens[1].clone()];
            }
            if step % 30 == 29 {
                let event = sync(&mut staff).await;
                assert_eq!(staff.codes, committed(&app, &t).await, "step {step}");
                assert_eq!(event["counts"], tally(&staff.codes), "step {step}");
                let uri = format!("/v1/events?q={}", t.event);
                let (_, list) = get(&app, sk, &uri).await;
                assert_eq!(list["data"][0]["counts"], event["counts"]);
                assert_eq!(list["data"][0]["seq"], event["seq"]);
                // The public view of the same moment.
                let uri = format!("/v1/public/events/{}/state?key={}", t.event, t.publishable);
                let ((_, _), bytes) =
                    send_raw(app.clone(), request(Method::GET, &uri, None, &[], None)).await;
                let wire::ServerFrame::Snapshot(public) =
                    wire::decode_server(&bytes, wire::View::Public).unwrap()
                else {
                    panic!("not a snapshot");
                };
                let mut codes = vec![0; staff.codes.len()];
                public.write_into(&mut codes).unwrap();
                if public.seq == staff.seq {
                    let reduced: Vec<u8> = staff.codes.iter().map(|&c| u8::from(c != 0)).collect();
                    assert_eq!(codes, reduced);
                }
                seen.extend(staff.codes.iter().copied());
                checks += 1;
            }
        }
        assert_eq!(checks, 8);
        assert!(staff.deltas >= checks, "{}", staff.deltas);
        assert_eq!(seen, [0, 1, 2, 3, 5].into(), "every status reached at a check");

        // Nothing new after the current seq.
        assert!(!staff.poll(&app, &t).await);
        // A seq the deltas no longer reach (over 60 s ago): the snapshot.
        clock.advance_ms(61_000);
        let body = json!({ "objects": by_idx(&[1_500]), "status": "blocked" });
        let (response, _) = post(&app, sk, &ev("change-status"), body).await;
        assert_eq!(response.status(), 200);
        sync(&mut staff).await;
        let mut old = Staff {
            seq: first_seq,
            codes: vec![0; staff.codes.len()],
            ..Staff::default()
        };
        assert!(old.poll(&app, &t).await);
        assert_eq!((old.snapshots, old.deltas), (1, 0));
        assert_eq!((old.seq, &old.codes), (staff.seq, &staff.codes));
        // From 0: the snapshot.
        let mut fresh = Staff::default();
        assert!(fresh.poll(&app, &t).await);
        assert_eq!((fresh.snapshots, &fresh.codes), (1, &staff.codes));
        // A malformed `after_seq` is a query error; another workspace's
        // event is unknown (the matrix covers every credential).
        let uri = format!("/v1/events/{}/state?after_seq=x", t.event);
        let (response, problem) = get(&app, sk, &uri).await;
        assert_problem(&response, &problem, Code::InvalidRequest);
    })
    .await;
}

/// `GET /v1/events`: `q` matches key or name, ignoring case, every character
/// literally; `archived` keeps one side; `order=newest` is the reverse of the
/// default; pages of any size, filters and order together, visit each match
/// once; each row carries its counts.
#[tokio::test]
#[ignore = "needs CAVEA_TEST_DATABASE_URL"]
async fn events_list_searches_and_orders() {
    let _serial = serial();
    with_db(async move |pool| {
        let app = api(&pool, Some(ADMIN));
        let t = Tenant::create(&app, "a").await;
        let sk = t.secret.as_str();
        let made = [
            ("hamlet-1", Some("Hamlet, matinee")),
            ("lear", Some("King Lear")),
            ("HAMLET-2", None),
            ("pct", Some("50%_off hamlet")),
            ("other", Some("Othello")),
        ];
        for (key, name) in made {
            let body = json!({ "key": key, "chart": t.chart, "name": name });
            let (response, created) = post(&app, sk, "/v1/events", body).await;
            assert_eq!(response.status(), 201, "{created}");
        }
        let (response, _) = call(&app, Method::DELETE, "/v1/events/lear", Some(sk), None).await;
        assert_eq!(response.status(), 200);
        let (response, _) = call(&app, Method::DELETE, "/v1/events/pct", Some(sk), None).await;
        assert_eq!(response.status(), 200);
        // Counts on rows: one booked seat in `other`.
        let body = json!({ "objects": { "version": t.hash, "idx": [4] }, "order_ref": "x" });
        let (response, _) = post(&app, sk, "/v1/events/other/book", body).await;
        assert_eq!(response.status(), 200);

        let walk = async |query: &str, limit: u32| -> Vec<String> {
            let mut keys = Vec::new();
            let mut after = String::new();
            loop {
                let uri = format!("/v1/events?limit={limit}{query}{after}");
                let (response, page) = get(&app, sk, &uri).await;
                assert_eq!(response.status(), 200, "{uri}: {page}");
                for e in page["data"].as_array().unwrap() {
                    keys.push(e["key"].as_str().unwrap().to_owned());
                }
                match page["next"].as_str() {
                    Some(next) => after = format!("&after={next}"),
                    None => return keys,
                }
            }
        };
        let all = [
            t.event.as_str(),
            "hamlet-1",
            "lear",
            "HAMLET-2",
            "pct",
            "other",
        ];
        for limit in [1, 2, 100] {
            assert_eq!(walk("", limit).await, all);
            let newest: Vec<&str> = all.iter().rev().copied().collect();
            assert_eq!(walk("&order=newest", limit).await, newest);
            assert_eq!(
                walk("&q=hamlet", limit).await,
                ["hamlet-1", "HAMLET-2", "pct"]
            );
            assert_eq!(
                walk("&q=HaMlEt&order=newest", limit).await,
                ["pct", "HAMLET-2", "hamlet-1"]
            );
            assert_eq!(
                walk("&q=hamlet&archived=false", limit).await,
                ["hamlet-1", "HAMLET-2"]
            );
            assert_eq!(walk("&archived=true", limit).await, ["lear", "pct"]);
            let chart = format!("&chart={}&q=e&order=newest", t.chart);
            assert_eq!(
                walk(&chart, limit).await,
                [
                    "other",
                    "pct",
                    "HAMLET-2",
                    "lear",
                    "hamlet-1",
                    t.event.as_str()
                ]
            );
        }
        // `%` and `_` are characters, not wildcards.
        assert_eq!(walk("&q=%25_", 100).await, ["pct"]);
        assert_eq!(walk("&q=_", 100).await, ["pct"]);
        assert_eq!(walk("&q=h%25t", 100).await, Vec::<String>::new());
        // An empty `q` is no filter; a long one is refused.
        assert_eq!(walk("&q=", 100).await, all);
        let uri = format!("/v1/events?q={}", "x".repeat(129));
        let (response, problem) = get(&app, sk, &uri).await;
        assert_problem(&response, &problem, Code::InvalidRequest);
        assert_eq!(problem["details"]["reason"], "query");
        let (response, problem) = get(&app, sk, "/v1/events?order=sideways").await;
        assert_problem(&response, &problem, Code::InvalidRequest);

        let (_, page) = get(&app, sk, "/v1/events?q=othello").await;
        let row = &page["data"][0];
        assert_eq!(row["counts"]["booked"], 1);
        assert_eq!(row["counts"]["free"], 1_999);
        assert_eq!(row["seq"], 1);
    })
    .await;
}

/// Every chart reply counts the workspace's events on any of the chart's
/// versions, active and archived apart, following creation, archiving and a
/// row deleted by hand; another workspace's events under the same chart key
/// are never counted.
#[tokio::test]
#[ignore = "needs CAVEA_TEST_DATABASE_URL"]
async fn charts_count_their_events() {
    let _serial = serial();
    with_db(async move |pool| {
        let app = api(&pool, Some(ADMIN));
        let a = Tenant::create_as(&app, "a", "x").await;
        let b = Tenant::create_as(&app, "b", "x").await;
        assert_eq!(a.chart, b.chart);
        let sk = a.secret.as_str();
        let counts = async |secret: &str| -> (Value, Value) {
            let (response, page) = get(&app, secret, "/v1/charts?limit=1").await;
            assert_eq!(response.status(), 200, "{page}");
            let (response, detail) = get(&app, secret, &format!("/v1/charts/{}", a.chart)).await;
            assert_eq!(response.status(), 200, "{detail}");
            assert_eq!(page["data"][0]["events"], detail["events"]);
            (
                detail["events"]["active"].clone(),
                detail["events"]["archived"].clone(),
            )
        };
        assert_eq!(counts(sk).await, (json!(1), json!(0)));

        // A chart without events, as created.
        let (response, empty) =
            post(&app, sk, "/v1/charts", json!({ "key": "e", "name": "E" })).await;
        assert_eq!(response.status(), 201);
        assert_eq!(empty["events"], json!({ "active": 0, "archived": 0 }));

        // Events on the first version and on a second one.
        for key in ["one", "two", "three"] {
            let body = json!({ "key": key, "chart": a.chart });
            let (response, _) = post(&app, sk, "/v1/events", body).await;
            assert_eq!(response.status(), 201);
        }
        let document: Value = serde_json::from_str(&a.document).unwrap();
        let mut renamed = document.clone();
        renamed["name"] = json!("renamed");
        let uri = format!("/v1/charts/{}/draft", a.chart);
        let headers = [(header::IF_MATCH, "*")];
        let (response, _) =
            call_with(&app, Method::PUT, &uri, Some(sk), &headers, Some(renamed)).await;
        assert_eq!(response.status(), 200);
        let uri = format!("/v1/charts/{}/publish", a.chart);
        let (response, version) = call(&app, Method::POST, &uri, Some(sk), None).await;
        assert_eq!(response.status(), 201, "{version}");
        let body = json!({ "key": "four", "chart": a.chart, "version": version["number"] });
        let (response, _) = post(&app, sk, "/v1/events", body).await;
        assert_eq!(response.status(), 201);
        assert_eq!(counts(sk).await, (json!(5), json!(0)));

        for key in ["one", "four", "four"] {
            let uri = format!("/v1/events/{key}");
            let (response, _) = call(&app, Method::DELETE, &uri, Some(sk), None).await;
            assert_eq!(response.status(), 200);
        }
        assert_eq!(counts(sk).await, (json!(3), json!(2)));
        // No route deletes an event; an operator's hand can.
        sqlx::query("DELETE FROM events WHERE key = 'two'")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(counts(sk).await, (json!(2), json!(2)));

        // The other workspace's events on its own `chart-x` stay its own.
        for key in ["b1", "b2"] {
            let body = json!({ "key": key, "chart": b.chart });
            let (response, _) = post(&app, &b.secret, "/v1/events", body).await;
            assert_eq!(response.status(), 201);
        }
        assert_eq!(counts(sk).await, (json!(2), json!(2)));
        assert_eq!(counts(&b.secret).await, (json!(3), json!(0)));

        // Every chart reply carries them: archiving, and saving the draft.
        let uri = format!("/v1/charts/{}/draft", a.chart);
        let (response, saved) =
            call_with(&app, Method::PUT, &uri, Some(sk), &headers, Some(document)).await;
        assert_eq!(response.status(), 200);
        assert_eq!(saved["events"], json!({ "active": 2, "archived": 2 }));
        let uri = format!("/v1/charts/{}", a.chart);
        let (response, archived) = call(&app, Method::DELETE, &uri, Some(sk), None).await;
        assert_eq!(response.status(), 200);
        assert_eq!(archived["events"], json!({ "active": 2, "archived": 2 }));
    })
    .await;
}

/// `PATCH /v1/keys/{id}`: a publishable key's allow-list changes for its next
/// request, back to every origin with `null`, with an audit row each time;
/// a secret key, a revoked key and a missing field are refused.
#[tokio::test]
#[ignore = "needs CAVEA_TEST_DATABASE_URL"]
async fn publishable_key_origins_change() {
    let _serial = serial();
    with_db(async move |pool| {
        let app = api(&pool, Some(ADMIN));
        let t = Tenant::create(&app, "a").await;
        let sk = t.secret.as_str();
        let uri = format!("/v1/keys/{}", t.publishable_id);
        let patch = async |uri: &str, body: Value| call(&app, Method::PATCH, uri, Some(sk), Some(body)).await;
        let public = async |origin: &str| {
            call_from(&app, Method::GET, "/test/public", Some(&t.publishable), Some(origin), None)
                .await
                .0
                .status()
        };
        assert_eq!(public("https://elsewhere.example").await, 200);
        let body = json!({ "allowed_origins": ["https://tickets.example.com"] });
        let (response, key) = patch(&uri, body).await;
        assert_eq!(response.status(), 200, "{key}");
        assert_eq!(key["allowed_origins"], json!(["https://tickets.example.com"]));
        assert_eq!(public("https://elsewhere.example").await, 403);
        assert_eq!(public("https://tickets.example.com").await, 200);
        let (response, key) = patch(&uri, json!({ "allowed_origins": null })).await;
        assert_eq!((response.status(), &key["allowed_origins"]), (StatusCode::OK, &Value::Null));
        assert_eq!(public("https://elsewhere.example").await, 200);
        let rows: Vec<(String, String)> = sqlx::query_as(
            "SELECT target, detail::text FROM audit_log WHERE action = 'api_key.origins_changed' ORDER BY id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            rows,
            [
                (t.publishable_id.clone(), r#"{"origins": 1}"#.to_owned()),
                (t.publishable_id.clone(), r#"{"origins": null}"#.to_owned())
            ]
        );

        let (response, problem) = patch(&uri, json!({})).await;
        assert_problem(&response, &problem, Code::InvalidRequest);
        let body = json!({ "allowed_origins": ["not an origin"] });
        let (response, problem) = patch(&uri, body).await;
        assert_eq!(problem["details"]["field"], "allowed_origins", "{problem}");
        assert_eq!(response.status(), 400);
        let secret = format!("/v1/keys/{}", t.secret_id);
        let body = json!({ "allowed_origins": ["https://a.example"] });
        let (_, problem) = patch(&secret, body.clone()).await;
        assert_eq!(problem["details"]["field"], "allowed_origins", "{problem}");
        let (response, _) = call(&app, Method::DELETE, &uri, Some(sk), None).await;
        assert_eq!(response.status(), 200);
        let (_, problem) = patch(&uri, body).await;
        assert_eq!(problem["details"]["reason"], "revoked", "{problem}");
    })
    .await;
}

/// 07 §9: the editing lock's life from the clients' side. An editor takes it
/// and renews it (`since` kept); another editor sees who holds it (a viewer
/// sees no email), cannot take it without `take_over`, and cannot save or
/// publish while it is live (`chart_locked`, apart from an etag conflict);
/// taking it over switches the first to refused; it expires by the
/// database's clock; releasing another's does nothing. A secret key is held
/// to the same rule. Another workspace's chart is the matrix's `404`.
#[tokio::test]
#[ignore = "needs CAVEA_TEST_DATABASE_URL"]
async fn chart_lock_life_cycle() {
    let _serial = serial();
    with_db(async move |pool| {
        let app = api(&pool, Some(ADMIN));
        let t = Tenant::create(&app, "a").await;
        let (anna, anna_id) = member(&pool, &t.id, Role::Editor, "anna@a.example").await;
        let (ben, ben_id) = member(&pool, &t.id, Role::Editor, "ben@a.example").await;
        let (_, viewer_id) = member(&pool, &t.id, Role::Viewer, "vic@a.example").await;
        let lock_uri = format!("/v1/charts/{}/lock", t.chart);
        let draft_uri = format!("/v1/charts/{}/draft", t.chart);
        let as_member = async |id: &SessionId, method: Method, uri: &str, body: Option<Value>| {
            call_with(
                &app,
                method,
                uri,
                None,
                &refs(&console(id, Some(&t.id))),
                body,
            )
            .await
        };
        let save = async |id: &SessionId, etag: &str| {
            let mut headers = console(id, Some(&t.id));
            headers.push((header::IF_MATCH, etag.to_owned()));
            headers.push((header::CONTENT_TYPE, "application/json".to_owned()));
            let req = request(
                Method::PUT,
                &draft_uri,
                None,
                &refs(&headers),
                Some(t.document.clone()),
            );
            send(app.clone(), req).await
        };

        let (response, state) = as_member(&viewer_id, Method::GET, &lock_uri, None).await;
        assert_eq!(
            (response.status(), &state),
            (StatusCode::OK, &json!({ "lock": null }))
        );
        let (response, lock) = as_member(&anna_id, Method::POST, &lock_uri, Some(json!({}))).await;
        assert_eq!(response.status(), 200, "{lock}");
        assert_eq!(
            (lock["holder"].as_str(), &lock["email"]),
            (Some(&*format!("usr_{anna}")), &json!("anna@a.example"))
        );
        tokio::time::sleep(Duration::from_millis(1_100)).await;
        let (_, renewed) = as_member(&anna_id, Method::POST, &lock_uri, Some(json!({}))).await;
        assert_eq!(renewed["since"], lock["since"]);
        assert!(renewed["expires_at"].as_str() > lock["expires_at"].as_str());

        // Ben sees who edits; the viewer sees the holder only.
        let (response, problem) =
            as_member(&ben_id, Method::POST, &lock_uri, Some(json!({}))).await;
        assert_problem(&response, &problem, Code::ChartLocked);
        assert_eq!(problem["details"]["lock"]["email"], "anna@a.example");
        let (_, state) = as_member(&viewer_id, Method::GET, &lock_uri, None).await;
        assert_eq!(
            (&state["lock"]["holder"], &state["lock"]["email"]),
            (&lock["holder"], &Value::Null)
        );
        // Ben cannot save or publish; a stale etag from Anna is an etag
        // conflict, not a lock.
        let (response, problem) = save(&ben_id, "*").await;
        assert_problem(&response, &problem, Code::ChartLocked);
        let (response, problem) = save(&anna_id, "\"999\"").await;
        assert_problem(&response, &problem, Code::VersionConflict);
        let (response, saved) = save(&anna_id, "*").await;
        assert_eq!(response.status(), 200, "{saved}");
        let publish = format!("/v1/charts/{}/publish", t.chart);
        let (response, problem) = as_member(&ben_id, Method::POST, &publish, None).await;
        assert_problem(&response, &problem, Code::ChartLocked);
        // Even with a stale etag: the lock is told first.
        let stale = [(header::IF_MATCH, "\"1\"".to_owned())];
        let mut headers = console(&ben_id, Some(&t.id));
        headers.extend(stale);
        let (response, problem) =
            call_with(&app, Method::POST, &publish, None, &refs(&headers), None).await;
        assert_problem(&response, &problem, Code::ChartLocked);
        // So is a key.
        let (response, problem) = call(&app, Method::POST, &publish, Some(&t.secret), None).await;
        assert_problem(&response, &problem, Code::ChartLocked);
        // Releasing another's lock leaves it.
        let (response, _) = as_member(&ben_id, Method::DELETE, &lock_uri, None).await;
        assert_eq!(response.status(), 204);
        let (_, state) = as_member(&ben_id, Method::GET, &lock_uri, None).await;
        assert_eq!(state["lock"]["holder"], lock["holder"]);

        // Ben takes over: Anna is now the one refused, on renewal and save.
        let body = Some(json!({ "take_over": true }));
        let (response, taken) = as_member(&ben_id, Method::POST, &lock_uri, body).await;
        assert_eq!(response.status(), 200, "{taken}");
        assert_eq!(taken["holder"], format!("usr_{ben}"));
        assert_ne!(taken["since"], lock["since"]);
        let (response, problem) =
            as_member(&anna_id, Method::POST, &lock_uri, Some(json!({}))).await;
        assert_problem(&response, &problem, Code::ChartLocked);
        assert_eq!(problem["details"]["lock"]["holder"], format!("usr_{ben}"));
        let (response, problem) = save(&anna_id, "*").await;
        assert_problem(&response, &problem, Code::ChartLocked);
        let (response, _) = save(&ben_id, "*").await;
        assert_eq!(response.status(), 200);

        // Expired by the database's clock: free for anyone, a key included.
        sqlx::query("UPDATE charts SET lock_expires_at = now() - interval '1 second'")
            .execute(&pool)
            .await
            .unwrap();
        let (_, state) = as_member(&anna_id, Method::GET, &lock_uri, None).await;
        assert_eq!(state["lock"], Value::Null);
        let (response, _) = save(&anna_id, "*").await;
        assert_eq!(response.status(), 200);
        let (response, lock) = post(&app, &t.secret, &lock_uri, json!({})).await;
        assert_eq!(
            (response.status(), &lock["email"]),
            (StatusCode::OK, &Value::Null)
        );
        assert_eq!(lock["holder"], t.secret_id);
        let (response, _) = call(&app, Method::DELETE, &lock_uri, Some(&t.secret), None).await;
        assert_eq!(response.status(), 204);
        let (_, state) = as_member(&anna_id, Method::GET, &lock_uri, None).await;
        assert_eq!(state["lock"], Value::Null);

        // No lock on an archived chart; a missing body is refused.
        let (response, problem) = as_member(&anna_id, Method::POST, &lock_uri, None).await;
        assert_eq!(response.status(), 400, "{problem}");
        let uri = format!("/v1/charts/{}", t.chart);
        let (response, _) = call(&app, Method::DELETE, &uri, Some(&t.secret), None).await;
        assert_eq!(response.status(), 200);
        let (response, problem) =
            as_member(&anna_id, Method::POST, &lock_uri, Some(json!({}))).await;
        assert_eq!(problem["details"]["reason"], "archived", "{problem}");
        assert_eq!(response.status(), 400);
    })
    .await;
}

/// 09 §3.1, §3.2: a version's thumbnail and its publisher. The thumbnail is
/// an image this workspace uploaded, shown on the charts list and the
/// versions; the versions list names the publisher, and their email only to
/// a member who may see members.
#[tokio::test]
#[ignore = "needs CAVEA_TEST_DATABASE_URL"]
async fn version_thumbnails_and_authors() {
    let _serial = serial();
    with_db(async move |pool| {
        let app = api(&pool, Some(ADMIN));
        let t = Tenant::create(&app, "a").await;
        let other = Tenant::create(&app, "b").await;
        let (owner, owner_id) = member(&pool, &t.id, Role::Owner, "olga@a.example").await;
        let (_, editor_id) = member(&pool, &t.id, Role::Editor, "ed@a.example").await;
        let upload = async |secret: &str, n: u32| {
            let req = Request::builder()
                .method(Method::POST)
                .uri("/v1/assets")
                .header(header::AUTHORIZATION, format!("Bearer {secret}"))
                .header(header::CONTENT_TYPE, "image/png")
                .body(Body::from(crate::assets::tests::png(64, n)))
                .unwrap();
            let (response, asset) = send(app.clone(), req).await;
            assert_eq!(response.status(), 201, "{asset}");
            asset["hash"].as_str().unwrap().to_owned()
        };
        let mine = upload(&t.secret, 40).await;
        let theirs = upload(&other.secret, 41).await;
        let uri = format!("/v1/charts/{}/versions/1/thumbnail", t.chart);
        let put = async |uri: &str, asset: &str| {
            call(
                &app,
                Method::PUT,
                uri,
                Some(&t.secret),
                Some(json!({ "asset": asset })),
            )
            .await
        };
        let (response, version) = put(&uri, &mine).await;
        assert_eq!(response.status(), 200, "{version}");
        assert_eq!(
            version["thumbnail"],
            json!({ "hash": mine, "url": format!("/v1/public/assets/{mine}") })
        );
        let (_, problem) = put(&uri, &theirs).await;
        assert_eq!(problem["details"]["field"], "asset", "{problem}");
        let (_, problem) = put(&uri, "nothex").await;
        assert_eq!(problem["details"]["field"], "asset", "{problem}");
        let missing = format!("/v1/charts/{}/versions/9/thumbnail", t.chart);
        let (response, problem) = put(&missing, &mine).await;
        assert_problem(&response, &problem, Code::NotFound);
        assert_eq!(problem["details"]["missing"], "version");
        let (_, charts) = get(&app, &t.secret, "/v1/charts").await;
        assert_eq!(
            charts["data"][0]["published_version"]["thumbnail"]["hash"],
            mine
        );

        // Version 2, published by the owner in the console.
        let mut document: Value = serde_json::from_str(&t.document).unwrap();
        document["name"] = json!("second");
        let mut headers = console(&owner_id, Some(&t.id));
        headers.push((header::IF_MATCH, "*".to_owned()));
        headers.push((header::CONTENT_TYPE, "application/json".to_owned()));
        let draft = format!("/v1/charts/{}/draft", t.chart);
        let req = request(
            Method::PUT,
            &draft,
            None,
            &refs(&headers),
            Some(document.to_string()),
        );
        assert_eq!(send(app.clone(), req).await.0.status(), 200);
        let publish = format!("/v1/charts/{}/publish", t.chart);
        let (response, v2) = call_with(
            &app,
            Method::POST,
            &publish,
            None,
            &refs(&console(&owner_id, Some(&t.id))),
            None,
        )
        .await;
        assert_eq!(response.status(), 201, "{v2}");
        assert_eq!(
            (&v2["created_by"], &v2["thumbnail"]),
            (&json!(format!("usr_{owner}")), &Value::Null)
        );

        let versions = format!("/v1/charts/{}/versions", t.chart);
        let authors = |page: &Value| -> Vec<(Value, Value)> {
            page["data"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| (v["created_by"].clone(), v["created_by_email"].clone()))
                .collect()
        };
        let (_, page) = call_with(
            &app,
            Method::GET,
            &versions,
            None,
            &refs(&console(&owner_id, Some(&t.id))),
            None,
        )
        .await;
        assert_eq!(
            authors(&page),
            [
                (json!(t.secret_id), Value::Null),
                (json!(format!("usr_{owner}")), json!("olga@a.example"))
            ]
        );
        let (_, page) = call_with(
            &app,
            Method::GET,
            &versions,
            None,
            &refs(&console(&editor_id, Some(&t.id))),
            None,
        )
        .await;
        assert_eq!(
            authors(&page)[1],
            (json!(format!("usr_{owner}")), Value::Null)
        );
        let (_, page) = get(&app, &t.secret, &versions).await;
        assert_eq!(
            authors(&page)[1],
            (json!(format!("usr_{owner}")), Value::Null)
        );
    })
    .await;
}

/// p50 and p99 of `times`, in milliseconds.
fn quantiles(mut times: Vec<f64>) -> (f64, f64) {
    times.sort_by(f64::total_cmp);
    let at = |q: f64| times[((times.len() - 1) as f64 * q) as usize];
    (at(0.5), at(0.99))
}

/// Time `n` GETs of `uri` (after one to warm up): p50, p99 and the body's
/// size.
async fn timed(app: &Router, secret: &str, uri: &str, n: usize) -> (f64, f64, usize) {
    let mut times = Vec::new();
    let mut size = 0;
    for i in 0..=n {
        let req = request(Method::GET, uri, Some(secret), &[], None);
        let started = std::time::Instant::now();
        let response = app.clone().oneshot(req).await.unwrap();
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 24)
            .await
            .unwrap();
        if i > 0 {
            times.push(started.elapsed().as_secs_f64() * 1e3);
        }
        size = bytes.len();
    }
    let (p50, p99) = quantiles(times);
    (p50, p99, size)
}

/// The staff state and the reads the console makes, at the sizes the spec
/// names: `stadium-100k` with a quarter of its seats not free (held, booked,
/// blocked, spread over the chart), and a workspace of 1,000 events. Through
/// the router, no network. Run with `--release -- --ignored --nocapture
/// measure_console_reads --test-threads=1`.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "a measurement: needs CAVEA_TEST_DATABASE_URL and --release"]
async fn measure_console_reads() {
    let _serial = serial();
    with_db(async move |pool| {
        let app = api(&pool, Some(ADMIN));
        let t = Tenant::create(&app, "a").await;
        let sk = t.secret.as_str();
        let mut document = cavea_testkit::synthetic::stadium(100_000, 1);
        document.name = "big".to_owned();
        let document: Value = serde_json::from_slice(&document.to_json().unwrap()).unwrap();
        let body = json!({ "key": "big", "name": "big", "document": document });
        let (response, _) = post(&app, sk, "/v1/charts", body).await;
        assert_eq!(response.status(), 201);
        let (response, version) = call(&app, Method::POST, "/v1/charts/big/publish", Some(sk), None).await;
        assert_eq!(response.status(), 201, "{version}");
        let hash = version["hash"].as_str().unwrap().to_owned();
        let (response, _) = post(&app, sk, "/v1/events", json!({ "key": "big", "chart": "big" })).await;
        assert_eq!(response.status(), 201);
        // 25,000 of 100,000: every fourth seat, a third each held, booked
        // and blocked, in commands of 1,000.
        let seats: Vec<u32> = (0..100_000).step_by(4).collect();
        for (n, chunk) in seats.chunks(1_000).enumerate() {
            let objects = json!({ "version": hash, "idx": chunk });
            let (uri, body) = match n % 3 {
                0 => {
                    let token = json!({ "event": "big" });
                    let (_, token) = post(&app, sk, "/v1/hold-tokens", token).await;
                    ("hold", json!({ "hold_token": token["hold_token"], "objects": objects }))
                }
                1 => ("book", json!({ "objects": objects, "order_ref": format!("o-{n}") })),
                _ => ("change-status", json!({ "objects": objects, "status": "blocked" })),
            };
            let (response, reply) = post(&app, sk, &format!("/v1/events/big/{uri}"), body).await;
            assert_eq!(response.status(), 200, "{reply}");
        }
        tokio::time::sleep(Duration::from_millis(300)).await;

        let started = std::time::Instant::now();
        let (response, _) = get(&app, sk, "/v1/events/big/state").await;
        assert_eq!(response.status(), 200);
        println!("staff snapshot, first (encoded): {:.2} ms", started.elapsed().as_secs_f64() * 1e3);
        let (p50, p99, size) = timed(&app, sk, "/v1/events/big/state", 200).await;
        println!("staff snapshot, cached: p50 {p50:.3} ms, p99 {p99:.3} ms, {size} bytes");
        let public = format!("/v1/public/events/big/state?key={}", t.publishable);
        let (p50, p99, size) = timed(&app, sk, &public, 200).await;
        println!("public snapshot: p50 {p50:.3} ms, p99 {p99:.3} ms, {size} bytes");
        let (_, event) = get(&app, sk, "/v1/events/big").await;
        let seq = event["seq"].as_u64().unwrap();
        let (p50, p99, _) = timed(&app, sk, &format!("/v1/events/big/state?after_seq={seq}"), 200).await;
        println!("staff poll, nothing new (204): p50 {p50:.3} ms, p99 {p99:.3} ms");
        let body = json!({ "objects": { "version": hash, "idx": [1, 5, 9] }, "status": "blocked" });
        let (response, _) = post(&app, sk, "/v1/events/big/change-status", body).await;
        assert_eq!(response.status(), 200);
        tokio::time::sleep(Duration::from_millis(300)).await;
        let (p50, p99, size) = timed(&app, sk, &format!("/v1/events/big/state?after_seq={seq}"), 200).await;
        println!("staff poll, a delta of 3 changes: p50 {p50:.3} ms, p99 {p99:.3} ms, {size} bytes");
        let (p50, p99, _) = timed(&app, sk, "/v1/events/big", 200).await;
        println!("event detail with counts: p50 {p50:.3} ms, p99 {p99:.3} ms");
        // What the detail's counts cost before `events.counts`: a scan.
        let mut times = Vec::new();
        for _ in 0..200 {
            let started = std::time::Instant::now();
            let _: Vec<(i16, i64)> = sqlx::query_as(
                "SELECT status, count(*) FROM object_state
                  WHERE event_id = (SELECT id FROM events WHERE key = 'big') GROUP BY status",
            )
            .fetch_all(&pool)
            .await
            .unwrap();
            times.push(started.elapsed().as_secs_f64() * 1e3);
        }
        let (p50, p99) = quantiles(times);
        println!("the scan the counts replace (25,000 rows): p50 {p50:.3} ms, p99 {p99:.3} ms");

        // 1,000 events: the list's first page, a search, newest first.
        for i in 0..1_000 {
            let body = json!({ "key": format!("ev-{i:04}"), "chart": t.chart, "name": format!("Night {i}") });
            let (response, _) = post(&app, sk, "/v1/events", body).await;
            assert_eq!(response.status(), 201);
        }
        for (label, uri) in [
            ("list, 100 of 1,000", "/v1/events?limit=100"),
            ("list, newest 100", "/v1/events?limit=100&order=newest"),
            ("list, search 'night 99'", "/v1/events?limit=100&q=night%2099"),
            ("list, last page by cursor", "/v1/events?limit=100&after=ev-0950"),
        ] {
            let (p50, p99, size) = timed(&app, sk, uri, 100).await;
            println!("{label}: p50 {p50:.3} ms, p99 {p99:.3} ms, {size} bytes");
        }
    })
    .await;
}

/// The charts list with each chart's event counts, at 1,000 charts and
/// 10,000 events (ten a chart, a third archived), made by SQL from the
/// tenant's chart and event: the console reads the whole list, ten pages.
/// Then the same without `events_chart_version`, for its worth. Run with
/// `--release -- --ignored --nocapture measure_charts_list --test-threads=1`.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "a measurement: needs CAVEA_TEST_DATABASE_URL and --release"]
async fn measure_charts_list() {
    let _serial = serial();
    with_db(async move |pool| {
        let app = api(&pool, Some(ADMIN));
        let t = Tenant::create(&app, "a").await;
        let sk = t.secret.as_str();
        for sql in [
            "INSERT INTO charts (id, workspace_id, key, name, draft, created_at)
             SELECT gen_random_uuid(), c.workspace_id, 'm-' || lpad(i::text, 4, '0'),
                    'Chart ' || i, c.draft, now() + i * interval '1 ms'
               FROM charts c, generate_series(1, 999) i WHERE c.key = $1",
            "INSERT INTO chart_versions
                    (id, chart_id, number, hash, full_hash, source, public_ccf, public_ccf_br, full_ccf, stats)
             SELECT gen_random_uuid(), m.id, 1, v.hash, v.full_hash, v.source, v.public_ccf, v.public_ccf_br,
                    v.full_ccf, v.stats
               FROM charts m, charts c JOIN chart_versions v ON v.id = c.published_version_id
              WHERE m.key LIKE 'm-%' AND c.key = $1",
            "UPDATE charts m SET published_version_id = v.id
               FROM chart_versions v WHERE v.chart_id = m.id AND m.key LIKE 'm-%' AND $1 <> ''",
            "INSERT INTO events (id, workspace_id, key, chart_version_id, archived_at)
             SELECT gen_random_uuid(), m.workspace_id, m.key || '-' || j, m.published_version_id,
                    CASE WHEN j % 3 = 0 THEN now() END
               FROM charts m, generate_series(1, 10) j WHERE m.key LIKE 'm-%' AND $1 <> ''",
            "ANALYZE",
        ] {
            let query = sqlx::query(sql);
            let query = if sql == "ANALYZE" { query } else { query.bind(&t.chart) };
            query.execute(&pool).await.unwrap();
        }
        let (_, page) = get(&app, sk, "/v1/charts/m-0500").await;
        assert_eq!(page["events"], json!({ "active": 7, "archived": 3 }));
        for index in [true, false] {
            if !index {
                sqlx::query("DROP INDEX events_chart_version")
                    .execute(&pool)
                    .await
                    .unwrap();
            }
            let label = if index { "with the index" } else { "without it" };
            for (what, uri) in [
                ("first page of 100", "/v1/charts?limit=100".to_owned()),
                ("last page by cursor", "/v1/charts?limit=100&after=m-0899".to_owned()),
                ("one chart", "/v1/charts/m-0500".to_owned()),
            ] {
                let (p50, p99, size) = timed(&app, sk, &uri, 50).await;
                println!("charts, {what}, {label}: p50 {p50:.3} ms, p99 {p99:.3} ms, {size} bytes");
            }
            let mut times = Vec::new();
            for _ in 0..20 {
                let started = std::time::Instant::now();
                let mut after = String::new();
                let mut charts = 0;
                loop {
                    let (_, page) = get(&app, sk, &format!("/v1/charts?limit=100{after}")).await;
                    charts += page["data"].as_array().unwrap().len();
                    match page["next"].as_str() {
                        Some(next) => after = format!("&after={next}"),
                        None => break,
                    }
                }
                assert_eq!(charts, 1_000);
                times.push(started.elapsed().as_secs_f64() * 1e3);
            }
            let (p50, p99) = quantiles(times);
            println!("charts, the whole list (10 pages), {label}: p50 {p50:.3} ms, p99 {p99:.3} ms");
        }
    })
    .await;
}
