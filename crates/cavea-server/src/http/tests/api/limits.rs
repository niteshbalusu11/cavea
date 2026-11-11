//! Rate limits on the public endpoints, per publishable key and client
//! address, behind trusted proxies or not (08 §12).

use std::net::SocketAddr;

use axum::extract::ConnectInfo;
use cavea_engine::{ManualClock, Timestamp};

use super::*;
use crate::limiter::Limiter;

/// The real routes on a manual clock, with these limits and trusted proxies.
fn limited(pool: &PgPool, clock: &ManualClock, holds: u32, proxies: &str) -> Router {
    let mut state = state();
    state.db = pool.clone();
    let config = Arc::make_mut(&mut state.config);
    config.admin_key = Some(ADMIN.parse().unwrap());
    config.trusted_proxies = proxies.parse().unwrap();
    state.seats = Seats::new(
        pool.clone(),
        &state.config,
        ServerClock::Manual(clock.clone()),
        None,
    );
    state.limiter = Arc::new(Limiter::new(holds, 3_000));
    finish(routes().into_router(), state)
}

const XFF: header::HeaderName = header::HeaderName::from_static("x-forwarded-for");

/// A buyer's release of everything `token` holds, from the socket `peer`.
async fn release(
    app: &Router,
    t: &Tenant,
    key: &str,
    token: &str,
    peer: &str,
    forwarded: Option<&str>,
) -> (Response<Body>, Value) {
    let uri = format!("/v1/public/events/{}/release", t.event);
    let headers: Vec<_> = forwarded.map(|f| (XFF, f)).into_iter().collect();
    let body = json!({ "hold_token": token }).to_string();
    let mut request = request(Method::POST, &uri, Some(key), &headers, Some(body));
    let peer = SocketAddr::new(peer.parse().unwrap(), 40_000);
    request.extensions_mut().insert(ConnectInfo(peer));
    send(app.clone(), request).await
}

/// The 121st hold-budget call in a minute from one key and address is `429`
/// with `Retry-After` and CORS headers; another address, another key, the
/// reads budget and secret keys are unaffected, and the bucket refills.
#[tokio::test]
#[ignore = "needs CAVEA_TEST_DATABASE_URL"]
async fn public_calls_are_limited_per_key_and_address() {
    let _serial = serial();
    with_db(async move |pool| {
        let clock = ManualClock::new(Timestamp(1_800_000_000_000));
        let app = limited(&pool, &clock, 120, "");
        let t = Tenant::create(&app, "a").await;
        let token = super::realtime::public_token(&app, &t, &t.event).await;
        let pk = t.publishable.as_str();
        let a = "198.51.100.1";
        for i in 0..120 {
            let (response, body) = release(&app, &t, pk, &token, a, None).await;
            assert_eq!(response.status(), 200, "{i}: {body}");
        }
        let (response, problem) = release(&app, &t, pk, &token, a, None).await;
        assert_problem(&response, &problem, Code::RateLimited);
        let headers = response.headers();
        assert_eq!(headers[header::RETRY_AFTER], "1");
        assert_eq!(headers[header::ACCESS_CONTROL_ALLOW_ORIGIN], "*");
        assert!(
            headers[header::ACCESS_CONTROL_EXPOSE_HEADERS]
                .to_str()
                .unwrap()
                .contains("Retry-After")
        );

        // Another address; another of the workspace's keys; a read.
        let (response, _) = release(&app, &t, pk, &token, "198.51.100.2", None).await;
        assert_eq!(response.status(), 200);
        let body = json!({ "kind": "publishable", "name": "second" });
        let (_, second) = call(&app, Method::POST, "/v1/keys", Some(&t.secret), Some(body)).await;
        let second = second["key"].as_str().unwrap();
        let (response, _) = release(&app, &t, second, &token, a, None).await;
        assert_eq!(response.status(), 200);
        let uri = format!("/v1/public/events/{}", t.event);
        let (response, _) = call(&app, Method::GET, &uri, Some(pk), None).await;
        assert_eq!(response.status(), 200);
        // A secret key is not limited.
        let uri = format!("/v1/events/{}/release", t.event);
        for _ in 0..130 {
            let body = json!({ "hold_token": token });
            let (response, _) = call(&app, Method::POST, &uri, Some(&t.secret), Some(body)).await;
            assert_eq!(response.status(), 200);
        }

        // One more every 500 ms.
        clock.advance_ms(499);
        let (response, _) = release(&app, &t, pk, &token, a, None).await;
        assert_eq!(response.status(), 429);
        clock.advance_ms(1);
        let (response, _) = release(&app, &t, pk, &token, a, None).await;
        assert_eq!(response.status(), 200);
        let (response, _) = release(&app, &t, pk, &token, a, None).await;
        assert_eq!(response.status(), 429);

        // Turned off.
        let open = limited(&pool, &clock, 0, "");
        for _ in 0..130 {
            let (response, _) = release(&open, &t, pk, &token, a, None).await;
            assert_eq!(response.status(), 200);
        }
    })
    .await;
}

/// Behind a trusted proxy the forwarded address is counted; behind any other
/// peer, `X-Forwarded-For` is ignored and the peer is counted.
#[tokio::test]
#[ignore = "needs CAVEA_TEST_DATABASE_URL"]
async fn the_forwarded_address_counts_only_behind_a_trusted_proxy() {
    let _serial = serial();
    with_db(async move |pool| {
        let clock = ManualClock::new(Timestamp(1_800_000_000_000));
        let app = limited(&pool, &clock, 2, "10.0.0.0/8, 2001:db8:a::/48");
        let t = Tenant::create(&app, "a").await;
        let token = super::realtime::public_token(&app, &t, &t.event).await;
        let pk = t.publishable.as_str();
        let status = async |peer: &str, forwarded: Option<&str>| {
            release(&app, &t, pk, &token, peer, forwarded)
                .await
                .0
                .status()
                .as_u16()
        };
        // Through the proxy, as the buyer at 198.51.100.7 (who sent a header
        // of their own): two, then limited.
        let chain = "6.6.6.6, 198.51.100.7";
        assert_eq!(status("10.0.0.5", Some(chain)).await, 200);
        assert_eq!(status("10.0.0.6", Some("198.51.100.7")).await, 200);
        assert_eq!(status("10.0.0.5", Some(chain)).await, 429);
        // Another buyer through the same proxy, and one through two proxies.
        assert_eq!(status("10.0.0.5", Some("198.51.100.8")).await, 200);
        assert_eq!(
            status("2001:db8:a::1", Some("198.51.100.9, 10.3.3.3")).await,
            200
        );
        // The same header from a peer that is not trusted: the peer counts.
        assert_eq!(status("203.0.113.1", Some("198.51.100.7")).await, 200);
        assert_eq!(status("203.0.113.1", Some("198.51.100.8")).await, 200);
        assert_eq!(status("203.0.113.1", Some("198.51.100.9")).await, 429);
        // One IPv6 client is its /64.
        assert_eq!(status("2001:db8:b:1::1", None).await, 200);
        assert_eq!(status("2001:db8:b:1::2", None).await, 200);
        assert_eq!(status("2001:db8:b:1:ffff::3", None).await, 429);
        assert_eq!(status("2001:db8:b:2::1", None).await, 200);
    })
    .await;
}
