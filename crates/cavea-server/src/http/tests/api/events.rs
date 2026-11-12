//! The events list against PostgreSQL (08 §6.2, 09 §3.2, §4.1): `chart=` keeps
//! one chart's events, archived chart included, pages through them in creation
//! order, and answers `404` for a chart the workspace does not have; the
//! creation range composes with the other filters and the cursor. Another
//! workspace's chart is the matrix's (`authorisation_matrix`,
//! `identical_keys_never_cross`).

use super::*;

/// Every key of `?chart=…` pages of `limit`, following `next`.
async fn keys(app: &Router, secret: &str, chart: &str, limit: u32) -> Vec<String> {
    let mut keys = Vec::new();
    let mut after = String::new();
    loop {
        let uri = format!("/v1/events?chart={chart}&limit={limit}{after}");
        let (response, page) = call(app, Method::GET, &uri, Some(secret), None).await;
        assert_eq!(response.status(), 200, "{page}");
        let data = page["data"].as_array().unwrap();
        assert!(data.len() <= limit as usize);
        for event in data {
            assert_eq!(event["chart"], chart);
            keys.push(event["key"].as_str().unwrap().to_owned());
        }
        match page["next"].as_str() {
            Some(next) => after = format!("&after={next}"),
            None => return keys,
        }
    }
}

#[tokio::test]
#[ignore = "needs CAVEA_TEST_DATABASE_URL"]
async fn events_filter_by_chart() {
    let _serial = serial();
    with_db(async move |pool| {
        let app = api(&pool, Some(ADMIN));
        let a = Tenant::create(&app, "a").await;
        let other = "other-chart";
        let body = json!({ "key": other, "name": "Other",
            "document": serde_json::from_str::<Value>(&a.document).unwrap() });
        let (response, created) = call(
            &app,
            Method::POST,
            "/v1/charts",
            Some(&a.secret),
            Some(body),
        )
        .await;
        assert_eq!(response.status(), 201, "{created}");
        let uri = format!("/v1/charts/{other}/publish");
        let (response, _) = call(&app, Method::POST, &uri, Some(&a.secret), None).await;
        assert_eq!(response.status(), 201);
        // Interleaved, so that a filter that ignored the chart, or a cursor
        // that walked the unfiltered order, would show.
        let mut mine = vec![a.event.clone()];
        let mut theirs = Vec::new();
        for i in 0..5 {
            for (chart, list) in [(&a.chart, &mut mine), (&other.to_owned(), &mut theirs)] {
                let key = format!("{chart}-{i}");
                let body = json!({ "key": key, "chart": chart });
                let (response, event) = call(
                    &app,
                    Method::POST,
                    "/v1/events",
                    Some(&a.secret),
                    Some(body),
                )
                .await;
                assert_eq!(response.status(), 201, "{event}");
                list.push(key);
            }
        }
        for limit in [1, 2, 4, 100] {
            assert_eq!(
                keys(&app, &a.secret, &a.chart, limit).await,
                mine,
                "{limit}"
            );
            assert_eq!(keys(&app, &a.secret, other, limit).await, theirs, "{limit}");
        }
        // An archived chart's events are still its events.
        let uri = format!("/v1/charts/{other}");
        let (response, _) = call(&app, Method::DELETE, &uri, Some(&a.secret), None).await;
        assert_eq!(response.status(), 200);
        assert_eq!(keys(&app, &a.secret, other, 3).await, theirs);
        // No such chart; and a chart without events is an empty page.
        let (response, problem) = call(
            &app,
            Method::GET,
            "/v1/events?chart=nope",
            Some(&a.secret),
            None,
        )
        .await;
        assert_problem(&response, &problem, Code::NotFound);
        assert_eq!(problem["details"]["missing"], "chart");
        let body = json!({ "key": "empty", "name": "Empty",
            "document": serde_json::from_str::<Value>(&a.document).unwrap() });
        let (response, _) = call(
            &app,
            Method::POST,
            "/v1/charts",
            Some(&a.secret),
            Some(body),
        )
        .await;
        assert_eq!(response.status(), 201);
        assert_eq!(
            keys(&app, &a.secret, "empty", 10).await,
            Vec::<String>::new()
        );
        // Unfiltered, everything, as before.
        let (_, all) = call(&app, Method::GET, "/v1/events", Some(&a.secret), None).await;
        assert_eq!(
            all["data"].as_array().unwrap().len(),
            mine.len() + theirs.len()
        );
    })
    .await;
}

