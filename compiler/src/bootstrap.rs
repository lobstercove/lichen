use axum::{
    extract::Json,
    http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Router,
};
use std::{
    process::{Command, Stdio},
    sync::Arc,
};
use tower_http::cors::CorsLayer;
use tracing::warn;

use super::{compile_handler_authed, health_handler, CompileRequest};

/// HTTP header name for API key authentication (P9-INF-01)
pub(super) const API_KEY_HEADER: &str = "x-api-key";
/// Default listener address for the compiler service.
const DEFAULT_COMPILER_HOST: &str = "127.0.0.1";
/// Default compiler service port.
const DEFAULT_COMPILER_PORT: u16 = 8901;
/// Mounted workspace root inside the compiler sandbox.
pub(super) const SANDBOX_WORKSPACE: &str = "/workspace";

/// Shared application state holding the required API key.
#[derive(Clone)]
pub(super) struct AppState {
    pub(super) api_key: Arc<String>,
    pub(super) compile_backend: Arc<CompileBackend>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum CompileBackend {
    Host,
    Docker(DockerSandbox),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct DockerSandbox {
    pub(super) runtime: String,
    pub(super) image: String,
}

impl CompileBackend {
    pub(super) fn description(&self) -> String {
        match self {
            Self::Host => "host-unsandboxed".to_string(),
            Self::Docker(sandbox) => {
                format!(
                    "container-sandbox(runtime={}, image={})",
                    sandbox.runtime, sandbox.image
                )
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ApiKeyError {
    Missing,
    Invalid,
}

impl IntoResponse for ApiKeyError {
    fn into_response(self) -> Response {
        let message = match self {
            Self::Missing => "Missing X-API-Key header",
            Self::Invalid => "Invalid API key",
        };

        (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": message})),
        )
            .into_response()
    }
}

pub(super) fn build_state() -> Result<(AppState, CompileBackend), String> {
    let api_key = required_api_key()?;
    let compile_backend = resolve_compile_backend()?;
    let state = AppState {
        api_key: Arc::new(api_key),
        compile_backend: Arc::new(compile_backend.clone()),
    };
    Ok((state, compile_backend))
}

pub(super) fn build_app(state: AppState) -> Result<Router, String> {
    let allowed_origin = compiler_cors_origin()?;

    Ok(Router::new()
        .route(
            "/compile",
            post({
                let state = state.clone();
                move |headers: HeaderMap, body: Json<CompileRequest>| {
                    let state = state.clone();
                    async move { compile_handler_authed(headers, body, state).await }
                }
            }),
        )
        .route("/health", get(health_handler))
        .layer(build_cors(allowed_origin)))
}

pub(super) fn compiler_bind_addr() -> String {
    format!("{}:{}", compiler_bind_host(), compiler_port())
}

/// P9-INF-01: Validate the X-API-Key header against the configured key.
/// Returns Ok(()) on success, or an error response on failure.
pub(super) fn validate_api_key(headers: &HeaderMap, state: &AppState) -> Result<(), ApiKeyError> {
    match headers
        .get(API_KEY_HEADER)
        .and_then(|value| value.to_str().ok())
    {
        Some(provided) if constant_time_eq(provided.as_bytes(), state.api_key.as_bytes()) => Ok(()),
        Some(_) => Err(ApiKeyError::Invalid),
        None => Err(ApiKeyError::Missing),
    }
}

pub(super) fn resolve_compile_backend() -> Result<CompileBackend, String> {
    if let Some(image) = std::env::var("COMPILER_SANDBOX_IMAGE")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    {
        let runtime = std::env::var("COMPILER_SANDBOX_RUNTIME")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "docker".to_string());

        if !command_available(&runtime) {
            return Err(format!(
                "Compiler sandbox runtime '{}' is unavailable; install it or set COMPILER_ALLOW_UNSANDBOXED=1 for local-only host compilation.",
                runtime
            ));
        }

        return Ok(CompileBackend::Docker(DockerSandbox { runtime, image }));
    }

    if env_var_enabled("LICHEN_LOCAL_DEV") {
        warn!(
            "⚠️ Compiler sandbox disabled because LICHEN_LOCAL_DEV=1; host toolchains remain a trusted local-dev path only"
        );
        return Ok(CompileBackend::Host);
    }

    if env_var_enabled("COMPILER_ALLOW_UNSANDBOXED") {
        warn!(
            "⚠️ Compiler sandbox disabled because COMPILER_ALLOW_UNSANDBOXED=1; this should only be used for isolated local development"
        );
        return Ok(CompileBackend::Host);
    }

    Err(
        "Compiler service refuses to execute untrusted source on the host by default. Configure COMPILER_SANDBOX_IMAGE for container isolation, or set COMPILER_ALLOW_UNSANDBOXED=1 only for local development."
            .to_string(),
    )
}

fn required_api_key() -> Result<String, String> {
    let api_key = std::env::var("COMPILER_API_KEY").map_err(|_| {
        "COMPILER_API_KEY environment variable is required\n   Set it to a strong random secret (≥32 chars)"
            .to_string()
    })?;

    if api_key.len() < 16 {
        return Err("COMPILER_API_KEY must be at least 16 characters".to_string());
    }

    Ok(api_key)
}

fn compiler_cors_origin() -> Result<HeaderValue, String> {
    let allowed_origin = std::env::var("COMPILER_CORS_ORIGIN")
        .unwrap_or_else(|_| "http://localhost:3000".to_string());

    allowed_origin
        .parse::<HeaderValue>()
        .map_err(|error| format!("Invalid COMPILER_CORS_ORIGIN value: {}", error))
}

fn build_cors(allowed_origin: HeaderValue) -> CorsLayer {
    CorsLayer::new()
        .allow_origin(allowed_origin)
        .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
        .allow_headers([
            axum::http::header::CONTENT_TYPE,
            axum::http::header::AUTHORIZATION,
            HeaderName::from_static(API_KEY_HEADER),
        ])
}

fn env_var_enabled(name: &str) -> bool {
    std::env::var(name)
        .ok()
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

fn compiler_bind_host() -> String {
    std::env::var("COMPILER_BIND")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_COMPILER_HOST.to_string())
}

fn compiler_port() -> u16 {
    std::env::var("PORT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_COMPILER_PORT)
}

fn command_available(program: &str) -> bool {
    Command::new(program)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok()
}

/// Constant-time byte comparison to prevent timing side-channel attacks.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}
