use crate::chain::ChainClient;
use crate::config::Config;
use crate::content::ContentStore;
use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Multipart, Path, State};
use axum::http::header::{
    ACCEPT_RANGES, CACHE_CONTROL, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, ETAG, RANGE,
};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use lichen_core::{Keypair, PqSignature, Pubkey};
use serde::Serialize;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncSeekExt, SeekFrom};
use tokio::sync::{Mutex, Notify};
use tokio_util::io::ReaderStream;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::trace::TraceLayer;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub store: Arc<ContentStore>,
    pub chain: Arc<ChainClient>,
    pub reconcile_notify: Arc<Notify>,
    limiter: Arc<Mutex<HashMap<String, HourlyUsage>>>,
}

#[derive(Debug, Clone, Copy)]
struct HourlyUsage {
    hour: u64,
    bytes: u64,
}

#[derive(Serialize)]
struct UploadResponse {
    hash: String,
    size: u64,
    created: bool,
    uri: String,
    gateway_url: String,
    state: &'static str,
}

pub fn upload_signing_message(hash: &str, size: u64, content_type: &str) -> String {
    format!("lichen-moss-upload-v1\n{hash}\n{size}\n{content_type}")
}

impl AppState {
    pub fn new(
        config: Arc<Config>,
        store: Arc<ContentStore>,
        chain: Arc<ChainClient>,
        reconcile_notify: Arc<Notify>,
    ) -> Self {
        Self {
            config,
            store,
            chain,
            reconcile_notify,
            limiter: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    async fn charge_owner(&self, owner: &str, bytes: u64) -> Result<(), String> {
        let hour = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| "system clock is before Unix epoch".to_string())?
            .as_secs()
            / 3_600;
        let mut limiter = self.limiter.lock().await;
        limiter.retain(|_, usage| usage.hour >= hour.saturating_sub(1));
        let usage = limiter
            .entry(owner.to_string())
            .or_insert(HourlyUsage { hour, bytes: 0 });
        if usage.hour != hour {
            *usage = HourlyUsage { hour, bytes: 0 };
        }
        let next = usage
            .bytes
            .checked_add(bytes)
            .ok_or_else(|| "owner upload quota overflow".to_string())?;
        if next > self.config.owner_hourly_bytes {
            return Err("owner hourly upload quota exceeded".to_string());
        }
        usage.bytes = next;
        Ok(())
    }
}

pub fn build_app(state: AppState) -> Result<Router, String> {
    let origins = state
        .config
        .allowed_origins
        .iter()
        .map(|origin| {
            origin
                .parse::<HeaderValue>()
                .map_err(|_| format!("invalid Moss CORS origin: {origin}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let body_limit = usize::try_from(
        state
            .config
            .max_object_bytes
            .checked_add(2 * 1024 * 1024)
            .ok_or_else(|| "Moss body limit overflow".to_string())?,
    )
    .map_err(|_| "Moss body limit exceeds this platform".to_string())?;
    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::list(origins))
        .allow_methods([Method::GET, Method::HEAD, Method::POST])
        .allow_headers([CONTENT_TYPE, RANGE])
        .expose_headers([
            CONTENT_LENGTH,
            CONTENT_RANGE,
            CONTENT_TYPE,
            ETAG,
            ACCEPT_RANGES,
        ]);

    Ok(Router::new()
        .route("/healthz", get(health))
        .route("/readyz", get(ready))
        .route("/v1/uploads", post(upload))
        .route("/v1/objects/:hash", get(get_object).head(head_object))
        .route("/moss/:hash", get(get_object).head(head_object))
        .layer(DefaultBodyLimit::max(body_limit))
        .layer(cors)
        .layer(TraceLayer::new_for_http())
        .with_state(state))
}

async fn health(State(state): State<AppState>) -> impl IntoResponse {
    Json(json!({
        "status": "ok",
        "provider": state.chain.provider().to_base58(),
        "stored_bytes": state.store.stored_bytes(),
    }))
}

async fn ready(State(state): State<AppState>) -> Response {
    match tokio::try_join!(state.chain.current_slot(), state.chain.provider_status()) {
        Ok((slot, Some(provider))) if provider.operational() => Json(json!({
            "status": "ready",
            "slot": slot,
            "provider": state.chain.provider().to_base58(),
            "stored_bytes": state.store.stored_bytes(),
            "capacity_bytes": provider.capacity,
            "used_bytes": provider.used,
            "stored_objects": provider.stored_count,
            "collateral_spores": provider.collateral,
            "remaining_obligations_spores": provider.remaining_obligations,
            "required_collateral_spores": provider.required_collateral,
            "accepting_assignments": provider.accepting_assignments(),
            "price": provider.price,
        }))
        .into_response(),
        Ok((_slot, Some(_))) => error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "Moss provider is inactive or has no valid price",
        ),
        Ok((_slot, None)) => error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "Moss provider is not registered on-chain",
        ),
        Err(error) => error_response(StatusCode::SERVICE_UNAVAILABLE, &error),
    }
}

async fn upload(State(state): State<AppState>, mut multipart: Multipart) -> Response {
    match state.chain.provider_status().await {
        Ok(Some(provider)) if provider.accepting_assignments() => {}
        Ok(Some(_)) => {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "Moss provider is not accepting new assignments",
            )
        }
        Ok(None) => {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "Moss provider is not registered on-chain",
            )
        }
        Err(error) => return error_response(StatusCode::SERVICE_UNAVAILABLE, &error),
    }
    let mut hash = None;
    let mut size = None;
    let mut owner = None;
    let mut content_type = None;
    let mut signature = None;

    loop {
        let field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(error) => {
                return error_response(StatusCode::BAD_REQUEST, &format!("invalid upload: {error}"))
            }
        };
        let name = field.name().unwrap_or_default().to_string();
        if name == "object" {
            let hash = match hash.as_deref() {
                Some(value) => value,
                None => return error_response(StatusCode::BAD_REQUEST, "hash must precede object"),
            };
            let size = match size {
                Some(value) => value,
                None => return error_response(StatusCode::BAD_REQUEST, "size must precede object"),
            };
            let content_type = match content_type.as_deref() {
                Some(value) => value,
                None => {
                    return error_response(
                        StatusCode::BAD_REQUEST,
                        "content_type must precede object",
                    )
                }
            };
            let owner = match owner.as_deref() {
                Some(value) => value,
                None if !state.config.require_upload_signature => "anonymous",
                None => {
                    return error_response(StatusCode::UNAUTHORIZED, "signed owner is required")
                }
            };
            if let Err(error) = authorize_upload(
                &state,
                hash,
                size,
                content_type,
                owner,
                signature.as_deref(),
            ) {
                return error_response(StatusCode::UNAUTHORIZED, &error);
            }
            if let Err(error) = state.charge_owner(owner, size).await {
                return error_response(StatusCode::TOO_MANY_REQUESTS, &error);
            }
            let put = match state.store.put_stream(hash, size, field).await {
                Ok(result) => result,
                Err(error) => return error_response(StatusCode::BAD_REQUEST, &error),
            };
            if let Err(error) = state.store.set_content_type(hash, content_type).await {
                return error_response(StatusCode::INTERNAL_SERVER_ERROR, &error);
            }
            let gateway_url = format!("{}/moss/{}", state.config.public_base_url, hash);
            state.reconcile_notify.notify_one();
            return (
                if put.created {
                    StatusCode::CREATED
                } else {
                    StatusCode::OK
                },
                Json(UploadResponse {
                    hash: hash.to_string(),
                    size: put.size,
                    created: put.created,
                    uri: format!("moss://{hash}"),
                    gateway_url,
                    state: "staged",
                }),
            )
                .into_response();
        }

        let text = match field.text().await {
            Ok(value) if value.len() <= 16 * 1024 => value,
            Ok(_) => return error_response(StatusCode::BAD_REQUEST, "upload field is too large"),
            Err(error) => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    &format!("invalid upload field: {error}"),
                )
            }
        };
        match name.as_str() {
            "hash" => hash = Some(text),
            "size" => {
                size = match text.parse::<u64>() {
                    Ok(value) => Some(value),
                    Err(_) => return error_response(StatusCode::BAD_REQUEST, "size must be u64"),
                }
            }
            "owner" => owner = Some(text),
            "content_type" => match normalize_content_type(&text) {
                Ok(value) => content_type = Some(value),
                Err(error) => return error_response(StatusCode::BAD_REQUEST, &error),
            },
            "signature" => signature = Some(text),
            _ => return error_response(StatusCode::BAD_REQUEST, "unknown upload field"),
        }
    }
    error_response(StatusCode::BAD_REQUEST, "object field is required")
}

