//! The events list against PostgreSQL (08 §6.2, 09 §3.2): `chart=` keeps one
//! chart's events, archived chart included, pages through them in creation
//! order, and answers `404` for a chart the workspace does not have. Another
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
