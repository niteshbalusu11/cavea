//! Charts against PostgreSQL: the draft and publish life cycle, the public
//! chart route, racing publishes and hostile drafts (08 §6.2, 03 §4.6).

use std::time::{Duration, Instant};

use cavea_compiler::ChartDocument;
use cavea_compiler::document::{BlockRow, Node};
use cavea_format::Artefact;
use cavea_testkit::synthetic::stadium;

use super::*;

const PUBLISHED: &str = "chart published";

async fn put_draft(
    app: &Router,
    secret: &str,
    chart: &str,
    if_match: Option<&str>,
    body: String,
) -> (Response<Body>, Value) {
    let headers: Vec<_> = if_match
        .map(|m| (header::IF_MATCH, m))
        .into_iter()
        .collect();
    let uri = format!("/v1/charts/{chart}/draft");
    let request = request(Method::PUT, &uri, Some(secret), &headers, Some(body));
    send(app.clone(), request).await
}

async fn post(
    app: &Router,
    secret: &str,
    uri: &str,
    if_match: Option<&str>,
) -> (Response<Body>, Value) {
    let headers: Vec<_> = if_match
        .map(|m| (header::IF_MATCH, m))
        .into_iter()
        .collect();
    call_with(app, Method::POST, uri, Some(secret), &headers, None).await
}

fn json_of(document: &ChartDocument) -> String {
    String::from_utf8(document.to_json().unwrap()).unwrap()
}

/// The first section of a synthetic stadium.
fn first_section(document: &mut ChartDocument) -> &mut cavea_compiler::document::Section {
    match document.nodes.first_mut() {
        Some(Node::Section(section)) => section,
        _ => panic!("a stadium starts with a section"),
    }
}

