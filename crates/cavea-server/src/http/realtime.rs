//! The realtime routes (08 §6.2, §8): the public snapshot and polling, the
//! WebSocket and the SSE stream. Each takes the publishable key in the `key`
//! query parameter ([`QueryKey`]). The hold token travels only inside the
//! WebSocket's SUBSCRIBE, never in a URL, so SSE and polling carry no PRIVATE
//! frames.
//!
//! A connection's task logs one record when it subscribes and one when it
//! closes, in a `realtime` span that carries the upgrade request's id; nothing
//! per frame at `info`.

use std::convert::Infallible;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use axum::Router;
use axum::body::{Body, Bytes, HttpBody};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{FromRequestParts, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::map_response;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use base64::Engine as _;
use cavea_engine::{EventId, Frame, TokenHash};
use cavea_format::wire::{self, ClientFrame, CloseReason, WireError};
use serde::Deserialize;
use tokio::sync::mpsc;
use tokio::time::{Instant, timeout};
use tracing::{Instrument as _, Span, field, info, info_span};

use super::auth::{Auth, ClientIp, QueryKey, has_hex_tail};
use super::extract::{Path, Query, invalid};
use super::public::none_match;
use super::seats::{Find, Target, target, token_hash};
use super::{ApiError, AppState, Code};
use crate::config::CorsOrigins;
use crate::realtime::{
    Ending, Feed, Gone, Inbound, Link, MAX_CLIENT_MESSAGE, MAX_MESSAGES_PER_PING, Out, Refused,
    Slot, Stats, Transport, close, run,
};

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/v1/public/events/{event}/state",
            get(state).layer(map_response(no_store_errors)),
        )
        .route(
            "/v1/public/events/{event}/stream",
            get(stream).layer(map_response(no_store_errors)),
        )
        .route("/v1/public/rt", get(websocket))
}

/// One frame of 08 §8 as an HTTP body.
pub const FRAME_TYPE: &str = "application/vnd.cavea.frame";

/// A shared cache must not keep an error of a cacheable route: the next
/// request may succeed.
async fn no_store_errors(mut response: Response) -> Response {
    let status = response.status();
    if !status.is_success() && status != StatusCode::NOT_MODIFIED {
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    response
}

fn unavailable() -> ApiError {
    ApiError::new(Code::Unavailable)
        .with_message("The event's state is unavailable; retry shortly.")
}

// ---- snapshot and polling (08 §8.4, §8.5) ----

#[derive(Debug, Deserialize)]
struct StateQuery {
    #[serde(default)]
    after_seq: Option<u64>,
}

/// The public SNAPSHOT with `ETag: "<seq>"`, or with `after_seq`, one DELTA
/// from it, the SNAPSHOT, or `204` when nothing was published after it. Read
/// from the engine's published state, without a message to the actor. A frame
/// may be cached for `CAVEA_SNAPSHOT_MAX_AGE`: it stays true, only less
/// current. A `204` may not: it stops being true at the next publication.
async fn state(
    QueryKey(auth): QueryKey,
    Path(event): Path<String>,
    Query(query): Query<StateQuery>,
    headers: HeaderMap,
    State(state): State<AppState>,
) -> Result<Response, ApiError> {
    let target = target(&state, &auth, Find::Key(&event)).await?;
    let registry = &state.seats.registry;
    let mut response = match query.after_seq {
        None => {
            let frame = registry
                .snapshot(target.id)
                .await
                .map_err(|_| unavailable())?;
            let etag = HeaderValue::try_from(format!("\"{}\"", frame.seq()))
                .map_err(ApiError::internal)?;
            let (listed, any) = none_match(&headers, &etag);
            let mut response = if listed || any {
                StatusCode::NOT_MODIFIED.into_response()
            } else {
                frame_body(frame)
            };
            response.headers_mut().insert(header::ETAG, etag);
            response
        }
        Some(after) => match registry.after(target.id, after).await {
            Ok(Some(frame)) => frame_body(frame),
            Ok(None) => {
                let mut response = StatusCode::NO_CONTENT.into_response();
                response
                    .headers_mut()
                    .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
                return Ok(response);
            }
            Err(_) => return Err(unavailable()),
        },
    };
    let cache = format!(
        "public, max-age={}, stale-while-revalidate=5",
        state.config.snapshot_max_age
    );
    let cache = HeaderValue::try_from(cache).map_err(ApiError::internal)?;
    response.headers_mut().insert(header::CACHE_CONTROL, cache);
    Ok(response)
}

fn frame_body(frame: Frame) -> Response {
    let mut response = Body::from(Bytes::from_owner(frame)).into_response();
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(FRAME_TYPE));
    response
}