fn authorize_upload(
    state: &AppState,
    hash: &str,
    size: u64,
    content_type: &str,
    owner: &str,
    signature_json: Option<&str>,
) -> Result<(), String> {
    crate::content::decode_hash(hash)?;
    if size == 0 || size > state.config.max_object_bytes {
        return Err("object size is outside provider limits".to_string());
    }
    if !state.config.require_upload_signature {
        return Ok(());
    }
    let owner = Pubkey::from_base58(owner).map_err(|_| "owner address is invalid".to_string())?;
    let signature = serde_json::from_str::<PqSignature>(
        signature_json.ok_or_else(|| "upload signature is required".to_string())?,
    )
    .map_err(|_| "upload signature JSON is invalid".to_string())?;
    let message = upload_signing_message(hash, size, content_type);
    if !Keypair::verify(&owner, message.as_bytes(), &signature) {
        return Err("upload signature verification failed".to_string());
    }
    Ok(())
}

fn normalize_content_type(value: &str) -> Result<String, String> {
    let value = value.trim().to_ascii_lowercase();
    if value.is_empty()
        || value.len() > 128
        || value.contains(['\r', '\n'])
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"!#$&^_.+-/;= ".contains(&byte))
    {
        return Err("content_type is invalid".to_string());
    }
    Ok(value)
}

