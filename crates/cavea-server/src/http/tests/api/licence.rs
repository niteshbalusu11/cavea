//! The licence against PostgreSQL (12 §7.3, D-20, D-80, D-81).
//!
//! LIC-005: every seat operation, public, secret-key and console, answers
//! exactly as with a valid licence in every state a licence can be in.
//! Then what the state does change: the console's chart and event writes
//! while expired, what `me` and the public event endpoint say, the upload,
//! and the state moving with the clock.

use std::net::SocketAddr;

use cavea_format::wire::{self, ServerFrame, Subscribe, View};
use cavea_licence::sign::SigningKey;
use cavea_licence::{DAY_MS, Date, Verifier};
use futures_util::{SinkExt as _, StreamExt as _};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;

use super::*;
use crate::licence::{Found, LicenceSource, Licences, State as LicenceState};
use crate::realtime::tests::Client;

fn now_ms() -> u64 {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap()
}

/// Today plus `n` days, as `YYYY-MM-DD`.
fn day(n: i64) -> String {
    let days = i64::from(Date::of_ms(now_ms()).days()) + n;
    Date::of_ms(u64::try_from(days).unwrap() * DAY_MS).to_string()
}

/// A customer and plan the public endpoint must never show.
const CUSTOMER: &str = "Undisclosed Customer Ltd";

/// A licence issued `issued` days from today, valid through `expires` days
/// from today, with 30 days of grace.
fn licence(key: &SigningKey, id: &str, issued: i64, expires: i64, domains: &[&str]) -> Vec<u8> {
    let payload = json!({
        "licence_id": id, "customer": CUSTOMER, "plan": "growth",
        "features": ["embed"], "domains": domains, "max_workspaces": 3,
        "issued_at": day(issued), "expires_at": day(expires), "grace_days": 30,
    });
    key.issue(payload.to_string().as_bytes()).unwrap().1
}

const DOMAINS: [&str; 2] = ["tickets.example.com", "*.example.org"];

/// The real routes on `clock`, with `licences`.
fn app_with(pool: &PgPool, clock: ServerClock, licences: Licences) -> (Router, AppState) {
    let mut state = state();
    state.db = pool.clone();
    let config = Arc::make_mut(&mut state.config);
    config.admin_key = Some(ADMIN.parse().unwrap());
    state.seats = Seats::new(pool.clone(), &state.config, clock, None);
    state.licences = licences;
    (finish(routes().into_router(), state.clone()), state)
}

/// [`app_with`] on the system clock, also served on a socket (for the
/// WebSocket); stopped with [`Node::stop`].
struct Node {
    addr: SocketAddr,
    app: Router,
    stop: oneshot::Sender<()>,
    done: tokio::task::JoinHandle<std::io::Result<()>>,
}

impl Node {
    async fn start(pool: &PgPool, licences: Licences) -> Self {
        let (app, state) = app_with(pool, ServerClock::System, licences);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (stop, stopped) = oneshot::channel();
        let done = tokio::spawn(crate::serve(listener, state, async move {
            let _ = stopped.await;
        }));
        Self {
            addr,
            app,
            stop,
            done,
        }
    }

    async fn stop(self) {
        let _ = self.stop.send(());
        self.done.await.unwrap().unwrap();
    }
}

/// `licence` of `me` for this console session.
async fn me_licence(app: &Router, id: &SessionId) -> Value {
    let headers = console(id, None);
    let (response, me) = call_with(
        app,
        Method::GET,
        "/console/api/me",
        None,
        &refs(&headers),
        None,
    )
    .await;
    assert_eq!(response.status(), 200, "{me}");
    me["licence"].clone()
}