// ---- connections ----

/// A connection slot, or why there is none (12 §3 T11).
fn admit(state: &AppState, ClientIp(ip): ClientIp) -> Result<Slot, ApiError> {
    state.realtime.admit(ip).map_err(|refused| match refused {
        Refused::ShuttingDown => ApiError::new(Code::Unavailable).with_message("Shutting down."),
        Refused::Node => ApiError::new(Code::Overloaded)
            .with_message("This server holds as many realtime connections as it may."),
        Refused::Ip => ApiError::new(Code::RateLimited)
            .with_message("Too many realtime connections from this address."),
    })
}

/// The span of one connection's task: not a child of the request, which ends
/// when the response starts, but carrying its id.
fn connection_span(transport: Transport, auth: &Auth) -> Span {
    let request_id = cavea_telemetry::request_id().map(|id| id.to_string());
    info_span!(
        parent: None,
        "realtime",
        request_id = request_id.as_deref().unwrap_or_default(),
        transport = transport.as_str(),
        workspace = %auth.workspace(),
        event = field::Empty,
    )
}

fn closed(started: Instant, stats: Stats, reason: &str) {
    info!(
        duration_ms = started.elapsed().as_secs_f64() * 1e3,
        frames = stats.frames,
        resyncs = stats.resyncs,
        reason,
        "realtime connection closed"
    );
}

// ---- WebSocket (08 §8.2, §8.3) ----

/// [`WebSocketUpgrade`], refused as problem+json.
struct Upgrade(WebSocketUpgrade);

impl<S: Send + Sync> FromRequestParts<S> for Upgrade {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, ApiError> {
        WebSocketUpgrade::from_request_parts(parts, state)
            .await
            .map(Self)
            .map_err(|_| invalid("upgrade", "Expected a WebSocket upgrade."))
    }
}

/// `GET /v1/public/rt`: the key and its origin are checked on the upgrade
/// request; the client then sends SUBSCRIBE. CORS does not apply to
/// WebSockets, so `CAVEA_CORS_ORIGINS` is checked here too: an `Origin` must
/// pass both, as for the other public routes.
async fn websocket(
    QueryKey(auth): QueryKey,
    ip: ClientIp,
    headers: HeaderMap,
    State(state): State<AppState>,
    Upgrade(upgrade): Upgrade,
) -> Result<Response, ApiError> {
    if let (CorsOrigins::List(list), Some(origin)) =
        (&state.config.cors_origins, headers.get(header::ORIGIN))
        && !origin
            .to_str()
            .is_ok_and(|o| list.iter().any(|a| a.eq_ignore_ascii_case(o)))
    {
        return Err(ApiError::new(Code::Forbidden)
            .with_message("WebSockets are not accepted from this origin.")
            .with_detail("reason", "origin"));
    }
    let slot = admit(&state, ip)?;
    let span = connection_span(Transport::WebSocket, &auth);
    // The read buffer holds one client message; tungstenite's default
    // allocates 128 KiB per connection. Frames are flushed one at a time, so
    // nothing is gained by buffering writes.
    Ok(upgrade
        .read_buffer_size(2 * MAX_CLIENT_MESSAGE)
        .write_buffer_size(0)
        .max_message_size(MAX_CLIENT_MESSAGE)
        .max_frame_size(MAX_CLIENT_MESSAGE)
        .on_upgrade(move |socket| {
            async move {
                let _slot = slot;
                websocket_session(state, auth, WsLink(socket)).await;
            }
            .instrument(span)
        }))
}

struct WsLink(WebSocket);

impl Link for WsLink {
    const HEARS_CLIENT: bool = true;