async fn head_object(State(state): State<AppState>, Path(hash): Path<String>) -> Response {
    serve_object(&state, &hash, &HeaderMap::new(), true).await
}

async fn get_object(
    State(state): State<AppState>,
    Path(hash): Path<String>,
    headers: HeaderMap,
) -> Response {
    serve_object(&state, &hash, &headers, false).await
}

async fn serve_object(
    state: &AppState,
    hash: &str,
    headers: &HeaderMap,
    head_only: bool,
) -> Response {
    let path = match state.store.path_for(hash) {
        Ok(path) => path,
        Err(error) => return error_response(StatusCode::BAD_REQUEST, &error),
    };
    let metadata = match tokio::fs::metadata(&path).await {
        Ok(metadata) if metadata.is_file() => metadata,
        Ok(_) => return error_response(StatusCode::NOT_FOUND, "object not found"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return error_response(StatusCode::NOT_FOUND, "object not found")
        }
        Err(error) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("inspect object: {error}"),
            )
        }
    };
    let size = metadata.len();
    let range = match parse_range(headers.get(RANGE), size) {
        Ok(range) => range,
        Err(error) => return error_response(StatusCode::RANGE_NOT_SATISFIABLE, &error),
    };
    let (start, end, status) = match range {
        Some((start, end)) => (start, end, StatusCode::PARTIAL_CONTENT),
        None => (0, size.saturating_sub(1), StatusCode::OK),
    };
    let length = if size == 0 { 0 } else { end - start + 1 };
    let mut builder = Response::builder()
        .status(status)
        .header(CONTENT_TYPE, state.store.content_type(hash).await)
        .header(CONTENT_LENGTH, length.to_string())
        .header(ACCEPT_RANGES, "bytes")
        .header(ETAG, format!("\"{hash}\""))
        .header(CACHE_CONTROL, "public, max-age=31536000, immutable")
        .header("x-content-type-options", "nosniff");
    if status == StatusCode::PARTIAL_CONTENT {
        builder = builder.header(CONTENT_RANGE, format!("bytes {start}-{end}/{size}"));
    }
    if head_only || length == 0 {
        return builder
            .body(Body::empty())
            .unwrap_or_else(internal_response);
    }
    let mut file = match tokio::fs::File::open(path).await {
        Ok(file) => file,
        Err(error) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("open object: {error}"),
            )
        }
    };
    if let Err(error) = file.seek(SeekFrom::Start(start)).await {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("seek object: {error}"),
        );
    }
    let stream = ReaderStream::new(file.take(length));
    builder
        .body(Body::from_stream(stream))
        .unwrap_or_else(internal_response)
}

fn parse_range(header: Option<&HeaderValue>, size: u64) -> Result<Option<(u64, u64)>, String> {
    let Some(header) = header else {
        return Ok(None);
    };
    if size == 0 {
        return Err("empty object has no byte range".to_string());
    }
    let value = header
        .to_str()
        .map_err(|_| "Range header is invalid".to_string())?;
    let range = value
        .strip_prefix("bytes=")
        .ok_or_else(|| "only byte ranges are supported".to_string())?;
    if range.contains(',') {
        return Err("multiple ranges are not supported".to_string());
    }
    let (start, end) = range
        .split_once('-')
        .ok_or_else(|| "Range header is invalid".to_string())?;
    if start.is_empty() {
        let suffix = end
            .parse::<u64>()
            .map_err(|_| "Range suffix is invalid".to_string())?;
        if suffix == 0 {
            return Err("Range suffix must be nonzero".to_string());
        }
        let length = suffix.min(size);
        return Ok(Some((size - length, size - 1)));
    }
    let start = start
        .parse::<u64>()
        .map_err(|_| "Range start is invalid".to_string())?;
    if start >= size {
        return Err("Range starts beyond the object".to_string());
    }
    let end = if end.is_empty() {
        size - 1
    } else {
        end.parse::<u64>()
            .map_err(|_| "Range end is invalid".to_string())?
            .min(size - 1)
    };
    if end < start {
        return Err("Range end precedes start".to_string());
    }
    Ok(Some((start, end)))
}

fn error_response(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({ "error": message }))).into_response()
}

fn internal_response(_: axum::http::Error) -> Response {
    error_response(
        StatusCode::INTERNAL_SERVER_ERROR,
        "could not build response",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_are_strict_and_bounded() {
        assert_eq!(parse_range(None, 100).unwrap(), None);
        assert_eq!(
            parse_range(Some(&HeaderValue::from_static("bytes=10-19")), 100).unwrap(),
            Some((10, 19))
        );
        assert_eq!(
            parse_range(Some(&HeaderValue::from_static("bytes=-10")), 100).unwrap(),
            Some((90, 99))
        );
        assert!(parse_range(Some(&HeaderValue::from_static("bytes=100-")), 100).is_err());
    }

    #[test]
    fn signing_message_is_canonical() {
        assert_eq!(
            upload_signing_message("abc", 42, "image/png"),
            "lichen-moss-upload-v1\nabc\n42\nimage/png"
        );
    }
}
