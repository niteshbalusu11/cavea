//! `GET /v1/events/{event}/objects` against PostgreSQL (08 §6.2).

use std::collections::BTreeMap;

use super::*;

/// Percent-encode a label for a query string.
fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
                char::from(b).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

/// Every page of `query`, following `next`: the objects and the page count.
async fn walk(app: &Router, sk: &str, query: &str) -> (Vec<Value>, usize) {
    let (mut objects, mut pages, mut after) = (Vec::new(), 0, None::<String>);
    loop {
        let mut uri = format!("/v1/events/big/objects?{query}");
        if let Some(after) = &after {
            uri.push_str(&format!("&after={}", encode(after)));
        }
        let (response, page) = call(app, Method::GET, &uri, Some(sk), None).await;
        assert_eq!(response.status(), 200, "{uri}: {page}");
        let data = page["data"].as_array().unwrap();
        assert!(data.len() <= 100);
        objects.extend(data.iter().cloned());
        pages += 1;
        match page["next"].as_str() {
            Some(next) => after = Some(next.to_owned()),
            None => return (objects, pages),
        }
    }
}

fn keys(object: &Value) -> Vec<&str> {
    object
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect()
}

/// Scattered holds, bookings, blocks and a custom status on a 12,000-seat
/// chart: each object's fields by label; pages of each status, and of all,
/// cover every object exactly once in `idx` order; a sparse status crosses
/// the 10,000-object window with an empty page; no hold token, hash, order
/// reference or `extra` is logged.
#[tokio::test]
#[ignore = "needs CAVEA_TEST_DATABASE_URL"]
async fn objects_query() {
    let _serial = serial();
    let logs = Captured::default();
    let _guard = logs.install();
    let logs_in = logs.clone();
    with_db(async move |pool| {
        let logs = logs_in;
        let (app, seats) = api_with(&pool, Some(ADMIN), ServerClock::System, "test");
        let t = Tenant::create(&app, "a").await;
        let sk = t.secret.as_str();
        let mut document = cavea_testkit::synthetic::stadium(12_000, 3);
        document.name = "big".to_owned();
        let document: Value = serde_json::from_slice(&document.to_json().unwrap()).unwrap();
        let body = json!({ "key": "big", "name": "big", "document": document });
        let (response, _) = call(&app, Method::POST, "/v1/charts", Some(sk), Some(body)).await;
        assert_eq!(response.status(), 201);
        let (response, version) =
            call(&app, Method::POST, "/v1/charts/big/publish", Some(sk), None).await;
        assert_eq!(response.status(), 201, "{version}");
        let body = json!({ "key": "big", "chart": "big" });
        let (response, _) = call(&app, Method::POST, "/v1/events", Some(sk), Some(body)).await;
        assert_eq!(response.status(), 201);

        // Everything is free: 120 pages of free objects, each with no more.
        let (all, pages) = walk(&app, sk, "limit=100").await;
        assert_eq!((all.len(), pages), (12_000, 120));
        assert!(
            all.iter()
                .all(|o| o["status"] == "free" && keys(o) == ["label", "oid", "status"])
        );
        let label = |idx: usize| all[idx]["label"].as_str().unwrap().to_owned();
        let labels = |idx: &[usize]| idx.iter().map(|&i| label(i)).collect::<Vec<_>>();

        // Scatter some state.
        let ev = |op: &str| format!("/v1/events/big/{op}");
        let (_, token) = call(
            &app,
            Method::POST,
            "/v1/hold-tokens",
            Some(sk),
            Some(json!({ "event": "big" })),
        )
        .await;
        let token = token["hold_token"].as_str().unwrap().to_owned();
        let held = [10, 11, 5_000, 11_999];
        let body = json!({ "hold_token": token, "objects": labels(&held) });
        let (response, reply) = call(&app, Method::POST, &ev("hold"), Some(sk), Some(body)).await;
        assert_eq!(response.status(), 200, "{reply}");
        let extra = json!({ "note": "EXTRA-secret-1", "n": 1 });
        let body = json!({ "objects": labels(&[20, 21]), "order_ref": "ORDREF-secret-9",
            "ticket_types": { label(20): "adult" }, "extra": { label(21): extra } });
        let (response, reply) = call(&app, Method::POST, &ev("book"), Some(sk), Some(body)).await;
        assert_eq!(response.status(), 200, "{reply}");
        let body = json!({ "objects": labels(&[7_000]), "order_ref": "ORDREF-secret-10" });
        assert_eq!(
            call(&app, Method::POST, &ev("book"), Some(sk), Some(body))
                .await
                .0
                .status(),
            200
        );
        for (idx, status) in [
            (vec![30, 6_000], json!("blocked")),
            (vec![11_000], json!(5)),
        ] {
            let body = json!({ "objects": labels(&idx), "status": status });
            assert_eq!(
                call(
                    &app,
                    Method::POST,
                    &ev("change-status"),
                    Some(sk),
                    Some(body)
                )
                .await
                .0
                .status(),
                200
            );
        }

        // By label: idx order, each once, unknown labels reported.
        let query: String = [
            label(30),
            label(20),
            "Nope".into(),
            label(10),
            label(21),
            label(30),
            label(100),
            "Nope".into(),
        ]
        .iter()
        .map(|l| format!("label={}&", encode(l)))
        .collect();
        let (response, found) = call(
            &app,
            Method::GET,
            &format!("/v1/events/big/objects?{query}"),
            Some(sk),
            None,
        )
        .await;
        assert_eq!(response.status(), 200, "{found}");
        assert_eq!(found["unknown"], json!(["Nope"]));
        let data = found["data"].as_array().unwrap();
        let got: Vec<&str> = data.iter().map(|o| o["label"].as_str().unwrap()).collect();
        assert_eq!(got, labels(&[10, 20, 21, 30, 100]));
        let [h, b20, b21, blocked, free] = &data[..] else {
            panic!("{found}")
        };
        assert_eq!(keys(h), ["expires_at", "label", "oid", "seq", "status"]);
        assert_eq!((&h["status"], &h["oid"]), (&json!("held"), &all[10]["oid"]));
        assert!(h["expires_at"].as_str().unwrap().ends_with('Z') && h["seq"].is_u64());
        assert_eq!(
            keys(b20),
            [
                "extra",
                "label",
                "oid",
                "order_ref",
                "seq",
                "status",
                "ticket_type"
            ]
        );
        assert_eq!(
            (
                &b20["status"],
                &b20["order_ref"],
                &b20["ticket_type"],
                &b20["extra"]
            ),
            (
                &json!("booked"),
                &json!("ORDREF-secret-9"),
                &json!("adult"),
                &Value::Null
            )
        );
        assert_eq!((&b21["ticket_type"], &b21["extra"]), (&Value::Null, &extra));
        assert_eq!(keys(blocked), ["label", "oid", "seq", "status"]);
        assert_eq!(blocked["status"], "blocked");
        assert_eq!(free, &all[100]);

        // Pages of each status, and of all: every object exactly once.
        let mut seen = BTreeMap::new();
        for (status, idx) in [
            ("held", held.to_vec()),
            ("booked", vec![20, 21, 7_000]),
            ("blocked", vec![30, 6_000]),
            ("5", vec![11_000]),
        ] {
            let (objects, _) = walk(&app, sk, &format!("status={status}&limit=100")).await;
            let got: Vec<&str> = objects
                .iter()
                .map(|o| o["label"].as_str().unwrap())
                .collect();
            assert_eq!(got, labels(&idx), "{status}");
            for o in objects {
                assert!(
                    seen.insert(o["label"].as_str().unwrap().to_owned(), o.clone())
                        .is_none()
                );
            }
        }
        let (free, _) = walk(&app, sk, "status=free&limit=100").await;
        assert_eq!(free.len(), 12_000 - 10);
        assert!(free.iter().all(|o| o["status"] == "free"));
        for o in free {
            assert!(
                seen.insert(o["label"].as_str().unwrap().to_owned(), o.clone())
                    .is_none()
            );
        }
        let (again, _) = walk(&app, sk, "limit=100").await;
        assert_eq!(again.len(), 12_000);
        for o in &again {
            assert_eq!(seen.get(o["label"].as_str().unwrap()), Some(o));
        }
        // A rare status: the first window holds none, so its page is empty
        // and says where to continue.
        let (_, page) = call(
            &app,
            Method::GET,
            "/v1/events/big/objects?status=5",
            Some(sk),
            None,
        )
        .await;
        assert_eq!(
            (&page["data"], &page["next"]),
            (&json!([]), &json!(label(9_999)))
        );
        let text = format!("{found}{page}{}", serde_json::to_string(&again).unwrap());
        assert!(!text.contains(&token) && !text.contains(&super::seats::hex(&sha256(&token))));

        // Refusals.
        let too_many: String = (0..1_001).map(|i| format!("label=x{i}&")).collect();
        for (query, reason) in [
            (format!("label={}&status=held", encode(&label(1))), "query"),
            (format!("label={}&limit=5", encode(&label(1))), "query"),
            ("status=sold".to_owned(), "query"),
            ("status=16".to_owned(), "query"),
            ("status=held&status=free".to_owned(), "query"),
            ("after=Nope".to_owned(), "query"),
            ("limit=0".to_owned(), "query"),
            ("limit=101".to_owned(), "query"),
            ("colour=red".to_owned(), "query"),
            (too_many, "too_many_objects"),
        ] {
            let (response, p) = call(
                &app,
                Method::GET,
                &format!("/v1/events/big/objects?{query}"),
                Some(sk),
                None,
            )
            .await;
            assert_problem(&response, &p, Code::InvalidRequest);
            assert_eq!(p["details"]["reason"], reason, "{query:.40}");
        }
        let (response, p) =
            call(&app, Method::GET, "/v1/events/nope/objects", Some(sk), None).await;
        assert_problem(&response, &p, Code::NotFound);

        seats.shutdown().await;
        let mut secrets = t.secrets().to_vec();
        secrets.extend([token.as_str(), "ORDREF-secret", "EXTRA-secret"]);
        assert_no_secrets(&logs, &secrets);
    })
    .await;
}