/// Every string with one of `ids` in it has it replaced by a placeholder,
/// every time by `<time>` and every hold token by `<hold token>`: what
/// differs between two tenants doing the same thing.
fn normalised(value: &Value, ids: &[(&str, &str)]) -> Value {
    match value {
        Value::String(s) => {
            let mut s = s.clone();
            for (id, name) in ids {
                s = s.replace(id, name);
            }
            let time = s.len() >= 20 && s.as_bytes()[4] == b'-' && s.as_bytes()[10] == b'T';
            Value::String(if time {
                "<time>".to_owned()
            } else if s.starts_with("ht_") {
                "<hold token>".to_owned()
            } else {
                s
            })
        }
        Value::Array(items) => Value::Array(items.iter().map(|v| normalised(v, ids)).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .filter(|(k, _)| k.as_str() != "request_id")
                .map(|(k, v)| (k.clone(), normalised(v, ids)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// A frame body (snapshot or delta) as the objects' public codes and seq.
fn frames(bytes: &[u8]) -> Value {
    let objects = match wire::decode_server(bytes, View::Public).unwrap() {
        ServerFrame::Snapshot(s) => s.objects,
        _ => return json!({ "frame": bytes.first() }),
    };
    let mut client = Client {
        codes: vec![0; objects as usize],
        ..Client::default()
    };
    client.apply(bytes);
    json!({ "seq": client.seq, "codes": client.codes })
}

/// The first snapshot a WebSocket subscriber of `t`'s event receives.
async fn websocket(addr: SocketAddr, t: &Tenant) -> Value {
    let request = format!("ws://{addr}/v1/public/rt?key={}", t.publishable)
        .into_client_request()
        .unwrap();
    let stream = TcpStream::connect(addr).await.unwrap();
    let (mut ws, response) = tokio_tungstenite::client_async(request, stream)
        .await
        .unwrap();
    assert_eq!(response.status(), 101);
    let subscribe = wire::encode_subscribe(&Subscribe {
        version: wire::PROTOCOL_VERSION,
        event_key: &t.event,
        last_seq: 0,
        hold_token: None,
        session_token: None,
    })
    .unwrap();
    ws.send(Message::Binary(subscribe.into())).await.unwrap();
    loop {
        let message = tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        if let Message::Binary(bytes) = message
            && matches!(
                wire::decode_server(&bytes, View::Public),
                Ok(ServerFrame::Snapshot(_))
            )
        {
            let _ = ws.close(None).await;
            return frames(&bytes);
        }
    }
}

/// Every seat operation once, in order, on `t`'s event: each step's name,
/// status and normalised answer. The public event's `licence` is left out
/// (it is the one thing that differs, by design: `public_event_discloses`).
async fn seat_operations(
    node: &Node,
    t: &Tenant,
    session: &SessionId,
) -> Vec<(String, u16, Value)> {
    let app = &node.app;
    let e = &t.event;
    let mut steps = Vec::new();
    let ids = [(t.hash.as_str(), "<hash>")];
    let cookie = console(session, Some(&t.id));
    let as_member = refs(&cookie);
    let objects = |idx: &[u32]| json!({ "version": t.hash, "idx": idx });

    macro_rules! step {
        ($name:expr, $method:expr, $uri:expr, $token:expr, $headers:expr, $body:expr) => {{
            let (response, answer) = {
                let uri: String = $uri.into();
                call_with(app, $method, &uri, $token, $headers, $body).await
            };
            let mut shown = normalised(&answer, &ids);
            if let Some(object) = shown.as_object_mut() {
                object.remove("licence");
            }
            steps.push(($name.to_owned(), response.status().as_u16(), shown));
            answer
        }};
    }

    let pk = Some(t.publishable.as_str());
    let sk = Some(t.secret.as_str());
    let made = step!(
        "public token",
        Method::POST,
        "/v1/public/hold-tokens",
        pk,
        &[],
        Some(json!({ "event": e }))
    );
    let public = made["hold_token"].as_str().unwrap().to_owned();
    step!(
        "public token read",
        Method::GET,
        "/v1/public/hold-tokens/current",
        pk,
        &[(HOLD_TOKEN, public.as_str())],
        None
    );
    step!(
        "public hold",
        Method::POST,
        format!("/v1/public/events/{e}/hold"),
        pk,
        &[],
        Some(json!({ "hold_token": public, "objects": objects(&[0, 1]) }))
    );
    step!(
        "public release",
        Method::POST,
        format!("/v1/public/events/{e}/release"),
        pk,
        &[],
        Some(json!({ "hold_token": public, "objects": objects(&[1]) }))
    );
    step!(
        "public event",
        Method::GET,
        format!("/v1/public/events/{e}"),
        pk,
        &[],
        None
    );
    let made = step!(
        "secret token",
        Method::POST,
        "/v1/hold-tokens",
        sk,
        &[],
        Some(json!({ "event": e }))
    );
    let secret = made["hold_token"].as_str().unwrap().to_owned();
    step!(
        "secret token read",
        Method::GET,
        format!("/v1/hold-tokens/{secret}"),
        sk,
        &[],
        None
    );
    step!(
        "extend",
        Method::POST,
        format!("/v1/hold-tokens/{secret}/extend"),
        sk,
        &[],
        Some(json!({}))
    );
    step!(
        "secret hold",
        Method::POST,
        format!("/v1/events/{e}/hold"),
        sk,
        &[],
        Some(json!({ "hold_token": secret, "objects": objects(&[2, 3, 4]) }))
    );
    step!(
        "release a held seat",
        Method::POST,
        format!("/v1/events/{e}/release"),
        sk,
        &[],
        Some(json!({ "hold_token": secret, "objects": objects(&[4]) }))
    );
    step!(
        "book by token",
        Method::POST,
        format!("/v1/events/{e}/book"),
        sk,
        &[],
        Some(json!({ "hold_token": secret, "order_ref": "o-1" }))
    );
    step!(
        "book directly",
        Method::POST,
        format!("/v1/events/{e}/book"),
        sk,
        &[],
        Some(json!({ "objects": objects(&[10, 11]), "order_ref": "o-2" }))
    );
    step!(
        "release booked",
        Method::POST,
        format!("/v1/events/{e}/release"),
        sk,
        &[],
        Some(json!({ "objects": objects(&[11]) }))
    );
    step!(
        "change status",
        Method::POST,
        format!("/v1/events/{e}/change-status"),
        sk,
        &[],
        Some(json!({ "objects": objects(&[12]), "status": "blocked" }))
    );
    step!(
        "delete the public token",
        Method::DELETE,
        format!("/v1/hold-tokens/{public}"),
        sk,
        &[],
        None
    );
    let made = step!(
        "console token",
        Method::POST,
        "/v1/hold-tokens",
        None,
        &as_member,
        Some(json!({ "event": e }))
    );
    let manager = made["hold_token"].as_str().unwrap().to_owned();
    step!(
        "console hold",
        Method::POST,
        format!("/v1/events/{e}/hold"),
        None,
        &as_member,
        Some(json!({ "hold_token": manager, "objects": objects(&[20]) }))
    );
    step!(
        "console book",
        Method::POST,
        format!("/v1/events/{e}/book"),
        None,
        &as_member,
        Some(json!({ "objects": objects(&[21]), "order_ref": "o-3" }))
    );
    step!(
        "console change status",
        Method::POST,
        format!("/v1/events/{e}/change-status"),
        None,
        &as_member,
        Some(json!({ "objects": objects(&[22]), "status": "blocked" }))
    );
    let last = step!(
        "console release",
        Method::POST,
        format!("/v1/events/{e}/release"),
        None,
        &as_member,
        Some(json!({ "objects": objects(&[21]) }))
    );
    // The public state follows commits within a publication interval: wait
    // for the last one, so that every run reads the same snapshot.
    let committed = last["seq"].as_u64().unwrap_or_else(|| panic!("{last}"));
    let snapshot = format!("/v1/public/events/{e}/state?key={}", t.publishable);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let ((_, _), bytes) = send_raw(app.clone(), get_req(&snapshot)).await;
        if frames(&bytes)["seq"] == committed {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "never published");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    step!(
        "objects",
        Method::GET,
        format!("/v1/events/{e}/objects?status=booked&limit=10"),
        sk,
        &[],
        None
    );
    // The realtime routes: the snapshot and a poll, the stream's head, and a
    // WebSocket's first snapshot.
    for (name, uri) in [
        (
            "snapshot",
            format!("/v1/public/events/{e}/state?key={}", t.publishable),
        ),
        (
            "poll",
            format!(
                "/v1/public/events/{e}/state?after_seq=1&key={}",
                t.publishable
            ),
        ),
        (
            "stream",
            format!("/v1/public/events/{e}/stream?key={}", t.publishable),
        ),
    ] {
        let ((response, _), bytes) = send_raw(app.clone(), get_req(&uri)).await;
        let status = response.status().as_u16();
        let shown = if bytes.is_empty() {
            Value::Null
        } else {
            frames(&bytes)
        };
        steps.push((name.to_owned(), status, shown));
    }
    steps.push(("websocket".to_owned(), 101, websocket(node.addr, t).await));
    steps
}

/// How a server's licence is set up for one row of the matrix.
enum Setup {
    /// `CAVEA_LICENCE` holds the file.
    Contents(Vec<u8>),
    /// No licence anywhere.
    Nothing,
    /// `CAVEA_LICENCE` names a file holding the first, replaced with the
    /// second while the server runs.
    Swapped(Vec<u8>, Vec<u8>),
    /// Stored as uploaded, without the upload route's check.
    Stored(Vec<u8>),
}

/// LIC-005 (D-20): every seat operation answers exactly as with a valid
/// licence, in every state: valid, in grace, expired years ago, missing,
/// malformed, signed by another key, issued in the future, for other
/// domains, a valid file replaced by garbage at run time, garbage in the
/// database. Each state gets a server of its own and a workspace of its own
/// (same event key, same chart shape), so that answers compare as equal.
#[tokio::test]
#[ignore = "needs CAVEA_TEST_DATABASE_URL"]
async fn seat_operations_ignore_the_licence() {
    let _serial = serial();
    with_db(async move |pool| {
        let key = SigningKey::generate().unwrap().0;
        let stranger = SigningKey::generate().unwrap().0;
        let verifier = Verifier::new([key.public()]);
        let valid = licence(&key, "lic_valid", -10, 300, &DOMAINS);
        let garbage: Vec<u8> = (0..=255u8).cycle().take(4_000).collect();
        let dir = std::env::temp_dir().join(format!("cavea-licence-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let swapped = dir.join("licence.json");
        let rows: Vec<(&str, Setup, LicenceState)> = vec![
            ("valid", Setup::Contents(valid.clone()), LicenceState::Valid),
            (
                "grace",
                Setup::Contents(licence(&key, "lic_grace", -400, -5, &DOMAINS)),
                LicenceState::Grace,
            ),
            (
                "expired years ago",
                Setup::Contents(licence(&key, "lic_old", -4_000, -3_000, &DOMAINS)),
                LicenceState::Expired,
            ),
            ("missing", Setup::Nothing, LicenceState::Missing),
            (
                "malformed",
                Setup::Contents(valid[..valid.len() / 2].to_vec()),
                LicenceState::Invalid,
            ),
            (
                "signed by another key",
                Setup::Contents(licence(&stranger, "lic_forged", -10, 300, &DOMAINS)),
                LicenceState::Invalid,
            ),
            (
                "issued in the future",
                Setup::Contents(licence(&key, "lic_future", 1_000, 2_000, &DOMAINS)),
                LicenceState::Valid,
            ),
            (
                "for another domain",
                Setup::Contents(licence(
                    &key,
                    "lic_elsewhere",
                    -10,
                    300,
                    &["elsewhere.example"],
                )),
                LicenceState::Valid,
            ),
            (
                "garbage swapped in at run time",
                Setup::Swapped(valid.clone(), garbage.clone()),
                LicenceState::Invalid,
            ),
            // Last: the row stays in the database.
            (
                "garbage in the database",
                Setup::Stored(garbage.clone()),
                LicenceState::Invalid,
            ),
        ];
        let mut reference: Option<Vec<(String, u16, Value)>> = None;
        for (n, (name, setup, expected)) in rows.into_iter().enumerate() {
            let source = match &setup {
                Setup::Contents(bytes) => Some(LicenceSource::Contents(bytes.clone())),
                Setup::Swapped(first, _) => {
                    std::fs::write(&swapped, first).unwrap();
                    Some(LicenceSource::Path(swapped.clone()))
                }
                Setup::Nothing | Setup::Stored(_) => None,
            };
            if let Setup::Stored(bytes) = &setup {
                sqlx::query(
                    "INSERT INTO licence (id, document, uploaded_by) VALUES (1, $1, 'test')",
                )
                .bind(bytes)
                .execute(&pool)
                .await
                .unwrap();
            }
            let licences = Licences::new(verifier.clone(), source);
            licences.refresh(&pool, now_ms()).await;
            if let Setup::Swapped(_, second) = &setup {
                assert_eq!(licences.status(now_ms()).state, LicenceState::Valid);
                std::fs::write(&swapped, second).unwrap();
                licences.refresh(&pool, now_ms()).await;
            }
            assert_eq!(licences.status(now_ms()).state, expected, "{name}");
            let node = Node::start(&pool, licences).await;
            let t = Tenant::create_as(&node.app, &format!("w{n}"), "x").await;
            let (_, session) = member(&pool, &t.id, Role::Owner, &format!("o{n}@a.example")).await;
            assert_eq!(
                me_licence(&node.app, &session).await["state"],
                expected.as_str(),
                "{name}"
            );
            let steps = seat_operations(&node, &t, &session).await;
            for (step, status, answer) in &steps {
                assert!(
                    (200..300).contains(status) || *status == 101,
                    "{name}: {step} answered {status}: {answer}"
                );
            }
            match &reference {
                None => reference = Some(steps),
                Some(valid) => {
                    assert_eq!(valid.len(), steps.len());
                    for (a, b) in valid.iter().zip(&steps) {
                        assert_eq!(a, b, "{name} differs from a valid licence");
                    }
                }
            }
            node.stop().await;
        }
        std::fs::remove_dir_all(&dir).unwrap();
    })
    .await;
}

/// While expired, a console session may not write charts or events (`403`
/// `licence_expired`), and may do everything else: seat operations (above),
/// keys, members, the workspace's name, sign-in, a password change, and
/// uploading a licence, after which writing works at once. A secret key is
/// never refused. In grace, missing or invalid, nothing is refused.
#[tokio::test]
#[ignore = "needs CAVEA_TEST_DATABASE_URL"]
async fn expired_console_is_read_only_but_for_seats() {
    let _serial = serial();
    with_db(async move |pool| {
        let key = SigningKey::generate().unwrap().0;
        let expired = licence(&key, "lic_expired", -800, -400, &DOMAINS);
        let licences = Licences::new(
            Verifier::new([key.public()]),
            Some(LicenceSource::Contents(expired)),
        );
        let (app, _) = app_with(&pool, ServerClock::System, licences.clone());
        let t = Tenant::create(&app, "a").await;
        let (_, session) = member(&pool, &t.id, Role::Owner, "owner@a.example").await;
        let cookie = console(&session, Some(&t.id));
        let headers = refs(&cookie);
        let png = crate::assets::tests::png(64, 48);
        let writes: Vec<(Method, String, Option<Value>)> = vec![
            (
                Method::POST,
                "/v1/charts".into(),
                Some(json!({ "key": "new", "name": "n" })),
            ),
            (
                Method::PUT,
                format!("/v1/charts/{}/draft", t.chart),
                Some(serde_json::from_str(&t.document).unwrap()),
            ),
            (
                Method::POST,
                format!("/v1/charts/{}/validate", t.chart),
                None,
            ),
            (
                Method::POST,
                format!("/v1/charts/{}/publish", t.chart),
                None,
            ),
            (
                Method::POST,
                format!("/v1/charts/{}/lock", t.chart),
                Some(json!({})),
            ),
            (
                Method::POST,
                "/v1/events".into(),
                Some(json!({ "key": "tomorrow", "chart": t.chart })),
            ),
            (
                Method::PATCH,
                format!("/v1/events/{}", t.event),
                Some(json!({ "name": "renamed" })),
            ),
            (Method::DELETE, format!("/v1/events/{}", t.event), None),
            (Method::DELETE, format!("/v1/charts/{}", t.chart), None),
        ];
        for (method, uri, body) in &writes {
            let (response, answer) =
                call_with(&app, method.clone(), uri, None, &headers, body.clone()).await;
            assert_problem(&response, &answer, Code::Forbidden);
            assert_eq!(
                answer["details"]["reason"], "licence_expired",
                "{method} {uri}"
            );
        }
        let upload = Request::builder()
            .method(Method::POST)
            .uri("/v1/assets")
            .header(header::CONTENT_TYPE, "image/png");
        let upload = cookie
            .iter()
            .fold(upload, |r, (n, v)| r.header(n, v))
            .body(Body::from(png.clone()))
            .unwrap();
        let (response, answer) = send(app.clone(), upload).await;
        assert_eq!(answer["details"]["reason"], "licence_expired");
        assert_eq!(response.status(), 403);
        // Reads go on.
        for uri in ["/v1/charts".into(), format!("/v1/events/{}", t.event)] {
            let (response, _) = call_with(&app, Method::GET, &uri, None, &headers, None).await;
            assert_eq!(response.status(), 200, "{uri}");
        }
        // Keys, members, the workspace's name.
        for (method, uri, body, status) in [
            (
                Method::POST,
                "/v1/keys",
                json!({ "kind": "secret", "name": "rotated" }),
                201,
            ),
            (
                Method::POST,
                "/console/api/members",
                json!({ "email": "new@a.example", "role": "viewer" }),
                201,
            ),
            (
                Method::PATCH,
                "/console/api/workspace",
                json!({ "name": "A" }),
                200,
            ),
        ] {
            let (response, answer) = call_with(&app, method, uri, None, &headers, Some(body)).await;
            assert_eq!(response.status(), status, "{uri}: {answer}");
        }
        // The secret key writes everything (a backend creating tomorrow's
        // event is selling).
        let (response, answer) = call(
            &app,
            Method::POST,
            "/v1/events",
            Some(&t.secret),
            Some(json!({ "key": "tomorrow", "chart": t.chart })),
        )
        .await;
        assert_eq!(response.status(), 201, "{answer}");
        // Sign-in and a password change.
        let (response, signed) = call(
            &app,
            Method::POST,
            "/console/api/session",
            None,
            Some(json!({ "email": "owner@a.example", "password": PASSWORD })),
        )
        .await;
        assert_eq!(response.status(), 200, "{signed}");
        assert_eq!(signed["licence"]["state"], "expired");
        assert_eq!(signed["licence"]["read_only"], true);
        let session = session_of(&pool, &signed["user"]["id"].as_str().unwrap()[4..]).await;
        let cookie = console(&session, None);
        let (response, changed) = call_with(
            &app,
            Method::POST,
            "/console/api/password",
            None,
            &refs(&cookie),
            Some(json!({ "current_password": PASSWORD, "new_password": PASSWORD })),
        )
        .await;
        assert_eq!(response.status(), 200, "{changed}");
        // The owner (of every workspace there is) uploads a renewal: writes
        // work again at once.
        let session = session_of(&pool, &signed["user"]["id"].as_str().unwrap()[4..]).await;
        let cookie = console(&session, Some(&t.id));
        let renewal = licence(&key, "lic_renewed", -1, 365, &DOMAINS);
        let (response, uploaded) = call_with(
            &app,
            Method::PUT,
            "/console/api/licence",
            None,
            &refs(&cookie),
            Some(json!({ "licence": String::from_utf8(renewal).unwrap() })),
        )
        .await;
        assert_eq!(response.status(), 200, "{uploaded}");
        assert_eq!(uploaded["state"], "valid");
        assert_eq!(uploaded["read_only"], false);
        assert_eq!(uploaded["source"], "upload");
        assert_eq!(uploaded["licence"]["id"], "lic_renewed");
        let (response, answer) = call_with(
            &app,
            Method::POST,
            "/v1/charts",
            None,
            &refs(&cookie),
            Some(json!({ "key": "new", "name": "n" })),
        )
        .await;
        assert_eq!(response.status(), 201, "{answer}");
        // Grace, missing and invalid refuse nothing.
        for found in [
            Found::of(
                licences.verifier(),
                &licence(&key, "lic_grace", -400, -5, &DOMAINS),
            ),
            Found::Missing,
            Found::Invalid("signature"),
        ] {
            let others = Licences::new(licences.verifier().clone(), None);
            others.set_upload(found, now_ms());
            let (app, _) = app_with(&pool, ServerClock::System, others);
            let (response, answer) = call_with(
                &app,
                Method::POST,
                "/v1/events",
                None,
                &refs(&cookie),
                Some(json!({ "key": format!("e-{}", now_ms()), "chart": t.chart })),
            )
            .await;
            assert_eq!(response.status(), 201, "{answer}");
        }
    })
    .await;
}

/// The public event endpoint carries `licence` as `{status, domains}` and
/// nothing else of it; `me` shows the state to everyone and the details only
/// to an owner of every workspace, who alone may upload.
#[tokio::test]
#[ignore = "needs CAVEA_TEST_DATABASE_URL"]
async fn what_each_one_is_told() {
    let _serial = serial();
    with_db(async move |pool| {
        let key = SigningKey::generate().unwrap().0;
        let verifier = Verifier::new([key.public()]);
        let cases = [
            (
                licence(&key, "lic_valid", -10, 300, &DOMAINS),
                json!({ "status": "licensed", "domains": DOMAINS }),
            ),
            (
                licence(&key, "lic_grace", -400, -5, &DOMAINS),
                json!({ "status": "licensed", "domains": DOMAINS }),
            ),
            (
                licence(&key, "lic_expired", -800, -400, &DOMAINS),
                json!({ "status": "expired", "domains": [] }),
            ),
            (
                licence(&SigningKey::generate().unwrap().0, "lic_forged", -10, 300, &DOMAINS),
                json!({ "status": "unlicensed", "domains": [] }),
            ),
        ];
        for (n, (file, public)) in cases.into_iter().enumerate() {
            let licences = Licences::new(verifier.clone(), Some(LicenceSource::Contents(file)));
            let (app, _) = app_with(&pool, ServerClock::System, licences);
            let t = Tenant::create(&app, &format!("p{n}")).await;
            let uri = format!("/v1/public/events/{}", t.event);
            let (response, body) = call(&app, Method::GET, &uri, Some(&t.publishable), None).await;
            assert_eq!(response.status(), 200);
            assert_eq!(body["licence"], public);
            let text = body.to_string();
            for secret in [CUSTOMER, "growth", "lic_", "embed", "grace", "max_workspaces"] {
                assert!(!text.contains(secret), "{secret} disclosed");
            }
        }
        let missing = Licences::new(verifier.clone(), None);
        let (app, _) = app_with(&pool, ServerClock::System, missing);
        let t = Tenant::create(&app, "m").await;
        let uri = format!("/v1/public/events/{}", t.event);
        let (_, body) = call(&app, Method::GET, &uri, Some(&t.publishable), None).await;
        assert_eq!(body["licence"], json!({ "status": "unlicensed", "domains": [] }));

        // `me`: workspaces m, p0..p3 exist. An owner of one sees the state only
        // and may not upload; an owner of every one sees it all.
        let grace = licence(&key, "lic_grace", -400, -5, &DOMAINS);
        let licences = Licences::new(verifier.clone(), Some(LicenceSource::Contents(grace)));
        let (app, _) = app_with(&pool, ServerClock::System, licences);
        let (one, session) = member(&pool, &t.id, Role::Owner, "one@a.example").await;
        let seen = me_licence(&app, &session).await;
        assert_eq!(
            seen,
            json!({
                "state": "grace", "read_only": false, "grace_until": day(25),
                "manage": false, "source": null, "reason": null, "licence": null,
            })
        );
        let file = String::from_utf8(licence(&key, "lic_new", -1, 365, &DOMAINS)).unwrap();
        let upload = async |session: &SessionId, file: &str| {
            let cookie = console(session, None);
            call_with(
                &app,
                Method::PUT,
                "/console/api/licence",
                None,
                &refs(&cookie),
                Some(json!({ "licence": file })),
            )
            .await
        };
        let (response, answer) = upload(&session, &file).await;
        assert_problem(&response, &answer, Code::Forbidden);
        assert_eq!(answer["details"]["reason"], "instance_owner");
        // Made owner of every workspace.
        sqlx::query(
            "INSERT INTO memberships (user_id, workspace_id, role)
             SELECT $1::uuid, id, 'owner' FROM workspaces ON CONFLICT (user_id, workspace_id) DO NOTHING",
        )
        .bind(&one)
        .execute(&pool)
        .await
        .unwrap();
        let seen = me_licence(&app, &session).await;
        assert_eq!(seen["manage"], true);
        assert_eq!(seen["source"], "setting");
        assert_eq!(
            seen["licence"],
            json!({
                "id": "lic_grace", "customer": CUSTOMER, "plan": "growth",
                "features": ["embed"], "domains": DOMAINS, "max_workspaces": 3,
                "issued_at": day(-400), "expires_at": day(-5), "grace_days": 30,
                "key_id": key.public().id(),
            })
        );
        // Not a licence, or not one this build trusts: refused, nothing stored.
        let forged = licence(&SigningKey::generate().unwrap().0, "lic_x", -1, 365, &DOMAINS);
        for (bad, problem) in [
            ("{}".to_owned(), "malformed"),
            ("x".repeat(20_000), "too_large"),
            (String::from_utf8(forged).unwrap(), "unknown_key"),
        ] {
            let (response, answer) = upload(&session, &bad).await;
            assert_problem(&response, &answer, Code::InvalidRequest);
            assert_eq!(answer["details"]["field"], "licence");
            assert_eq!(answer["details"]["problem"], problem);
        }
        let stored: i64 = sqlx::query_scalar("SELECT count(*) FROM licence")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(stored, 0);
        let (response, answer) = upload(&session, &file).await;
        assert_eq!(response.status(), 200, "{answer}");
        assert_eq!(answer["licence"]["id"], "lic_new");
        let (action, target, actor): (String, String, String) =
            sqlx::query_as("SELECT action, target, actor FROM audit_log WHERE action = 'licence.uploaded'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!((action.as_str(), target.as_str()), ("licence.uploaded", "lic_new"));
        assert_eq!(actor, format!("usr_{one}"));

        // Another node on the same database: the upload counts there after
        // its next read (at most `REFRESH` later), not before.
        let other = Licences::new(verifier.clone(), None);
        assert_eq!(other.status(now_ms()).state, LicenceState::Missing);
        other.refresh(&pool, now_ms()).await;
        let status = other.status(now_ms());
        assert_eq!(status.state, LicenceState::Valid);
        assert_eq!(status.licence.unwrap().id, "lic_new");
    })
    .await;
}

/// The state follows the engine's clock across expiry and the end of grace,
/// with no restart or refresh: `me`, the public endpoint and the refusal
/// change together.
#[tokio::test]
#[ignore = "needs CAVEA_TEST_DATABASE_URL"]
async fn the_state_moves_with_the_clock() {
    let _serial = serial();
    with_db(async move |pool| {
        let key = SigningKey::generate().unwrap().0;
        let file = licence(&key, "lic_1", -10, 0, &DOMAINS);
        let licences = Licences::new(
            Verifier::new([key.public()]),
            Some(LicenceSource::Contents(file)),
        );
        let start = Date::parse(&day(0)).unwrap().start_ms();
        let clock = cavea_engine::ManualClock::new(cavea_engine::Timestamp(start));
        let (app, _) = app_with(&pool, ServerClock::Manual(clock.clone()), licences);
        let t = Tenant::create(&app, "a").await;
        let (_, session) = member(&pool, &t.id, Role::Admin, "admin@a.example").await;
        let cookie = console(&session, Some(&t.id));
        let uri = format!("/v1/public/events/{}", t.event);
        for (at, state, public) in [
            (start + DAY_MS - 1, "valid", "licensed"),
            (start + DAY_MS, "grace", "licensed"),
            (start + 31 * DAY_MS - 1, "grace", "licensed"),
            (start + 31 * DAY_MS, "expired", "expired"),
        ] {
            clock.set(cavea_engine::Timestamp(at));
            assert_eq!(me_licence(&app, &session).await["state"], state);
            let (_, body) = call(&app, Method::GET, &uri, Some(&t.publishable), None).await;
            assert_eq!(body["licence"]["status"], public);
            let (response, answer) = call_with(
                &app,
                Method::PATCH,
                &format!("/v1/events/{}", t.event),
                None,
                &refs(&cookie),
                Some(json!({ "name": state })),
            )
            .await;
            let refused = answer["details"]["reason"] == "licence_expired";
            assert_eq!(refused, state == "expired", "{state}: {answer}");
            assert_eq!(response.status() == 200, state != "expired");
        }
    })
    .await;
}
