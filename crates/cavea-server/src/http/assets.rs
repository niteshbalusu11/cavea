//! `POST /v1/assets` and `GET /v1/public/assets/{hash}` (08 §6.2): images for
//! charts' background and reference images and published thumbnails.
//!
//! An upload is the image's bytes as the body (any `Content-Type`, which is
//! ignored), read up to `CAVEA_ASSET_MAX_BYTES` and refused as soon as it
//! passes it, then sniffed and named by its content hash (`crate::assets`).
//! It belongs to the key's workspace; uploading the same bytes again returns
//! the same asset.
//!
//! An asset is served to anyone who has its hash, without a credential, as
//! the compiled charts are (D-26, D-49): a buyer's browser fetches a chart's
//! background images from the URL alone. The response can do no harm if a
//! browser is sent to it: the type the server sniffed, `nosniff`, and a
//! sandboxing Content Security Policy.

use std::convert::Infallible;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use axum::body::{Body, Bytes, HttpBody as _};
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::map_response;
use axum::response::{IntoResponse, Response};
use http_body::{Frame, SizeHint};
use serde_json::json;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use super::extract::{Path, invalid};
use super::openapi::{Needs, Op, Routes};
use super::public::{IMMUTABLE, cors_open, none_match, parse_hash};
use super::staff::Staff;
use super::{ApiError, AppState, Code};
use crate::assets::{self, Image};
use crate::console::Permission;

pub(super) fn routes() -> Routes {
    let image = || Some(json!({ "image/png": {}, "image/jpeg": {}, "image/webp": {} }));
    Routes::new()
        // The handler reads its body itself, against its own limit; axum's
        // default limit applies only to its body extractors.
        .add(
            Op::post("/v1/assets", "uploadAsset", "Upload an image")
                .about("The body is the image: PNG, JPEG or WebP, decided by its bytes (any \
`Content-Type` is ignored); at most `CAVEA_ASSET_MAX_BYTES` (10 MiB by default) and 8,192 pixels \
a side. Refused with `400` and `reason`: `body_too_large`, `asset_type`, `asset_malformed` \
(truncated, or data after the image's end) or `asset_dimensions`.")
                .needs(Needs::Staff(Permission::ChartsWrite))
                .raw_body(json!({
                    "required": true,
                    "content": { "application/octet-stream": {}, "image/png": {}, "image/jpeg": {}, "image/webp": {} },
                }))
                .reply(201, "Asset", "A new asset")
                .reply(200, "Asset", "The workspace already had these bytes"),
            upload,
        )
        .layered(
            Op::get("/v1/public/assets/{hash}", "publicAsset", "An image")
                .about("No credential: the content hash names the file. Immutable; CORS open.")
                .response(200, "The image, with the type it was sniffed as", image())
                .etag()
                .response(304, "Not modified", None)
                .etag(),
            asset,
            |route| route.layer(map_response(cors_open)),
        )
}

fn too_large(max: u32) -> ApiError {
    invalid("body_too_large", "The image is over the size limit.").with_detail("limit", max)
}

async fn upload(
    Staff(auth): Staff,
    State(state): State<AppState>,
    headers: HeaderMap,
    mut body: Body,
) -> Result<Response, ApiError> {
    let max = state.config.asset_max_bytes.get();
    let limit = usize::try_from(max).unwrap_or(usize::MAX);
    let declared = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<usize>().ok());
    if declared.is_some_and(|n| n > limit) {
        return Err(too_large(max));
    }
    // The bytes held are counted against the node's upload budget as they
    // arrive, and released when this handler returns.
    let budget = &state.asset_memory.uploads;
    let mut held: Option<OwnedSemaphorePermit> = None;
    hold(&mut held, budget, declared.unwrap_or(0))?;
    let mut bytes = Vec::with_capacity(declared.unwrap_or(0));
    while let Some(frame) = std::future::poll_fn(|cx| Pin::new(&mut body).poll_frame(cx)).await {
        let frame = frame.map_err(|_| invalid("body_unreadable", "The body could not be read."))?;
        if let Ok(data) = frame.into_data() {
            if bytes.len() + data.len() > limit {
                return Err(too_large(max));
            }
            hold(&mut held, budget, bytes.len() + data.len())?;
            bytes.extend_from_slice(&data);
        }
    }
    // The bytes travel with their share of the budget: a cancelled request
    // leaves both to the blocking task, which drops them together.
    let (bytes, _held, image, hash) = tokio::task::spawn_blocking(move || {
        let image = assets::sniff(&bytes);
        let hash = assets::hash(&bytes);
        (bytes, held, image, hash)
    })
    .await
    .map_err(ApiError::internal)?;
    let image = image.map_err(|refused| {
        let error = invalid(refused.reason(), "Not an image this server takes.");
        match refused {
            assets::Refused::Dimensions => error.with_detail("limit", assets::MAX_SIDE),
            _ => error,
        }
    })?;
    let created = assets::put(&state.db, auth.workspace().uuid(), &hash, image, &bytes).await?;
    let status = if created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((status, axum::Json(view(&hash, image, bytes.len()))).into_response())
}