/// Every key of `/v1/events?{query}` in pages of `limit`, following `next`.
async fn walk(app: &Router, secret: &str, query: &str, limit: u32) -> Vec<String> {
    let mut keys = Vec::new();
    let mut after = String::new();
    loop {
        let uri = format!("/v1/events?{query}&limit={limit}{after}");
        let (response, page) = call(app, Method::GET, &uri, Some(secret), None).await;
        assert_eq!(response.status(), 200, "{uri}: {page}");
        for event in page["data"].as_array().unwrap() {
            keys.push(event["key"].as_str().unwrap().to_owned());
        }
        match page["next"].as_str() {
            Some(next) => after = format!("&after={next}"),
            None => return keys,
        }
    }
}

/// CON-007's date range: `created_after` (inclusive) and `created_before`
/// (exclusive) compose with search, archive, order and the cursor, which
/// stays valid page after page.
#[tokio::test]
#[ignore = "needs CAVEA_TEST_DATABASE_URL"]
async fn events_filter_by_creation() {
    let _serial = serial();
    with_db(async move |pool| {
        let app = api(&pool, Some(ADMIN));
        let a = Tenant::create(&app, "a").await;
        // Two events a day from 1 to 6 March 2027, `show-` at noon and
        // `talk-` an hour later; the tenant's own event is from today, before them.
        for day in 1..=6 {
            for (kind, hour) in [("show", 12), ("talk", 13)] {
                let key = format!("{kind}-{day}");
                let body = json!({ "key": key, "chart": a.chart });
                let (response, _) = call(
                    &app,
                    Method::POST,
                    "/v1/events",
                    Some(&a.secret),
                    Some(body),
                )
                .await;
                assert_eq!(response.status(), 201);
                let at = format!("2027-03-0{day}T{hour}:00:00Z");
                sqlx::query("UPDATE events SET created_at = $2::timestamptz WHERE key = $1")
                    .bind(&key)
                    .bind(&at)
                    .execute(&pool)
                    .await
                    .unwrap();
            }
        }
        let uri = "/v1/events/talk-3";
        let (response, _) = call(&app, Method::DELETE, uri, Some(&a.secret), None).await;
        assert_eq!(response.status(), 200);
        let days = |kinds: &[&str], range: std::ops::RangeInclusive<u32>| -> Vec<String> {
            range
                .flat_map(|d| kinds.iter().map(move |k| format!("{k}-{d}")))
                .collect()
        };
        let both = ["show", "talk"];
        for limit in [1, 3, 100] {
            let range = "created_after=2027-03-02&created_before=2027-03-05";
            assert_eq!(
                walk(&app, &a.secret, range, limit).await,
                days(&both, 2..=4)
            );
            // To the microsecond: at the instant is in, before it is out.
            let exact =
                "created_after=2027-03-02T12:00:00Z&created_before=2027-03-04T13:00:00.000001Z";
            assert_eq!(
                walk(&app, &a.secret, exact, limit).await,
                days(&both, 2..=4)
            );
            let open = "created_after=2027-01-01&created_before=2027-03-03";
            assert_eq!(walk(&app, &a.secret, open, limit).await, days(&both, 1..=2));
            let mut newest = days(&both, 4..=6);
            newest.reverse();
            let query = "created_after=2027-03-04T00:00:00.5Z&order=newest";
            assert_eq!(walk(&app, &a.secret, query, limit).await, newest);
            let query = "created_after=2027-03-01&created_before=2027-03-07&q=show";
            assert_eq!(
                walk(&app, &a.secret, query, limit).await,
                days(&["show"], 1..=6)
            );
            let query = "created_before=2027-03-07&archived=true";
            assert_eq!(walk(&app, &a.secret, query, limit).await, ["talk-3"]);
            let query = format!("created_before=2027-03-07&chart={}&archived=false", a.chart);
            let mut live = vec![a.event.clone()];
            live.extend(days(&both, 1..=6));
            live.retain(|k| k != "talk-3");
            assert_eq!(walk(&app, &a.secret, &query, limit).await, live);
        }
        // Today's event is before all of them; an empty range is an empty page.
        let query = "created_after=2027-03-07";
        assert_eq!(walk(&app, &a.secret, query, 10).await, Vec::<String>::new());
        let query = "created_after=2020-01-01&created_before=2027-01-01";
        assert_eq!(
            walk(&app, &a.secret, query, 10).await,
            std::slice::from_ref(&a.event)
        );
        let query = "created_after=2027-03-05&created_before=2027-03-02";
        assert_eq!(walk(&app, &a.secret, query, 10).await, Vec::<String>::new());
        for bad in [
            "created_after=2027-03-32",
            "created_before=yesterday",
            "created_after=2027-03-01T00:00:00+01:00",
        ] {
            let uri = format!("/v1/events?{bad}");
            let (response, problem) = call(&app, Method::GET, &uri, Some(&a.secret), None).await;
            assert_problem(&response, &problem, Code::InvalidRequest);
            assert_eq!(problem["details"]["reason"], "query", "{bad}");
        }
    })
    .await;
}