    async fn send(&mut self, out: Out) -> Result<(), Gone> {
        let bytes = match out {
            Out::Frame(frame) => Bytes::from_owner(frame),
            Out::Bytes(bytes) => Bytes::from(bytes),
            Out::Ping(nonce) => Bytes::from(wire::encode_ping(nonce)),
        };
        self.0.send(Message::Binary(bytes)).await.map_err(|_| Gone)
    }

    async fn recv(&mut self) -> Inbound {
        match self.0.recv().await {
            None | Some(Ok(Message::Close(_))) => Inbound::Gone,
            Some(Ok(Message::Ping(_) | Message::Pong(_))) => Inbound::Alive,
            // Unknown frame types are skipped (08 §8.6).
            Some(Ok(Message::Binary(bytes))) => match wire::decode_client(&bytes) {
                Ok(ClientFrame::Pong(_)) | Err(WireError::UnknownType) => Inbound::Alive,
                Ok(ClientFrame::Subscribe(_)) | Err(_) => Inbound::Bad,
            },
            Some(Ok(Message::Text(_))) => Inbound::Bad,
            Some(Err(e)) if oversized(&e) => Inbound::Bad,
            // The connection broke, or the client closed it without a
            // closing handshake.
            Some(Err(_)) => Inbound::Gone,
        }
    }
}

/// Whether a read failed because the client's message was over
/// [`MAX_CLIENT_MESSAGE`]: a broken rule (CLOSE 6), not a broken connection.
fn oversized(error: &axum::Error) -> bool {
    let inner = std::error::Error::source(error);
    matches!(
        inner.and_then(|e| e.downcast_ref::<tungstenite::Error>()),
        Some(tungstenite::Error::Capacity(_))
    )
}

/// SUBSCRIBE, owned.
struct Subscribe {
    event: String,
    last_seq: u64,
    hold_token: Option<Vec<u8>>,
}

/// Why a connection ends before streaming: the CLOSE to send (none if the
/// client has gone or is silent) and the reason logged.
type Refusal = (Option<CloseReason>, &'static str);

const BAD_FRAME: Refusal = (Some(CloseReason::BAD_FRAME), "bad_frame");
const NOT_FOUND: Refusal = (Some(CloseReason::NOT_FOUND), "not_found");
const UNAVAILABLE: Refusal = (Some(CloseReason::SHUTTING_DOWN), "unavailable");

/// The first frame must be SUBSCRIBE (08 §8.2); a session token in it is
/// ignored until session tokens exist (SRV-016).
async fn first_subscribe(socket: &mut WebSocket) -> Result<Subscribe, Refusal> {
    for _ in 0..MAX_MESSAGES_PER_PING {
        let bytes = match socket.recv().await {
            Some(Ok(Message::Ping(_) | Message::Pong(_))) => continue,
            Some(Ok(Message::Binary(bytes))) => bytes,
            Some(Ok(Message::Text(_))) => return Err(BAD_FRAME),
            Some(Err(e)) if oversized(&e) => return Err(BAD_FRAME),
            None | Some(Ok(Message::Close(_)) | Err(_)) => return Err((None, "client_closed")),
        };
        return match wire::decode_client(&bytes) {
            Ok(ClientFrame::Subscribe(s)) => Ok(Subscribe {
                event: s.event_key.to_owned(),
                last_seq: s.last_seq,
                hold_token: s.hold_token.map(<[u8]>::to_vec),
            }),
            Err(WireError::UnknownType) => continue,
            Err(WireError::UnsupportedVersion) => Err((
                Some(CloseReason::UNSUPPORTED_VERSION),
                "unsupported_version",
            )),
            Ok(ClientFrame::Pong(_)) | Err(_) => Err(BAD_FRAME),
        };
    }
    Err(BAD_FRAME)
}

async fn websocket_session(state: AppState, auth: Auth, mut link: WsLink) {
    let started = Instant::now();
    let mut stats = Stats::default();
    let handshake = state.realtime.timing().handshake;
    let first = tokio::select! {
        biased;
        () = state.realtime.stopping() => Err((Some(CloseReason::SHUTTING_DOWN), "shutdown")),
        first = timeout(handshake, first_subscribe(&mut link.0)) => {
            first.unwrap_or(Err((None, "handshake_timeout")))
        }
    };
    let ended = match first {
        Ok(subscribe) => follow(&state, &auth, &mut link, subscribe, &mut stats).await,
        Err(refusal) => Err(refusal),
    };
    let reason = match ended {
        Ok(ending) => ending.as_str(),
        Err((close_reason, why)) => {
            if let Some(close_reason) = close_reason {
                close(&mut link, close_reason).await;
            }
            why
        }
    };
    if reason != Ending::Stalled.as_str() {
        let _ = timeout(Duration::from_secs(1), link.0.send(Message::Close(None))).await;
    }
    closed(started, stats, reason);
}

/// Hold tokens whose PRIVATE frames a browser can take: the renderer refuses a
/// set of more than this many objects, the most an event lets a buyer's token
/// hold (08 §6.2).
const PRIVATE_MAX_OBJECTS: i32 = 100;

/// Whether the token is a live hold of this event of the key's workspace that
/// a browser may follow: one whose object limit is at most
/// [`PRIVATE_MAX_OBJECTS`], as every buyer's token is.
const TOKEN_OF_EVENT: &str = "SELECT EXISTS (
  SELECT FROM holds h JOIN events e ON e.id = h.event_id
   WHERE h.token_hash = $1 AND h.event_id = $2::uuid AND e.workspace_id = $3::uuid
     AND h.expires_at > now() AND h.max_objects BETWEEN 1 AND $4)";