/// Every column of a version, to show it never changes.
async fn version_row(pool: &PgPool, number: i32) -> Vec<u8> {
    sqlx::query_scalar(
        "SELECT convert_to(v::text, 'UTF8') FROM chart_versions v
          JOIN charts c ON c.id = v.chart_id WHERE c.key = 'royal' AND v.number = $1",
    )
    .bind(number)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// Create, save with etags, validate, publish (refused, then versions 1 to 3),
/// list, archive; stored artefacts decode, and a published version never
/// changes.
#[tokio::test]
#[ignore = "needs CAVEA_TEST_DATABASE_URL"]
async fn chart_lifecycle() {
    let _serial = serial();
    let logs = Captured::default();
    let _guard = logs.install();
    let logs_in = logs.clone();
    with_db(async move |pool| {
        let logs = logs_in;
        let app = api(&pool, Some(ADMIN));
        let t = Tenant::create(&app, "acme").await;
        let sk = t.secret.as_str();

        // Created without a document: an empty draft at etag "1".
        let body = json!({ "key": "royal", "name": "Royal Theatre" });
        let (response, chart) = call(
            &app,
            Method::POST,
            "/v1/charts",
            Some(sk),
            Some(body.clone()),
        )
        .await;
        assert_eq!(response.status(), 201, "{chart}");
        assert_eq!(chart["draft_etag"], "\"1\"");
        assert_eq!(chart["published_version"], Value::Null);
        let (response, problem) =
            call(&app, Method::POST, "/v1/charts", Some(sk), Some(body)).await;
        assert_problem(&response, &problem, Code::AlreadyExists);
        let (response, draft) =
            call(&app, Method::GET, "/v1/charts/royal/draft", Some(sk), None).await;
        assert_eq!(response.status(), 200);
        assert_eq!(response.headers()[header::ETAG], "\"1\"");
        assert_eq!(
            draft,
            json!({ "schema": 1, "name": "Royal Theatre", "nextOid": 1 })
        );

        // A draft that does not validate is saved; If-Match is required and
        // must be current.
        let mut invalid = stadium(2_000, 7);
        let section = first_section(&mut invalid);
        section.category = Some("nope".to_owned());
        let section_id = section.id;
        let (response, problem) = put_draft(&app, sk, "royal", None, json_of(&invalid)).await;
        assert_problem(&response, &problem, Code::InvalidRequest);
        assert_eq!(problem["details"]["reason"], "if_match");
        let (response, chart) =
            put_draft(&app, sk, "royal", Some("\"1\""), json_of(&invalid)).await;
        assert_eq!(response.status(), 200, "{chart}");
        assert_eq!(response.headers()[header::ETAG], "\"2\"");
        assert_eq!(chart["draft_etag"], "\"2\"");
        let (response, problem) =
            put_draft(&app, sk, "royal", Some("\"1\""), json_of(&invalid)).await;
        assert_problem(&response, &problem, Code::VersionConflict);
        assert_eq!(problem["details"]["draft_etag"], "\"2\"");

        // Validate reports the findings with their node; publish refuses.
        let (response, validation) = post(&app, sk, "/v1/charts/royal/validate", None).await;
        assert_eq!(response.status(), 200, "{validation}");
        assert_eq!(validation["valid"], false);
        assert_eq!(validation["draft_etag"], "\"2\"");
        let unknown = json!({ "code": "E007", "severity": "error", "node": section_id });
        let has_unknown = |findings: &Value| {
            findings.as_array().unwrap().iter().any(|f| {
                f["code"] == unknown["code"]
                    && f["node"] == unknown["node"]
                    && f["severity"] == unknown["severity"]
            })
        };
        assert!(has_unknown(&validation["findings"]), "{validation}");
        let (response, problem) = post(&app, sk, "/v1/charts/royal/publish", None).await;
        assert_problem(&response, &problem, Code::ChartInvalid);
        assert!(has_unknown(&problem["details"]["findings"]), "{problem}");
        let (_, versions) = call(
            &app,
            Method::GET,
            "/v1/charts/royal/versions",
            Some(sk),
            None,
        )
        .await;
        assert_eq!(versions["data"], json!([]));

        // A valid draft: validates, then publishes as version 1, but only
        // with a current If-Match.
        let mut document = stadium(2_000, 7);
        document.name = "Royal Theatre".to_owned();
        let (response, _) = put_draft(&app, sk, "royal", Some("\"2\""), json_of(&document)).await;
        assert_eq!(response.status(), 200);
        let (_, validation) = post(&app, sk, "/v1/charts/royal/validate", None).await;
        assert_eq!(
            (
                &validation["valid"],
                &validation["findings"],
                &validation["stats"]["seats"]
            ),
            (&json!(true), &json!([]), &json!(2_000)),
        );
        let (response, problem) = post(&app, sk, "/v1/charts/royal/publish", Some("\"2\"")).await;
        assert_problem(&response, &problem, Code::VersionConflict);
        let (response, v1) = post(&app, sk, "/v1/charts/royal/publish", Some("\"3\"")).await;
        assert_eq!(response.status(), 201, "{v1}");
        assert_eq!(v1["number"], 1);
        let hash = v1["hash"].as_str().unwrap();
        assert!(v1["id"].as_str().unwrap().starts_with("cv_"));
        assert_eq!(v1["url"], format!("/v1/public/charts/{hash}.ccf"));
        assert_eq!(v1["stats"]["seats"], 2_000);

        // What is stored: both artefacts decode, the public one with the
        // document's seats, under its own content hash; brotli decodes to it;
        // the source is the draft that was compiled.
        let (public, br, full, full_hash, source): (Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>, String) =
            sqlx::query_as(
                "SELECT public_ccf, public_ccf_br, full_ccf, full_hash, source::text
                   FROM chart_versions WHERE number = 1 AND hash = decode($1, 'hex')",
            )
            .bind(hash)
            .fetch_one(&pool)
            .await
            .unwrap();
        let chart = cavea_format::decode(&public, Artefact::Public).unwrap();
        assert_eq!(chart.meta.seat_count, 2_000);
        assert_eq!(hex(&cavea_format::content_hash(&public).unwrap()), hash);
        assert_eq!(unbrotli(&br), public);
        let decoded_full = cavea_format::decode(&full, Artefact::Full).unwrap();
        assert!(decoded_full.full.is_some());
        assert_eq!(
            cavea_format::content_hash(&full).unwrap().as_slice(),
            full_hash
        );
        assert_eq!(
            ChartDocument::from_json(source.as_bytes()).unwrap(),
            document
        );
        let (_, chart) = call(&app, Method::GET, "/v1/charts/royal", Some(sk), None).await;
        assert_eq!(chart["published_version"], v1);

        // Unchanged: the same version again, nothing new.
        let (response, again) = post(&app, sk, "/v1/charts/royal/publish", None).await;
        assert_eq!(response.status(), 200, "{again}");
        assert_eq!(again, v1);

        // Changed: version 2 with another hash; version 1 is untouched (INV-12).
        let v1_row = version_row(&pool, 1).await;
        document.name = "Royal Theatre, 2027".to_owned();
        put_draft(&app, sk, "royal", Some("\"3\""), json_of(&document)).await;
        let (response, v2) = post(&app, sk, "/v1/charts/royal/publish", None).await;
        assert_eq!(response.status(), 201, "{v2}");
        assert_eq!(v2["number"], 2);
        assert_ne!(v2["hash"], v1["hash"]);
        assert_eq!(version_row(&pool, 1).await, v1_row);

        // A change only the full artefact sees (a section priority): a new
        // version whose public file, and so its hash and URL, is version 2's.
        first_section(&mut document).priority = 2.0;
        put_draft(&app, sk, "royal", Some("\"4\""), json_of(&document)).await;
        let (response, v3) = post(&app, sk, "/v1/charts/royal/publish", None).await;
        assert_eq!(response.status(), 201, "{v3}");
        assert_eq!((&v3["number"], &v3["hash"]), (&json!(3), &v2["hash"]));

        // Republishing an older draft points the chart back at its version.
        document.name = "Royal Theatre".to_owned();
        first_section(&mut document).priority = 1.0;
        put_draft(&app, sk, "royal", Some("\"5\""), json_of(&document)).await;
        let (response, back) = post(&app, sk, "/v1/charts/royal/publish", None).await;
        assert_eq!((response.status(), &back), (StatusCode::OK, &v1));
        let (_, chart) = call(&app, Method::GET, "/v1/charts/royal", Some(sk), None).await;
        assert_eq!(chart["published_version"]["number"], 1);

        // Versions, oldest first, in pages.
        let (_, page) = call(
            &app,
            Method::GET,
            "/v1/charts/royal/versions?limit=2",
            Some(sk),
            None,
        )
        .await;
        assert_eq!(page["data"], json!([v1, v2]));
        assert_eq!(page["next"], v2["id"]);
        let uri = format!(
            "/v1/charts/royal/versions?after={}",
            v2["id"].as_str().unwrap()
        );
        let (_, page) = call(&app, Method::GET, &uri, Some(sk), None).await;
        assert_eq!((&page["data"], &page["next"]), (&json!([v3]), &Value::Null));

        // Charts list without drafts; the cursor is a chart key.
        let (_, list) = call(&app, Method::GET, "/v1/charts?limit=1", Some(sk), None).await;
        assert_eq!(list["data"][0]["key"], t.chart.as_str());
        assert!(list["data"][0].get("draft").is_none());
        assert_eq!(list["next"], t.chart.as_str());
        let uri = format!("/v1/charts?after={}", t.chart);
        let (_, list) = call(&app, Method::GET, &uri, Some(sk), None).await;
        assert_eq!(list["data"][0]["key"], "royal");

        // Archive: idempotent; the draft is frozen, the key stays taken, the
        // versions and their public files remain.
        let (response, archived) =
            call(&app, Method::DELETE, "/v1/charts/royal", Some(sk), None).await;
        assert_eq!(response.status(), 200);
        assert!(archived["archived_at"].is_string());
        let (_, again) = call(&app, Method::DELETE, "/v1/charts/royal", Some(sk), None).await;
        assert_eq!(again["archived_at"], archived["archived_at"]);
        let (response, problem) = put_draft(&app, sk, "royal", Some("*"), json_of(&document)).await;
        assert_problem(&response, &problem, Code::InvalidRequest);
        assert_eq!(problem["details"]["reason"], "archived");
        let (response, problem) = post(&app, sk, "/v1/charts/royal/publish", None).await;
        assert_eq!(
            (response.status(), &problem["details"]["reason"]),
            (StatusCode::BAD_REQUEST, &json!("archived"))
        );
        let body = json!({ "key": "royal", "name": "Again" });
        let (response, problem) =
            call(&app, Method::POST, "/v1/charts", Some(sk), Some(body)).await;
        assert_problem(&response, &problem, Code::AlreadyExists);
        let (response, _) = fetch(&app, v3["url"].as_str().unwrap(), &[]).await;
        assert_eq!(response.status(), 200);

        // The publish log line: request id, workspace, sizes and timings;
        // never the document.
        let published: Vec<Value> = logs
            .records()
            .into_iter()
            .filter(|r| r["message"] == PUBLISHED)
            .collect();
        let created = published
            .iter()
            .find(|r| r["number"] == 2 && r["created"] == true)
            .unwrap();
        assert!(created["spans"][0]["request_id"].is_string(), "{created}");
        assert_eq!(created["spans"][0]["workspace"], t.id.as_str());
        for field in [
            "compile_ms",
            "compress_ms",
            "store_ms",
            "public_br_bytes",
            "seats",
        ] {
            assert!(created[field].is_number(), "{field}: {created}");
        }
        assert!(!logs.text().contains("premium"), "a document was logged");
        assert_no_secrets(&logs, &t.secrets());
    })
    .await;
}

/// GET a URL without a credential; the body as bytes.
async fn fetch(
    app: &Router,
    uri: &str,
    headers: &[(header::HeaderName, &str)],
) -> (Response<Body>, Vec<u8>) {
    fetch_with(app, Method::GET, uri, headers).await
}

async fn fetch_with(
    app: &Router,
    method: Method,
    uri: &str,
    headers: &[(header::HeaderName, &str)],
) -> (Response<Body>, Vec<u8>) {
    let response = app
        .clone()
        .oneshot(request(method, uri, None, headers, None))
        .await
        .unwrap();
    let (parts, body) = response.into_parts();
    let bytes = axum::body::to_bytes(body, 64 << 20).await.unwrap().to_vec();
    (Response::from_parts(parts, Body::empty()), bytes)
}

fn unbrotli(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    brotli::BrotliDecompress(&mut &bytes[..], &mut out).unwrap();
    out
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The compiled chart by its hash: headers, encodings, conditional requests,
/// what is not served, and the same file for every caller.
#[tokio::test]
#[ignore = "needs CAVEA_TEST_DATABASE_URL"]
async fn public_chart_route() {
    let _serial = serial();
    with_db(async move |pool| {
        let app = api(&pool, Some(ADMIN));
        let a = Tenant::create(&app, "a").await;
        let (public, full_hash): (Vec<u8>, Vec<u8>) = sqlx::query_as(
            "SELECT public_ccf, full_hash FROM chart_versions WHERE hash = decode($1, 'hex')",
        )
        .bind(&a.hash)
        .fetch_one(&pool)
        .await
        .unwrap();
        let url = format!("/v1/public/charts/{}.ccf", a.hash);

        // (Accept-Encoding, brotli chosen)
        let cases = [
            (Some("gzip, deflate, br, zstd"), true),
            (Some("br"), true),
            (Some("*"), true),
            (Some("gzip, br;q=0"), false),
            (Some("gzip"), false),
            (Some("identity"), false),
            (None, false),
        ];
        for (accept, brotli) in cases {
            let headers: Vec<_> = accept
                .map(|a| (header::ACCEPT_ENCODING, a))
                .into_iter()
                .collect();
            let (response, body) = fetch(&app, &url, &headers).await;
            assert_eq!(response.status(), 200, "{accept:?}");
            let h = response.headers();
            assert_eq!(h[header::CONTENT_TYPE], "application/vnd.cavea.ccf");
            assert_eq!(
                h[header::CACHE_CONTROL],
                "public, max-age=31536000, immutable"
            );
            assert_eq!(h[header::VARY], "Accept-Encoding");
            assert_eq!(h[header::ACCESS_CONTROL_ALLOW_ORIGIN], "*");
            assert_eq!(h[header::CONTENT_LENGTH], body.len().to_string().as_str());
            let (encoding, etag) = if brotli {
                (Some("br"), format!("\"{}.br\"", a.hash))
            } else {
                (None, format!("\"{}\"", a.hash))
            };
            assert_eq!(
                h.get(header::CONTENT_ENCODING).map(|v| v.to_str().unwrap()),
                encoding
            );
            assert_eq!(h[header::ETAG], etag.as_str());
            let decoded = if brotli { unbrotli(&body) } else { body };
            assert_eq!(decoded, public, "{accept:?}");

            // The same tag, weak or not, among others: 304 without a body.
            let tags = format!("\"x\", W/{etag}");
            let mut conditional = headers.clone();
            conditional.push((header::IF_NONE_MATCH, &tags));
            let (response, body) = fetch(&app, &url, &conditional).await;
            assert_eq!(response.status(), 304, "{accept:?}");
            assert!(body.is_empty());
            assert_eq!(response.headers()[header::ETAG], etag.as_str());
            assert_eq!(
                response.headers()[header::CACHE_CONTROL],
                "public, max-age=31536000, immutable"
            );
        }

        // `If-None-Match: *`: 304 when the file exists, 404 when not.
        let star = [(header::IF_NONE_MATCH, "*")];
        let (response, _) = fetch(&app, &url, &star).await;
        assert_eq!(response.status(), 304);
        let zeros = "0".repeat(32);
        let unknown = format!("/v1/public/charts/{zeros}.ccf");
        let (response, _) = fetch(&app, &unknown, &star).await;
        assert_eq!(response.status(), 404);
        // Nor does naming an unknown file's own tag make it exist.
        let tag = format!("\"{zeros}\"");
        let (response, _) = fetch(&app, &unknown, &[(header::IF_NONE_MATCH, &tag)]).await;
        assert_eq!(response.status(), 404);

        // HEAD: the headers of GET, no body.
        let (response, body) = fetch_with(&app, Method::HEAD, &url, &[]).await;
        assert_eq!(response.status(), 200);
        assert!(body.is_empty());
        assert_eq!(
            response.headers()[header::ETAG],
            format!("\"{}\"", a.hash).as_str()
        );

        // Not served: unknown hashes, the full artefact's hash, other forms,
        // other methods.
        let not_found = [
            format!("/v1/public/charts/{}.ccf", "0".repeat(32)),
            format!("/v1/public/charts/{}.ccf", hex(&full_hash)),
            format!("/v1/public/charts/{}.ccf", a.hash.to_uppercase()),
            format!("/v1/public/charts/{}", a.hash),
            format!("/v1/public/charts/{}.ccf", &a.hash[..31]),
            format!("/v1/public/charts/{}/versions/{}.ccf", a.chart, a.hash),
        ];
        for uri in &not_found {
            let (response, body) = fetch(&app, uri, &[]).await;
            assert_eq!(response.status(), 404, "{uri}");
            let problem: Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(problem["code"], "not_found");
        }
        let (response, _) = fetch_with(&app, Method::POST, &url, &[]).await;
        assert_eq!(response.status(), 404);

        // Another workspace publishing the same document has the same file
        // at the same URL; archiving a chart does not withdraw it.
        let b = Tenant::create(&app, "b").await;
        let body = json!({ "key": "copy", "name": "copy",
                           "document": serde_json::from_str::<Value>(&a.document).unwrap() });
        call(
            &app,
            Method::POST,
            "/v1/charts",
            Some(&b.secret),
            Some(body),
        )
        .await;
        let (_, version) = post(&app, &b.secret, "/v1/charts/copy/publish", None).await;
        assert_eq!(version["url"], url.as_str());
        let uri = format!("/v1/charts/{}", a.chart);
        call(&app, Method::DELETE, &uri, Some(&a.secret), None).await;
        let (response, body) = fetch(&app, &url, &[]).await;
        assert_eq!((response.status(), body), (StatusCode::OK, public));
    })
    .await;
}

/// Publishes of one chart racing each other, with saves between rounds:
/// versions are numbered 1, 2, 3 without gaps or duplicates, and each round
/// creates exactly one.
#[tokio::test]
#[ignore = "needs CAVEA_TEST_DATABASE_URL"]
async fn concurrent_publishes() {
    let _serial = serial();
    with_db(async move |pool| {
        let app = api(&pool, Some(ADMIN));
        let t = Tenant::create(&app, "race").await;
        let mut document = stadium(500, 3);
        for round in 1..=3 {
            document.name = format!("round {round}");
            let (response, _) =
                put_draft(&app, &t.secret, &t.chart, Some("*"), json_of(&document)).await;
            assert_eq!(response.status(), 200);
            let mut racers = tokio::task::JoinSet::new();
            for _ in 0..4 {
                let app = app.clone();
                let uri = format!("/v1/charts/{}/publish", t.chart);
                let secret = t.secret.clone();
                racers.spawn(async move { post(&app, &secret, &uri, None).await });
            }
            let mut created = 0;
            while let Some(result) = racers.join_next().await {
                let (response, version) = result.unwrap();
                assert!(matches!(response.status().as_u16(), 200 | 201), "{version}");
                created += usize::from(response.status() == 201);
                // The fixture is version 1.
                assert_eq!(version["number"], round + 1, "{version}");
            }
            assert_eq!(created, 1, "round {round}");
        }
        let numbers: Vec<i32> =
            sqlx::query_scalar("SELECT number FROM chart_versions ORDER BY number")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(numbers, [1, 2, 3, 4]);
    })
    .await;
}

/// Drafts that are too large, too deep, not chart documents or not storable
/// are refused before anything is stored; one that expands past the object
/// limit is saved (work in progress) but fails validation and publishing
/// quickly, without expanding.
#[tokio::test]
#[ignore = "needs CAVEA_TEST_DATABASE_URL"]
async fn hostile_drafts() {
    let _serial = serial();
    with_db(async move |pool| {
        let app = api(&pool, Some(ADMIN));
        let t = Tenant::create(&app, "h").await;
        let sk = t.secret.as_str();
        let chart = t.chart.as_str();
        let huge_name = "x".repeat(crate::charts::MAX_DRAFT_BYTES);
        let deep = format!("{}{}", "[".repeat(40), "]".repeat(40));
        // (body, reason)
        let refused = [
            (
                format!(r#"{{"schema":1,"name":"{huge_name}","nextOid":1}}"#),
                "body_too_large",
            ),
            (
                format!(r#"{{"schema":1,"name":"x","nextOid":1,"nodes":{deep}}}"#),
                "json_depth",
            ),
            (
                r#"{"schema":1,"name":"x","nextOid":1,"colour":"red"}"#.to_owned(),
                "document",
            ),
            (
                r#"{"schema":2,"name":"x","nextOid":1}"#.to_owned(),
                "document",
            ),
            (
                r#"{"schema":1,"name":"x\u0000","nextOid":1}"#.to_owned(),
                "document",
            ),
            (r#"[]"#.to_owned(), "document"),
            (r#"{"schema":1,"#.to_owned(), "document"),
        ];
        for (body, reason) in refused {
            let (response, problem) = put_draft(&app, sk, chart, Some("*"), body.clone()).await;
            assert_problem(&response, &problem, Code::InvalidRequest);
            assert_eq!(problem["details"]["reason"], reason, "{:.60}", body);
            // The same document at creation.
            if let Ok(document) = serde_json::from_str::<Value>(&body) {
                let create = json!({ "key": "new", "name": "n", "document": document });
                let (response, problem) =
                    call(&app, Method::POST, "/v1/charts", Some(sk), Some(create)).await;
                assert_problem(&response, &problem, Code::InvalidRequest);
                assert_eq!(problem["details"]["reason"], reason, "create {:.60}", body);
            }
        }
        let (_, state) = call(
            &app,
            Method::GET,
            &format!("/v1/charts/{chart}"),
            Some(sk),
            None,
        )
        .await;
        assert_eq!(state["draft_etag"], "\"1\"", "nothing was stored");

        // 300 rows of 4,095 seats: past the 1,000,000-object limit.
        let mut document = stadium(500, 1);
        let mut next = document.next_oid;
        let Some(Node::Section(section)) = document.nodes.first_mut() else {
            panic!()
        };
        let Some(Node::RowBlock(block)) = section.children.first_mut() else {
            panic!()
        };
        for _ in 0..300 {
            block.rows.push(BlockRow {
                rid: next,
                seats: 4_095,
                offset: 0.0,
                oids: vec![[next + 1, 4_095]],
                label: None,
                removed: Vec::new(),
            });
            next += 4_096;
        }
        document.next_oid = next;
        let (response, _) = put_draft(&app, sk, chart, Some("*"), json_of(&document)).await;
        assert_eq!(response.status(), 200);
        let started = Instant::now();
        let (_, validation) = post(&app, sk, &format!("/v1/charts/{chart}/validate"), None).await;
        let is_limit = |findings: &Value| {
            findings
                .as_array()
                .unwrap()
                .iter()
                .any(|f| f["code"] == "E006")
        };
        assert!(is_limit(&validation["findings"]), "{validation}");
        let (response, problem) =
            post(&app, sk, &format!("/v1/charts/{chart}/publish"), None).await;
        assert_problem(&response, &problem, Code::ChartInvalid);
        assert!(is_limit(&problem["details"]["findings"]), "{problem}");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
    })
    .await;
}
