//! The realtime routes against PostgreSQL, the real registry and chart
//! provider, over a listening socket where the transport needs one (08 §8).

use std::net::SocketAddr;
use std::time::Duration;

use cavea_format::wire::{self, CloseReason, Subscribe, types};
use futures_util::{SinkExt as _, StreamExt as _};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio::time::{Instant, timeout};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
use tokio_tungstenite::tungstenite::{self, Message};

use super::*;
use crate::config::CorsOrigins;
use crate::realtime::{Realtime, Timing, tests::Client};

/// A server on a socket, sharing its state with `app` (for HTTP calls through
/// the router).
struct Server {
    addr: SocketAddr,
    app: Router,
    seats: Seats,
    realtime: Realtime,
    stop: Option<oneshot::Sender<()>>,
    done: tokio::task::JoinHandle<std::io::Result<()>>,
}

/// The protocol's timers, with a handshake short enough to wait out.
const QUICK_HANDSHAKE: Timing = Timing {
    ping: Duration::from_secs(25),
    idle: Duration::from_secs(60),
    handshake: Duration::from_millis(300),
    stall: Duration::from_secs(30),
};

async fn start(pool: &PgPool, timing: Timing, per_ip: u32, cors: CorsOrigins) -> Server {
    let mut state = state();
    state.db = pool.clone();
    let config = Arc::make_mut(&mut state.config);
    config.admin_key = Some(ADMIN.parse().unwrap());
    config.cors_origins = cors;
    state.seats = Seats::new(pool.clone(), &state.config, ServerClock::System);
    state.realtime = Realtime::new(100, per_ip, timing);
    let (seats, realtime) = (state.seats.clone(), state.realtime.clone());
    let app = finish(routes(), state.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (stop, stopped) = oneshot::channel();
    let done = tokio::spawn(crate::serve(listener, state, async move {
        let _ = stopped.await;
    }));
    Server {
        addr,
        app,
        seats,
        realtime,
        stop: Some(stop),
        done,
    }
}

impl Server {
    async fn shutdown(mut self) -> Duration {
        let started = Instant::now();
        let _ = self.stop.take().unwrap().send(());
        timeout(Duration::from_secs(25), self.done)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        started.elapsed()
    }

    /// Every connection has closed and left no subscriber behind.
    async fn settled(&self, event: EventId) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while self.realtime.connections() > 0 || self.seats.registry.subscribers(event) > 0 {
            assert!(
                Instant::now() < deadline,
                "a connection or subscriber leaked"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}

async fn event_id(pool: &PgPool, key: &str) -> EventId {
    let id: String =
        sqlx::query_scalar("SELECT replace(id::text, '-', '') FROM events WHERE key = $1")
            .bind(key)
            .fetch_one(pool)
            .await
            .unwrap();
    crate::seats::event_id(&id).unwrap()
}

type Ws = WebSocketStream<TcpStream>;

/// Open `/v1/public/rt`, or the status the upgrade was refused with.
async fn connect(addr: SocketAddr, key: Option<&str>, origin: Option<&str>) -> Result<Ws, u16> {
    let query = key.map(|k| format!("?key={k}")).unwrap_or_default();
    let mut request = format!("ws://{addr}/v1/public/rt{query}")
        .into_client_request()
        .unwrap();
    if let Some(origin) = origin {
        request
            .headers_mut()
            .insert(header::ORIGIN, origin.parse().unwrap());
    }
    let stream = TcpStream::connect(addr).await.unwrap();
    match tokio_tungstenite::client_async(request, stream).await {
        Ok((ws, _)) => Ok(ws),
        Err(tungstenite::Error::Http(response)) => Err(response.status().as_u16()),
        Err(e) => panic!("{e}"),
    }
}

fn subscribe_frame(event: &str, last_seq: u64, token: Option<&str>) -> Vec<u8> {
    wire::encode_subscribe(&Subscribe {
        version: wire::PROTOCOL_VERSION,
        event_key: event,
        last_seq,
        hold_token: token.map(str::as_bytes),
        session_token: None,
    })
    .unwrap()
}

async fn send(ws: &mut Ws, frame: Vec<u8>) {
    ws.send(Message::Binary(frame.into())).await.unwrap();
}

/// The next binary message, or `None` once the connection has closed.
async fn next(ws: &mut Ws) -> Option<Vec<u8>> {
    loop {
        match timeout(Duration::from_secs(5), ws.next()).await.unwrap() {
            Some(Ok(Message::Binary(bytes))) => return Some(bytes.to_vec()),
            Some(Ok(Message::Close(_)) | Err(_)) | None => return None,
            Some(Ok(_)) => {}
        }
    }
}

/// Subscribe and apply frames until the client has `seq` (and PRIVATE, if it
/// gave a token).
async fn follow(addr: SocketAddr, t: &Tenant, last_seq: u64, token: Option<&str>) -> (Ws, Client) {
    let mut ws = connect(addr, Some(&t.publishable), None).await.unwrap();
    send(&mut ws, subscribe_frame(&t.event, last_seq, token)).await;
    let mut client = Client::default();
    client.apply(&next(&mut ws).await.unwrap());
    (ws, client)
}

async fn read_until(ws: &mut Ws, client: &mut Client, done: impl Fn(&Client) -> bool) {
    while !done(client) {
        client.apply(&next(ws).await.expect("a frame"));
    }
}

/// Every frame left until the server closes the connection.
async fn rest(ws: &mut Ws) -> Vec<Vec<u8>> {
    let mut frames = Vec::new();
    while let Some(frame) = next(ws).await {
        frames.push(frame);
    }
    frames
}

/// The HTTP snapshot as a client state.
async fn http_state(app: &Router, t: &Tenant) -> Client {
    let uri = format!("/v1/public/events/{}/state?key={}", t.event, t.publishable);
    let response = app.clone().oneshot(get_req(&uri)).await.unwrap();
    assert_eq!(response.status(), 200);
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .unwrap();
    let mut client = Client::default();
    let objects = match wire::decode_server(&bytes, wire::View::Public).unwrap() {
        wire::ServerFrame::Snapshot(s) => s.objects,
        _ => panic!("not a snapshot"),
    };
    client.codes = vec![0; objects as usize];
    client.apply(&bytes);
    client
}

async fn public_hold(app: &Router, t: &Tenant, token: &str, idx: &[u32]) -> u64 {
    let body = json!({ "hold_token": token, "objects": { "version": t.hash, "idx": idx } });
    let uri = format!("/v1/public/events/{}/hold", t.event);
    let (response, held) = call(app, Method::POST, &uri, Some(&t.publishable), Some(body)).await;
    assert_eq!(response.status(), 200, "{held}");
    held["seq"].as_u64().unwrap()
}

async fn public_token(app: &Router, t: &Tenant, event: &str) -> String {
    let body = json!({ "event": event });
    let (response, token) = call(
        app,
        Method::POST,
        "/v1/public/hold-tokens",
        Some(&t.publishable),
        Some(body),
    )
    .await;
    assert_eq!(response.status(), 201, "{token}");
    token["hold_token"].as_str().unwrap().to_owned()
}

fn hash_bytes(hex: &str) -> [u8; 16] {
    let mut out = [0; 16];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap();
    }
    out
}

/// HELLO carries the chart's hash; a hold arrives as a DELTA, and as PRIVATE
/// for its token's subscriber only; resuming gets only the missing DELTAs; a
/// token that is not this event's, or an event that is not this key's, is
/// refused alike.
#[tokio::test]
#[ignore = "needs CAVEA_TEST_DATABASE_URL"]
async fn websocket_follows_the_event() {
    let _serial = serial();
    let logs = Captured::default();
    let _guard = logs.install();
    let logs_in = logs.clone();
    with_db(async move |pool| {
        let logs = logs_in;
        let server = start(&pool, Timing::default(), 10, CorsOrigins::Any).await;
        let (app, addr) = (&server.app, server.addr);
        let a = Tenant::create(app, "a").await;
        let b = Tenant::create(app, "b").await;
        let id = event_id(&pool, &a.event).await;

        let (mut watcher, mut public) = follow(addr, &a, 0, None).await;
        let hello = public.hello.unwrap();
        assert_eq!(hello.chart_hash, hash_bytes(&a.hash));
        let snapshot = http_state(app, &a).await;
        assert_eq!(hello.objects as usize, snapshot.codes.len());
        read_until(&mut watcher, &mut public, |c| c.count(types::SNAPSHOT) == 1).await;
        let start_seq = public.seq;

        let token = public_token(app, &a, &a.event).await;
        let (mut buyer, mut mine) = follow(addr, &a, 0, Some(&token)).await;
        read_until(&mut buyer, &mut mine, |c| c.private.is_some()).await;
        assert_eq!(mine.types, [types::HELLO, types::SNAPSHOT, types::PRIVATE]);

        let seq = public_hold(app, &a, &token, &[5, 6]).await;
        read_until(&mut watcher, &mut public, |c| c.seq == seq).await;
        read_until(&mut buyer, &mut mine, |c| {
            c.seq == seq && c.private.as_ref().is_some_and(|p| !p.1.is_empty())
        })
        .await;
        assert_eq!(mine.types.last(), Some(&types::PRIVATE));
        assert_eq!(mine.types[mine.types.len() - 2], types::DELTA);
        assert_eq!(mine.private.as_ref().unwrap().1, [5, 6]);
        assert_eq!(public.count(types::PRIVATE), 0);
        assert_eq!((public.codes[5], public.codes[6]), (1, 1));
        let committed = http_state(app, &a).await;
        assert_eq!(
            (&public.codes, public.seq),
            (&committed.codes, committed.seq)
        );
        assert_eq!(server.realtime.subscribers(), 2);

        // Resuming from a buffered seq: only the DELTAs since.
        let (mut resumed, mut client) = follow(addr, &a, start_seq, None).await;
        client.codes.clone_from(&snapshot.codes);
        client.seq = start_seq;
        read_until(&mut resumed, &mut client, |c| c.seq == seq).await;
        assert_eq!(client.count(types::SNAPSHOT), 0);
        assert!(client.count(types::DELTA) >= 1);
        assert_eq!(client.codes, committed.codes);
        // A seq the server never published: the SNAPSHOT.
        let (mut stale, mut client) = follow(addr, &a, 1_000_000, None).await;
        read_until(&mut stale, &mut client, |c| c.seq == seq).await;
        assert_eq!(client.types, [types::HELLO, types::SNAPSHOT]);

        // Refused alike: another workspace's token, another event's, a secret
        // key's unlimited one, a token that never existed, another
        // workspace's event, an unknown event.
        let body = json!({ "key": "second", "chart": a.chart });
        let (response, _) =
            call(app, Method::POST, "/v1/events", Some(&a.secret), Some(body)).await;
        assert_eq!(response.status(), 201);
        let other_event = public_token(app, &a, "second").await;
        let never = format!("ht_{}", "0".repeat(64));
        let refused = [
            (a.event.as_str(), Some(b.token.as_str())),
            (&a.event, Some(&other_event)),
            (&a.event, Some(&a.token)),
            (&a.event, Some(&never)),
            (&a.event, Some("not-a-token")),
            (&b.event, None),
            ("no-such-event", None),
        ];
        for (event, token) in refused {
            let mut ws = connect(addr, Some(&a.publishable), None).await.unwrap();
            send(&mut ws, subscribe_frame(event, 0, token)).await;
            assert_eq!(
                rest(&mut ws).await,
                [wire::encode_close(CloseReason::NOT_FOUND)]
            );
        }

        drop((watcher, buyer, resumed, stale));
        server.settled(id).await;
        assert_eq!(server.realtime.subscribers(), 0);
        let closes: Vec<Value> = logs
            .records()
            .into_iter()
            .filter(|r| r["message"] == "realtime connection closed")
            .collect();
        assert_eq!(closes.len(), 4 + refused.len());
        let span = &closes[0]["spans"][0];
        assert_eq!(span["name"], "realtime");
        assert!(span["request_id"].as_str().unwrap().starts_with("req_"));
        assert_eq!(span["workspace"], a.id);
        let secrets: Vec<&str> = a
            .secrets()
            .into_iter()
            .chain(b.secrets())
            .chain([token.as_str(), other_event.as_str()])
            .collect();
        assert_no_secrets(&logs, &secrets);
        server.shutdown().await;
    })
    .await;
}

/// T11: the upgrade checks the key and both origin gates; the first frame
/// must be a valid SUBSCRIBE in time; afterwards the client may only PONG.
/// Each refusal closes the connection and leaves nothing behind.
#[tokio::test]
#[ignore = "needs CAVEA_TEST_DATABASE_URL"]
async fn websocket_refuses_what_breaks_the_rules() {
    let _serial = serial();
    with_db(async move |pool| {
        let allowed = "https://tickets.example.com";
        let cors = format!("{allowed},https://other.example.com")
            .parse()
            .unwrap();
        let server = start(&pool, QUICK_HANDSHAKE, 3, cors).await;
        let (app, addr) = (&server.app, server.addr);
        let t = Tenant::create(app, "a").await;
        let id = event_id(&pool, &t.event).await;
        let body = json!({ "kind": "publishable", "name": "site", "allowed_origins": [allowed] });
        let (response, key) =
            call(app, Method::POST, "/v1/keys", Some(&t.secret), Some(body)).await;
        assert_eq!(response.status(), 201);
        let site = key["key"].as_str().unwrap();

        assert_eq!(connect(addr, None, None).await.err(), Some(401));
        assert_eq!(connect(addr, Some(&t.secret), None).await.err(), Some(403));
        assert_eq!(
            connect(addr, Some(site), Some("https://other.example.com"))
                .await
                .err(),
            Some(403)
        );
        assert_eq!(
            connect(addr, Some(&t.publishable), Some("https://evil.example"))
                .await
                .err(),
            Some(403)
        );
        let ws = connect(addr, Some(site), Some(allowed)).await.unwrap();
        drop(ws);

        let subscribe = subscribe_frame(&t.event, 0, None);
        let mut version_0 = subscribe.clone();
        version_0[1] = 0;
        let binary = |bytes: Vec<u8>| Some(Message::Binary(bytes.into()));
        let cases = [
            ("silent", None, None),
            (
                "malformed",
                binary(vec![0x10, 1, 0xff]),
                Some(CloseReason::BAD_FRAME),
            ),
            (
                "version 0",
                binary(version_0),
                Some(CloseReason::UNSUPPORTED_VERSION),
            ),
            (
                "text",
                Some(Message::Text("hi".into())),
                Some(CloseReason::BAD_FRAME),
            ),
            (
                "pong first",
                binary(wire::encode_pong(1)),
                Some(CloseReason::BAD_FRAME),
            ),
            (
                "oversized",
                binary(vec![0x10; 4096]),
                Some(CloseReason::BAD_FRAME),
            ),
        ];
        for (case, message, reason) in cases {
            let mut ws = connect(addr, Some(&t.publishable), None).await.unwrap();
            if let Some(message) = message {
                let _ = ws.send(message).await;
            }
            let frames = rest(&mut ws).await;
            let close = reason.map(wire::encode_close);
            assert_eq!(frames, Vec::from_iter(close), "{case}");
        }

        // After SUBSCRIBE: a second SUBSCRIBE or text is CLOSE 6.
        for second in [
            Message::Binary(subscribe.clone().into()),
            Message::Text("hi".into()),
        ] {
            let mut ws = connect(addr, Some(&t.publishable), None).await.unwrap();
            send(&mut ws, subscribe.clone()).await;
            let mut client = Client::default();
            read_until(&mut ws, &mut client, |c| c.count(types::SNAPSHOT) == 1).await;
            ws.send(second).await.unwrap();
            let frames = rest(&mut ws).await;
            assert_eq!(
                frames.last(),
                Some(&wire::encode_close(CloseReason::BAD_FRAME))
            );
        }
        server.settled(id).await;

        // At most 3 connections from one address.
        let mut open = Vec::new();
        for _ in 0..3 {
            open.push(connect(addr, Some(&t.publishable), None).await.unwrap());
        }
        assert_eq!(
            connect(addr, Some(&t.publishable), None).await.err(),
            Some(429)
        );
        drop(open);
        server.settled(id).await;
        server.shutdown().await;
    })
    .await;
}

/// The snapshot is the engine's frame, with its `seq` as `ETag`, conditional
/// requests and cache headers; an unknown or foreign event is `404`, never
/// cached; an archived one is still served.
#[tokio::test]
#[ignore = "needs CAVEA_TEST_DATABASE_URL"]
async fn snapshot() {
    let _serial = serial();
    with_db(async move |pool| {
        let (app, seats) = api_with(&pool, Some(ADMIN), ServerClock::System, "test");
        let a = Tenant::create(&app, "a").await;
        let b = Tenant::create(&app, "b").await;
        let id = event_id(&pool, &a.event).await;
        let get = async |uri: &str, headers: &[(header::HeaderName, &str)]| {
            let response = app
                .clone()
                .oneshot(request(Method::GET, uri, None, headers, None))
                .await
                .unwrap();
            let (parts, body) = response.into_parts();
            (parts, axum::body::to_bytes(body, 1 << 20).await.unwrap())
        };
        let state = format!("/v1/public/events/{}/state?key={}", a.event, a.publishable);
        let cache = "public, max-age=1, stale-while-revalidate=5";

        let (parts, body) = get(&state, &[]).await;
        let frame = seats.registry.snapshot(id).await.unwrap();
        assert_eq!(parts.status, 200);
        assert_eq!(&body[..], frame.bytes());
        assert_eq!(
            parts.headers[header::CONTENT_TYPE],
            "application/vnd.cavea.frame"
        );
        let etag = format!("\"{}\"", frame.seq());
        assert_eq!(parts.headers[header::ETAG], etag.as_str());
        assert_eq!(parts.headers[header::CACHE_CONTROL], cache);
        assert_eq!(parts.headers[header::ACCESS_CONTROL_ALLOW_ORIGIN], "*");
        for tag in [etag.clone(), format!("W/{etag}"), "*".to_owned()] {
            let (parts, body) = get(&state, &[(header::IF_NONE_MATCH, &tag)]).await;
            assert_eq!(parts.status, 304, "{tag}");
            assert!(body.is_empty());
            assert_eq!(parts.headers[header::ETAG], etag.as_str());
            assert_eq!(parts.headers[header::CACHE_CONTROL], cache);
        }
        let (parts, _) = get(&state, &[(header::IF_NONE_MATCH, "\"0\"")]).await;
        assert_eq!(parts.status, 200);

        // Not found, and not cached.
        for uri in [
            format!("/v1/public/events/nope/state?key={}", a.publishable),
            format!("/v1/public/events/{}/state?key={}", a.event, b.publishable),
        ] {
            let (parts, _) = get(&uri, &[]).await;
            assert_eq!(parts.status, 404, "{uri}");
            assert_eq!(parts.headers[header::CACHE_CONTROL], "no-store");
        }
        // The key goes in the URL; a header is not read here.
        let uri = format!("/v1/public/events/{}/state", a.event);
        let (response, problem) = call(&app, Method::GET, &uri, Some(&a.publishable), None).await;
        assert_problem(&response, &problem, Code::Unauthorized);

        let uri = format!("/v1/events/{}", a.event);
        let (response, _) = call(&app, Method::DELETE, &uri, Some(&a.secret), None).await;
        assert_eq!(response.status(), 200);
        let (parts, _) = get(&state, &[]).await;
        assert_eq!(parts.status, 200);
        seats.shutdown().await;
    })
    .await;
}

/// On shutdown every WebSocket subscriber gets CLOSE 3 and its connection is
/// closed at once; the server exits well inside the deadline.
#[tokio::test]
#[ignore = "needs CAVEA_TEST_DATABASE_URL"]
async fn shutdown_closes_every_subscriber() {
    let _serial = serial();
    with_db(async move |pool| {
        let server = start(&pool, Timing::default(), 10, CorsOrigins::Any).await;
        let (app, addr) = (&server.app, server.addr);
        let t = Tenant::create(app, "a").await;

        let mut sockets = Vec::new();
        for _ in 0..3 {
            let (mut ws, mut client) = follow(addr, &t, 0, None).await;
            read_until(&mut ws, &mut client, |c| c.count(types::SNAPSHOT) == 1).await;
            sockets.push(ws);
        }
        let realtime = server.realtime.clone();
        let took = server.shutdown().await;
        assert!(took < Duration::from_secs(3), "{took:?}");
        assert_eq!(realtime.connections(), 0);
        let close = wire::encode_close(CloseReason::SHUTTING_DOWN);
        for mut ws in sockets {
            assert_eq!(rest(&mut ws).await.last(), Some(&close));
        }
    })
    .await;
}