/// The token's hash if it may be followed on `event`, else `None`: unknown,
/// expired, of another event or workspace, or unlimited, all alike.
async fn followable_token(
    state: &AppState,
    auth: &Auth,
    event: EventId,
    token: &[u8],
) -> Result<Option<TokenHash>, ApiError> {
    let Some(token) = std::str::from_utf8(token)
        .ok()
        .filter(|t| has_hex_tail(t, "ht_", 64))
    else {
        return Ok(None);
    };
    let hash = token_hash(token);
    let found: bool = sqlx::query_scalar(TOKEN_OF_EVENT)
        .bind(hash.0.as_slice())
        .bind(format!("{:032x}", event.0))
        .bind(auth.workspace().uuid())
        .bind(PRIVATE_MAX_OBJECTS)
        .fetch_one(&state.db)
        .instrument(cavea_telemetry::db_span("find_hold_token"))
        .await?;
    Ok(found.then_some(hash))
}

/// Find the event and the token, subscribe, and stream. An unknown event and
/// a token that may not be followed get the same CLOSE 5.
async fn follow(
    state: &AppState,
    auth: &Auth,
    link: &mut WsLink,
    s: Subscribe,
    stats: &mut Stats,
) -> Result<Ending, Refusal> {
    let target = target(state, auth, Find::Key(&s.event))
        .await
        .map_err(|e| match e.code() {
            Code::NotFound => NOT_FOUND,
            _ => UNAVAILABLE,
        })?;
    Span::current().record("event", target.key.as_str());
    if target.archived {
        return Err((Some(CloseReason::ARCHIVED), "archived"));
    }
    let token = match &s.hold_token {
        None => None,
        Some(token) => Some(
            followable_token(state, auth, target.id, token)
                .await
                .map_err(|_| UNAVAILABLE)?
                .ok_or(NOT_FOUND)?,
        ),
    };
    let registry = &state.seats.registry;
    let sub = registry
        .subscribe(target.id, s.last_seq, token)
        .await
        .map_err(|_| UNAVAILABLE)?;
    let _counted = state.realtime.subscribed(Transport::WebSocket);
    info!(
        last_seq = s.last_seq,
        private = token.is_some(),
        "realtime subscribed"
    );
    let feed = Feed {
        registry,
        event: target.id,
        token,
    };
    Ok(run(&state.realtime, link, &feed, sub, stats).await)
}

// ---- SSE (08 §8.4) ----

#[derive(Debug, Deserialize)]
struct StreamQuery {
    #[serde(default)]
    last_seq: Option<u64>,
}

