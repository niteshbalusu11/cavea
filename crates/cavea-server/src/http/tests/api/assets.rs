//! Assets against PostgreSQL (08 §6.2): an upload is decided by its bytes,
//! refused past its size while it streams, stored per workspace, and served by
//! its hash to anyone, harmlessly.

use std::sync::Arc;

use crate::assets::tests::{jpeg, png, webp_lossy};

use super::*;

async fn upload(
    app: &Router,
    key: &str,
    content_type: Option<&str>,
    body: Body,
) -> (Response<Body>, Value) {
    let mut request =
        Request::post("/v1/assets").header(header::AUTHORIZATION, format!("Bearer {key}"));
    if let Some(content_type) = content_type {
        request = request.header(header::CONTENT_TYPE, content_type);
    }
    send(app.clone(), request.body(body).unwrap()).await
}

fn hex(bytes: &[u8]) -> String {
    let hash = crate::assets::hash(bytes);
    hash.iter().map(|b| format!("{b:02x}")).collect()
}

#[tokio::test]
#[ignore = "needs CAVEA_TEST_DATABASE_URL"]
async fn assets_are_sniffed_stored_and_served() {
    let _serial = serial();
    with_db(async move |pool| {
        let app = api(&pool, Some(ADMIN));
        let a = Tenant::create(&app, "a").await;
        let b = Tenant::create(&app, "b").await;

        // The type is the bytes', whatever the header says.
        let cases = [
            (png(640, 480), Some("image/jpeg"), "image/png", (640, 480)),
            (
                jpeg(4000, 3000),
                Some("application/octet-stream"),
                "image/jpeg",
                (4000, 3000),
            ),
            (webp_lossy(300, 200), None, "image/webp", (300, 200)),
        ];
        for (bytes, header, content_type, (width, height)) in &cases {
            let (response, asset) =
                upload(&app, &a.secret, *header, Body::from(bytes.clone())).await;
            assert_eq!(response.status(), 201, "{asset}");
            let hash = hex(bytes);
            assert_eq!(
                asset,
                json!({
                    "hash": hash, "content_type": content_type, "size": bytes.len(),
                    "width": width, "height": height, "url": format!("/v1/public/assets/{hash}"),
                })
            );
            // Again: the same asset.
            let (response, again) =
                upload(&app, &a.secret, *header, Body::from(bytes.clone())).await;
            assert_eq!((response.status().as_u16(), &again), (200, &asset));
        }

        // Refused, whatever the header says; nothing is stored.
        let mut polyglot = png(10, 10);
        polyglot.extend_from_slice(b"<html><script>alert(1)</script>");
        let full = png(10, 10);
        let refusals: [(Vec<u8>, &str); 4] = [
            (
                br#"<svg xmlns="http://www.w3.org/2000/svg"><script>alert(1)</script></svg>"#
                    .to_vec(),
                "asset_type",
            ),
            (polyglot, "asset_malformed"),
            (full[..full.len() - 4].to_vec(), "asset_malformed"),
            (png(8_193, 10), "asset_dimensions"),
        ];
        for (bytes, reason) in refusals {
            let (response, problem) =
                upload(&app, &a.secret, Some("image/png"), Body::from(bytes)).await;
            assert_problem(&response, &problem, Code::InvalidRequest);
            assert_eq!(problem["details"]["reason"], reason);
        }

        // Over the limit: refused from `Content-Length`, and while streaming
        // when there is none, before more than the limit is read.
        let limit = 10 << 20;
        let mut big = png(10, 10);
        big.resize(limit + 1, 0);
        let (response, problem) = upload(&app, &a.secret, None, Body::from(big)).await;
        assert_problem(&response, &problem, Code::InvalidRequest);
        assert_eq!(
            (&problem["details"]["reason"], &problem["details"]["limit"]),
            (&json!("body_too_large"), &json!(limit))
        );
        let read = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = read.clone();
        let chunks = (0..20).map(move |_| {
            counted.fetch_add(1 << 20, std::sync::atomic::Ordering::Relaxed);
            Ok::<_, std::io::Error>(axum::body::Bytes::from(vec![0u8; 1 << 20]))
        });
        let streamed = Body::from_stream(futures_util::stream::iter(chunks));
        let (response, problem) = upload(&app, &a.secret, None, streamed).await;
        assert_problem(&response, &problem, Code::InvalidRequest);
        assert_eq!(problem["details"]["reason"], "body_too_large");
        assert_eq!(read.load(std::sync::atomic::Ordering::Relaxed), 11 << 20);
        let stored: i64 = sqlx::query_scalar("SELECT count(*) FROM assets")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(stored, 3);

        // Another workspace's upload of the same bytes is its own asset.
        let bytes = &cases[0].0;
        let (response, _) = upload(&app, &b.secret, None, Body::from(bytes.clone())).await;
        assert_eq!(response.status(), 201);
        let rows: i64 =
            sqlx::query_scalar("SELECT count(*) FROM assets WHERE hash = decode($1, 'hex')")
                .bind(hex(bytes))
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(rows, 2);

        // Served to anyone with the hash, harmlessly and for a year.
        let hash = hex(bytes);
        let uri = format!("/v1/public/assets/{hash}");
        let request = Request::get(&uri)
            .header(header::ORIGIN, "https://elsewhere.example")
            .body(Body::empty())
            .unwrap();
        let ((response, _), served) = send_raw(app.clone(), request).await;
        assert_eq!(response.status(), 200);
        assert_eq!(&served[..], &bytes[..]);
        let headers = response.headers();
        for (name, value) in [
            (header::CONTENT_TYPE, "image/png".to_owned()),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_owned()),
            (
                header::CONTENT_SECURITY_POLICY,
                "default-src 'none'; sandbox".to_owned(),
            ),
            (
                header::CONTENT_DISPOSITION,
                format!("inline; filename=\"{hash}.png\""),
            ),
            (
                header::CACHE_CONTROL,
                "public, max-age=31536000, immutable".to_owned(),
            ),
            (header::ETAG, format!("\"{hash}\"")),
            (header::ACCESS_CONTROL_ALLOW_ORIGIN, "*".to_owned()),
        ] {
            assert_eq!(headers[&name], value.as_str(), "{name}");
        }
        assert!(headers.get(header::VARY).is_none());
        let request = Request::get(&uri)
            .header(header::IF_NONE_MATCH, format!("\"{hash}\""))
            .body(Body::empty())
            .unwrap();
        let (response, _) = send(app.clone(), request).await;
        assert_eq!(response.status(), 304);
        for missing in [
            format!("/v1/public/assets/{}", "0".repeat(32)),
            "/v1/public/assets/x.png".to_owned(),
        ] {
            let (response, problem) = call(&app, Method::GET, &missing, None, None).await;
            assert_problem(&response, &problem, Code::NotFound);
            assert_eq!(response.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN], "*");
        }
    })
    .await;
}
