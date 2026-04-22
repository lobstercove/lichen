// Lichen Compiler Service
// Compile Rust/C/AssemblyScript to WASM for smart contracts

use axum::{
    extract::Json,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use lichen_core::MAX_CONTRACT_CODE;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};
use tracing::{error, info, warn};

mod bootstrap;
mod error_support;
mod language_support;
mod process_support;
#[cfg(test)]
mod tests;
mod wasm_support;

use bootstrap::{
    build_app, build_state, compiler_bind_addr, validate_api_key, AppState, CompileBackend,
    SANDBOX_WORKSPACE,
};
#[cfg(test)]
use bootstrap::{resolve_compile_backend, API_KEY_HEADER};
#[cfg(test)]
use error_support::{
    parse_asc_errors, parse_cargo_errors_with_locations, parse_cargo_warnings, parse_clang_errors,
};
use language_support::{compile_assemblyscript, compile_c, compile_rust};
#[cfg(test)]
use process_support::{path_to_str, sandbox_path_for_host_path};
#[cfg(test)]
use wasm_support::read_leb128;
use wasm_support::{extract_wasm_exports, validate_wasm_output_size};

/// Maximum source code size accepted (512 KB)
const MAX_SOURCE_SIZE: usize = 512 * 1024;
/// Maximum compilation wall-clock time (120 seconds)
const COMPILE_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, Deserialize)]
struct CompileRequest {
    code: String,
    language: String, // "rust", "c", "assemblyscript"
    #[serde(default = "default_optimize")]
    optimize: bool,
}

fn default_optimize() -> bool {
    true
}

#[derive(Debug, Serialize)]
struct CompileResponse {
    success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    wasm: Option<String>, // base64-encoded WASM
    #[serde(skip_serializing_if = "Option::is_none")]
    size: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    time_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    warnings: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    errors: Option<Vec<CompileError>>,
    /// Exported function names extracted from WASM (for ABI generation)
    #[serde(skip_serializing_if = "Option::is_none")]
    exports: Option<Vec<WasmExport>>,
}

#[derive(Debug, Serialize)]
struct WasmExport {
    name: String,
    kind: String, // "function", "memory", "global", "table"
}

#[derive(Debug, Serialize)]
struct CompileError {
    file: String,
    line: usize,
    col: usize,
    message: String,
}

#[tokio::main]
async fn main() {
    // Initialize logging
    tracing_subscriber::fmt::init();

    let (state, compile_backend) = build_state().unwrap_or_else(|message| {
        eprintln!("❌ {}", message);
        std::process::exit(1);
    });

    let app = build_app(state).unwrap_or_else(|message| {
        eprintln!("❌ {}", message);
        std::process::exit(1);
    });

    let addr = compiler_bind_addr();
    info!(
        "🔨 Lichen Compiler Service starting on {} (auth: enabled, backend: {})",
        addr,
        compile_backend.description()
    );

    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .unwrap_or_else(|e| {
            eprintln!("❌ Failed to bind to {}: {}", addr, e);
            std::process::exit(1);
        });
    if let Err(e) = axum::serve(listener, app).await {
        eprintln!("❌ Server error: {}", e);
        std::process::exit(1);
    }
}

/// Health check endpoint
async fn health_handler() -> Response {
    (StatusCode::OK, "OK").into_response()
}

/// P9-INF-01: Authenticated compile handler — validates API key then delegates.
async fn compile_handler_authed(
    headers: HeaderMap,
    body: Json<CompileRequest>,
    state: AppState,
) -> Response {
    if let Err(err) = validate_api_key(&headers, &state) {
        warn!("🔒 Compile request rejected: missing or invalid API key");
        return err.into_response();
    }
    compile_handler(body, state).await
}

/// Compile handler
async fn compile_handler(Json(req): Json<CompileRequest>, state: AppState) -> Response {
    info!("📝 Compile request: language={}", req.language);

    // F7.9: Reject oversized source code
    if req.code.len() > MAX_SOURCE_SIZE {
        return (
            StatusCode::BAD_REQUEST,
            Json(CompileResponse {
                success: false,
                wasm: None,
                size: None,
                time_ms: None,
                warnings: None,
                errors: Some(vec![CompileError {
                    file: "request".to_string(),
                    line: 0,
                    col: 0,
                    message: format!(
                        "Source code too large: {} bytes (max {} bytes)",
                        req.code.len(),
                        MAX_SOURCE_SIZE
                    ),
                }]),
                exports: None,
            }),
        )
            .into_response();
    }

    let start = Instant::now();

    let result = match req.language.to_lowercase().as_str() {
        "rust" => compile_rust(&req.code, req.optimize, state.compile_backend.as_ref()).await,
        "c" | "cpp" | "c++" => {
            compile_c(&req.code, req.optimize, state.compile_backend.as_ref()).await
        }
        "assemblyscript" | "typescript" => {
            compile_assemblyscript(&req.code, req.optimize, state.compile_backend.as_ref()).await
        }
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(CompileResponse {
                    success: false,
                    wasm: None,
                    size: None,
                    time_ms: None,
                    warnings: None,
                    errors: Some(vec![CompileError {
                        file: "request".to_string(),
                        line: 0,
                        col: 0,
                        message: format!("Unsupported language: {}", req.language),
                    }]),
                    exports: None,
                }),
            )
                .into_response();
        }
    };

    let time_ms = start.elapsed().as_millis() as u64;

    match result {
        Ok((wasm_bytes, warnings)) => {
            if let Err(errors) = validate_wasm_output_size(wasm_bytes.len()) {
                warn!(
                    "⚠️ Compiled WASM output rejected: {} bytes exceeds {} byte limit",
                    wasm_bytes.len(),
                    MAX_CONTRACT_CODE
                );
                return (
                    StatusCode::OK,
                    Json(CompileResponse {
                        success: false,
                        wasm: None,
                        size: None,
                        time_ms: Some(time_ms),
                        warnings: if warnings.is_empty() {
                            None
                        } else {
                            Some(warnings)
                        },
                        errors: Some(errors),
                        exports: None,
                    }),
                )
                    .into_response();
            }

            use base64::Engine;
            let wasm_base64 = base64::engine::general_purpose::STANDARD.encode(&wasm_bytes);
            let size = wasm_bytes.len();

            // Extract WASM exports for ABI hints
            let exports = extract_wasm_exports(&wasm_bytes);

            info!(
                "✅ Compilation successful: {} bytes in {}ms, {} exports",
                size,
                time_ms,
                exports.as_ref().map(|e| e.len()).unwrap_or(0)
            );

            (
                StatusCode::OK,
                Json(CompileResponse {
                    success: true,
                    wasm: Some(wasm_base64),
                    size: Some(size),
                    time_ms: Some(time_ms),
                    warnings: if warnings.is_empty() {
                        None
                    } else {
                        Some(warnings)
                    },
                    errors: None,
                    exports,
                }),
            )
                .into_response()
        }
        Err(errors) => {
            error!("❌ Compilation failed: {:?}", errors);

            (
                StatusCode::OK,
                Json(CompileResponse {
                    success: false,
                    wasm: None,
                    size: None,
                    time_ms: Some(time_ms),
                    warnings: None,
                    errors: Some(errors),
                    exports: None,
                }),
            )
                .into_response()
        }
    }
}