/// `GET /v1/public/events/{event}/stream`: the frames of the WebSocket, each
/// base64 in a `data:` line, SNAPSHOT and DELTA with `id: <seq>`. Resumes from
/// `Last-Event-ID` (an `EventSource` reconnecting), else from `last_seq`. No
/// hold token, so no PRIVATE frames. A client that stops reading blocks the
/// task's write once the one-frame channel and the connection's write queue
/// (hyper's: at most 16 chunks) are full, and is dropped at the stall limit
/// like a WebSocket.
async fn stream(
    QueryKey(auth): QueryKey,
    Path(event): Path<String>,
    Query(query): Query<StreamQuery>,
    ip: ClientIp,
    headers: HeaderMap,
    State(state): State<AppState>,
) -> Result<Response, ApiError> {
    let last_seq = match headers.get("last-event-id") {
        Some(id) => id
            .to_str()
            .ok()
            .and_then(|id| id.trim().parse().ok())
            .ok_or_else(|| invalid("last_event_id", "`Last-Event-ID` must be a seq."))?,
        None => query.last_seq.unwrap_or(0),
    };
    let target = target(&state, &auth, Find::Key(&event)).await?;
    let slot = admit(&state, ip)?;
    let span = connection_span(Transport::Sse, &auth);
    span.record("event", target.key.as_str());
    // One frame waits here at most; the rest stay in the engine's channel.
    let (tx, rx) = mpsc::channel(1);
    tokio::spawn(
        async move {
            let _slot = slot;
            sse_session(state, target, last_seq, SseLink(tx)).await;
        }
        .instrument(span),
    );
    let mut response = Body::new(SseBody(rx)).into_response();
    let headers = response.headers_mut();
    let pairs = [
        (header::CONTENT_TYPE, "text/event-stream"),
        (header::CACHE_CONTROL, "no-cache"),
    ];
    for (name, value) in pairs {
        headers.insert(name, HeaderValue::from_static(value));
    }
    // Proxies such as nginx otherwise buffer the stream.
    headers.insert("x-accel-buffering", HeaderValue::from_static("no"));
    Ok(response)
}

async fn sse_session(state: AppState, target: Target, last_seq: u64, mut link: SseLink) {
    let started = Instant::now();
    let mut stats = Stats::default();
    let registry = &state.seats.registry;
    let reason = if target.archived {
        close(&mut link, CloseReason::ARCHIVED).await;
        "archived"
    } else if let Ok(sub) = registry.subscribe(target.id, last_seq, None).await {
        let _counted = state.realtime.subscribed(Transport::Sse);
        info!(last_seq, "realtime subscribed");
        let feed = Feed {
            registry,
            event: target.id,
            token: None,
        };
        run(&state.realtime, &mut link, &feed, sub, &mut stats)
            .await
            .as_str()
    } else {
        close(&mut link, CloseReason::SHUTTING_DOWN).await;
        "unavailable"
    };
    closed(started, stats, reason);
}

struct SseLink(mpsc::Sender<Bytes>);

impl Link for SseLink {
    const HEARS_CLIENT: bool = false;

    async fn send(&mut self, out: Out) -> Result<(), Gone> {
        let base64 = base64::engine::general_purpose::STANDARD;
        let event = match out {
            Out::Frame(frame) => format!(
                "id: {}\ndata: {}\n\n",
                frame.seq(),
                base64.encode(frame.bytes())
            ),
            Out::Bytes(bytes) => format!("data: {}\n\n", base64.encode(bytes)),
            Out::Ping(_) => ":\n\n".to_owned(),
        };
        self.0.send(Bytes::from(event)).await.map_err(|_| Gone)
    }

    /// The client sends nothing; this resolves when it disconnects.
    async fn recv(&mut self) -> Inbound {
        self.0.closed().await;
        Inbound::Gone
    }
}

/// The SSE response body: what the connection's task writes.
struct SseBody(mpsc::Receiver<Bytes>);

impl HttpBody for SseBody {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<http_body::Frame<Bytes>, Infallible>>> {
        self.0
            .poll_recv(cx)
            .map(|event| event.map(|e| Ok(http_body::Frame::data(e))))
    }
}