/// Grow `held` to `total` bytes of `budget`, without waiting: `503` when the
/// node holds as many upload bytes as it may.
fn hold(
    held: &mut Option<OwnedSemaphorePermit>,
    budget: &Arc<Semaphore>,
    total: usize,
) -> Result<(), ApiError> {
    let counted = held.as_ref().map_or(0, OwnedSemaphorePermit::num_permits);
    let Some(more) = total.checked_sub(counted).filter(|&n| n > 0) else {
        return Ok(());
    };
    let more = u32::try_from(more).map_err(|_| busy())?;
    let permit = budget
        .clone()
        .try_acquire_many_owned(more)
        .map_err(|_| busy())?;
    match held {
        Some(held) => held.merge(permit),
        None => *held = Some(permit),
    }
    Ok(())
}

/// The node holds as many image bytes as it may (`assets::Memory`).
fn busy() -> ApiError {
    ApiError::new(Code::Overloaded)
        .with_message("Too many images are being transferred; retry shortly.")
        .with_detail("reason", "asset_memory")
}

fn hex(hash: &[u8; 16]) -> String {
    hash.iter().map(|b| format!("{b:02x}")).collect()
}

fn view(hash: &[u8; 16], image: Image, size: usize) -> serde_json::Value {
    let hex = hex(hash);
    json!({
        "url": format!("/v1/public/assets/{hex}"),
        "hash": hex,
        "content_type": image.kind.content_type(),
        "size": size,
        "width": image.width,
        "height": image.height,
    })
}

/// Harmless if navigated to: nothing may load or run, and the document is
/// sandboxed (12 §3 T7).
const CSP: &str = "default-src 'none'; sandbox";

async fn asset(
    Path(hex): Path<String>,
    headers: HeaderMap,
    State(state): State<AppState>,
) -> Result<Response, ApiError> {
    let not_found = || ApiError::new(Code::NotFound).with_message("No such asset.");
    let hash = parse_hash(&hex).ok_or_else(not_found)?;
    let etag = HeaderValue::try_from(format!("\"{hex}\"")).map_err(ApiError::internal)?;
    let (listed, any) = none_match(&headers, &etag);
    let mut response = if listed || any {
        if !assets::exists(&state.db, &hash).await? {
            return Err(not_found());
        }
        StatusCode::NOT_MODIFIED.into_response()
    } else {
        let (content_type, size) = assets::size(&state.db, &hash)
            .await?
            .ok_or_else(not_found)?;
        // Room for the bytes until they have been written to the client. An
        // image larger than the whole budget (stored before
        // `CAVEA_ASSET_MAX_BYTES` was lowered) is never read into memory.
        let budget = &state.asset_memory;
        let wanted = u32::try_from(size)
            .ok()
            .filter(|&n| n <= budget.budget)
            .ok_or_else(|| {
                tracing::warn!(
                    size,
                    budget = budget.budget,
                    "an asset is larger than the serving budget; raise CAVEA_ASSET_MAX_BYTES to serve it"
                );
                busy()
            })?;
        let permit = tokio::time::timeout(
            assets::SERVE_WAIT,
            budget.serves.clone().acquire_many_owned(wanted),
        )
        .await
        .map_err(|_| busy())?
        .map_err(ApiError::internal)?;
        let bytes = assets::get(&state.db, &hash).await?.ok_or_else(not_found)?;
        let extension = match content_type.as_str() {
            "image/png" => "png",
            "image/jpeg" => "jpg",
            _ => "webp",
        };
        let disposition = format!("inline; filename=\"{hex}.{extension}\"");
        // The permit lives as long as the allocation: every piece written
        // shares it, and the last one may outlive the body in the
        // connection's write buffer.
        let bytes = Bytes::from_owner(Owned {
            bytes,
            _permit: permit,
        });
        let mut response = Body::new(Pieces(bytes)).into_response();
        let out = response.headers_mut();
        out.insert(
            header::CONTENT_TYPE,
            HeaderValue::try_from(content_type).map_err(ApiError::internal)?,
        );
        out.insert(
            header::CONTENT_DISPOSITION,
            HeaderValue::try_from(disposition).map_err(ApiError::internal)?,
        );
        out.insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(CSP),
        );
        response
    };
    let out = response.headers_mut();
    out.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    out.insert(header::CACHE_CONTROL, HeaderValue::from_static(IMMUTABLE));
    out.insert(header::ETAG, etag);
    Ok(response)
}

/// An asset's bytes with their share of [`assets::Memory::serves`], given
/// back when the last [`Bytes`] sharing them is dropped.
struct Owned {
    bytes: Vec<u8>,
    _permit: OwnedSemaphorePermit,
}

impl AsRef<[u8]> for Owned {
    fn as_ref(&self) -> &[u8] {
        &self.bytes
    }
}

/// A response body written in pieces, so that the HTTP connection takes the
/// next only as the client reads.
struct Pieces(Bytes);

/// The largest piece of an asset written at once.
const PIECE: usize = 64 * 1024;

impl http_body::Body for Pieces {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, Infallible>>> {
        if self.0.is_empty() {
            return Poll::Ready(None);
        }
        let n = self.0.len().min(PIECE);
        Poll::Ready(Some(Ok(Frame::data(self.0.split_to(n)))))
    }

    fn is_end_stream(&self) -> bool {
        self.0.is_empty()
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::with_exact(self.0.len() as u64)
    }
}
