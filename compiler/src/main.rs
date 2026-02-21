// MoltChain Compiler Service
// Compile Rust/C/AssemblyScript to WASM for smart contracts

use axum::{
    extract::Json,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Router,
};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tempfile::TempDir;
use tower_http::cors::CorsLayer;
use tracing::{error, info, warn};

/// Maximum source code size accepted (512 KB)
const MAX_SOURCE_SIZE: usize = 512 * 1024;
/// Maximum compilation wall-clock time (120 seconds)
const COMPILE_TIMEOUT: Duration = Duration::from_secs(120);
/// HTTP header name for API key authentication (P9-INF-01)
const API_KEY_HEADER: &str = "x-api-key";

/// Shared application state holding the required API key.
#[derive(Clone)]
struct AppState {
    api_key: Arc<String>,
}

/// P9-INF-01: Validate the X-API-Key header against the configured key.
/// Returns Ok(()) on success, or an error response on failure.
fn validate_api_key(headers: &HeaderMap, state: &AppState) -> Result<(), Response> {
    match headers.get(API_KEY_HEADER).and_then(|v| v.to_str().ok()) {
        Some(provided) if provided == state.api_key.as_str() => Ok(()),
        Some(_) => Err((
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": "Invalid API key"})),
        )
            .into_response()),
        None => Err((
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": "Missing X-API-Key header"})),
        )
            .into_response()),
    }
}

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
    kind: String,  // "function", "memory", "global", "table"
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

    // P9-INF-01: Require COMPILER_API_KEY environment variable
    let api_key = std::env::var("COMPILER_API_KEY").unwrap_or_else(|_| {
        eprintln!("❌ COMPILER_API_KEY environment variable is required");
        eprintln!("   Set it to a strong random secret (≥32 chars)");
        std::process::exit(1);
    });
    if api_key.len() < 16 {
        eprintln!("❌ COMPILER_API_KEY must be at least 16 characters");
        std::process::exit(1);
    }

    let state = AppState {
        api_key: Arc::new(api_key),
    };

    let port = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8900);

    // Build router — health is unauthenticated; compile requires API key
    let app = Router::new()
        .route("/compile", post({
            let state = state.clone();
            move |headers: HeaderMap, body: Json<CompileRequest>| {
                compile_handler_authed(headers, body, state)
            }
        }))
        .route("/health", axum::routing::get(health_handler))
        // P9-INF-08: Restrict CORS to known developer portal origins instead of
        // permissive wildcard. In production, set COMPILER_CORS_ORIGIN env var.
        .layer({
            let allowed_origin = std::env::var("COMPILER_CORS_ORIGIN")
                .unwrap_or_else(|_| "http://localhost:3000".to_string());
            CorsLayer::new()
                .allow_origin(allowed_origin.parse::<axum::http::HeaderValue>()
                    .unwrap_or_else(|e| {
                        eprintln!("Invalid COMPILER_CORS_ORIGIN value: {}", e);
                        std::process::exit(1);
                    }))
                .allow_methods([axum::http::Method::GET, axum::http::Method::POST, axum::http::Method::OPTIONS])
                .allow_headers([axum::http::header::CONTENT_TYPE, axum::http::header::AUTHORIZATION,
                    axum::http::HeaderName::from_static("x-api-key")])
        });

    let addr = format!("0.0.0.0:{}", port);
    info!("🔨 MoltChain Compiler Service starting on {} (auth: enabled)", addr);

    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap_or_else(|e| {
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
    if let Err(resp) = validate_api_key(&headers, &state) {
        warn!("🔒 Compile request rejected: missing or invalid API key");
        return resp;
    }
    compile_handler(body).await
}

/// Compile handler
async fn compile_handler(Json(req): Json<CompileRequest>) -> Response {
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
        "rust" => compile_rust(&req.code, req.optimize).await,
        "c" | "cpp" | "c++" => compile_c(&req.code, req.optimize).await,
        "assemblyscript" | "typescript" => compile_assemblyscript(&req.code, req.optimize).await,
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
            use base64::Engine;
            let wasm_base64 = base64::engine::general_purpose::STANDARD.encode(&wasm_bytes);
            let size = wasm_bytes.len();

            // Extract WASM exports for ABI hints
            let exports = extract_wasm_exports(&wasm_bytes);

            info!("✅ Compilation successful: {} bytes in {}ms, {} exports", size, time_ms, exports.as_ref().map(|e| e.len()).unwrap_or(0));

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

/// Extract exported function names from WASM bytecode (lightweight, no full WASM runtime)
fn extract_wasm_exports(wasm_bytes: &[u8]) -> Option<Vec<WasmExport>> {
    // Parse WASM binary header to find export section
    // WASM magic: \0asm, version: 1
    if wasm_bytes.len() < 8 {
        return None;
    }
    if &wasm_bytes[0..4] != b"\x00asm" {
        return None;
    }

    let mut exports = Vec::new();
    let mut pos = 8; // skip magic + version

    while pos < wasm_bytes.len() {
        if pos + 1 >= wasm_bytes.len() {
            break;
        }
        let section_id = wasm_bytes[pos];
        pos += 1;

        // Read LEB128 section size
        let (section_size, bytes_read) = read_leb128(&wasm_bytes[pos..]);
        pos += bytes_read;
        let section_end = pos + section_size as usize;

        if section_id == 7 {
            // Export section
            let mut epos = pos;
            let (export_count, br) = read_leb128(&wasm_bytes[epos..]);
            epos += br;

            for _ in 0..export_count {
                if epos >= section_end {
                    break;
                }
                // Read name length + name
                let (name_len, br) = read_leb128(&wasm_bytes[epos..]);
                epos += br;
                let name_end = epos + name_len as usize;
                if name_end > section_end {
                    break;
                }
                let name = String::from_utf8_lossy(&wasm_bytes[epos..name_end]).to_string();
                epos = name_end;

                // Export kind (1 byte) + index (LEB128)
                if epos >= section_end {
                    break;
                }
                let kind_byte = wasm_bytes[epos];
                epos += 1;
                let (_index, br) = read_leb128(&wasm_bytes[epos..]);
                epos += br;

                let kind = match kind_byte {
                    0 => "function",
                    1 => "table",
                    2 => "memory",
                    3 => "global",
                    _ => "unknown",
                };

                // Skip internal WASM exports
                if !name.starts_with("__") && name != "memory" {
                    exports.push(WasmExport {
                        name,
                        kind: kind.to_string(),
                    });
                }
            }
            break; // Found the export section, stop
        }

        pos = section_end;
    }

    if exports.is_empty() {
        None
    } else {
        Some(exports)
    }
}

/// Read a LEB128 unsigned integer, return (value, bytes_consumed).
/// Returns (0, 0) if data is empty — callers must check bytes_consumed > 0
/// for validity when needed.
fn read_leb128(data: &[u8]) -> (u64, usize) {
    if data.is_empty() {
        return (0, 0);
    }
    let mut result: u64 = 0;
    let mut shift: u32 = 0;
    let mut pos = 0;
    loop {
        if pos >= data.len() {
            break;
        }
        let byte = data[pos];
        // Guard against shift overflow: LEB128 for u64 is at most 10 bytes (70 bits)
        if shift >= 64 {
            pos += 1;
            break;
        }
        result |= ((byte & 0x7F) as u64) << shift;
        pos += 1;
        if byte & 0x80 == 0 {
            break;
        }
        shift += 7;
    }
    (result, pos)
}

/// Compile Rust to WASM
async fn compile_rust(
    code: &str,
    optimize: bool,
) -> Result<(Vec<u8>, Vec<String>), Vec<CompileError>> {
    info!("🦀 Compiling Rust code...");

    // Create temporary directory
    let temp_dir = TempDir::new().map_err(|e| {
        vec![CompileError {
            file: "system".to_string(),
            line: 0,
            col: 0,
            message: format!("Failed to create temp dir: {}", e),
        }]
    })?;

    let project_dir = temp_dir.path();

    // Create Cargo.toml
    let cargo_toml = format!(
        r#"[package]
name = "wasm-contract"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
# Add moltchain-sdk here if needed

[profile.release]
opt-level = {}
lto = true
panic = "abort"
"#,
        if optimize { "\"z\"" } else { "1" }
    );

    fs::write(project_dir.join("Cargo.toml"), cargo_toml).map_err(|e| {
        vec![CompileError {
            file: "Cargo.toml".to_string(),
            line: 0,
            col: 0,
            message: format!("Failed to write Cargo.toml: {}", e),
        }]
    })?;

    // Create src/lib.rs
    fs::create_dir_all(project_dir.join("src")).map_err(|e| {
        vec![CompileError {
            file: "src".to_string(),
            line: 0,
            col: 0,
            message: format!("Failed to create src dir: {}", e),
        }]
    })?;

    fs::write(project_dir.join("src/lib.rs"), code).map_err(|e| {
        vec![CompileError {
            file: "lib.rs".to_string(),
            line: 0,
            col: 0,
            message: format!("Failed to write source file: {}", e),
        }]
    })?;

    // Run cargo build with timeout (F7.10)
    let mut child = Command::new("cargo")
        .args(&[
            "build",
            "--target",
            "wasm32-unknown-unknown",
            "--release",
        ])
        .current_dir(project_dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| {
            vec![CompileError {
                file: "cargo".to_string(),
                line: 0,
                col: 0,
                message: format!("Failed to run cargo: {}", e),
            }]
        })?;

    let output = wait_with_timeout(&mut child, COMPILE_TIMEOUT).map_err(|e| {
        vec![CompileError {
            file: "cargo".to_string(),
            line: 0,
            col: 0,
            message: e,
        }]
    })?;

    if !output.status.success() {
        // Parse cargo errors
        let stderr = String::from_utf8_lossy(&output.stderr);
        let errors = parse_cargo_errors_with_locations(&stderr);
        return Err(errors);
    }

    // Read WASM file
    let wasm_path = project_dir
        .join("target/wasm32-unknown-unknown/release/wasm_contract.wasm");

    let wasm_bytes = fs::read(&wasm_path).map_err(|e| {
        vec![CompileError {
            file: "output".to_string(),
            line: 0,
            col: 0,
            message: format!("Failed to read WASM output: {}", e),
        }]
    })?;

    // Parse warnings from stderr (cargo/rustc emit warnings to stderr)
    let stderr_str = String::from_utf8_lossy(&output.stderr);
    let warnings = parse_cargo_warnings(&stderr_str);

    // Optional: Optimize WASM with wasm-opt
    let optimized_wasm = if optimize {
        optimize_wasm(&wasm_bytes).unwrap_or(wasm_bytes)
    } else {
        wasm_bytes
    };

    Ok((optimized_wasm, warnings))
}

/// Compile C/C++ to WASM
async fn compile_c(
    code: &str,
    optimize: bool,
) -> Result<(Vec<u8>, Vec<String>), Vec<CompileError>> {
    info!("🔧 Compiling C/C++ code...");

    // Create temporary directory
    let temp_dir = TempDir::new().map_err(|e| {
        vec![CompileError {
            file: "system".to_string(),
            line: 0,
            col: 0,
            message: format!("Failed to create temp dir: {}", e),
        }]
    })?;

    let source_file = temp_dir.path().join("contract.c");
    let wasm_file = temp_dir.path().join("contract.wasm");

    // Write source file
    fs::write(&source_file, code).map_err(|e| {
        vec![CompileError {
            file: "contract.c".to_string(),
            line: 0,
            col: 0,
            message: format!("Failed to write source file: {}", e),
        }]
    })?;

    // Compile with clang (F7.4: safe path conversion, F7.10: timeout)
    let wasm_str = path_to_str(&wasm_file)?;
    let source_str = path_to_str(&source_file)?;

    let mut args = vec![
        "--target=wasm32",
        "-nostdlib",
        "-Wl,--no-entry",
        "-Wl,--export-all",
        "-o",
        &wasm_str,
        &source_str,
    ];

    if optimize {
        args.push("-O3");
    }

    let mut child = Command::new("clang")
        .args(&args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| {
            vec![CompileError {
                file: "clang".to_string(),
                line: 0,
                col: 0,
                message: format!("Failed to run clang: {}", e),
            }]
        })?;

    let output = wait_with_timeout(&mut child, COMPILE_TIMEOUT).map_err(|e| {
        vec![CompileError {
            file: "clang".to_string(),
            line: 0,
            col: 0,
            message: e,
        }]
    })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let errors = parse_clang_errors(&stderr);
        return Err(errors);
    }

    // Read WASM file
    let wasm_bytes = fs::read(&wasm_file).map_err(|e| {
        vec![CompileError {
            file: "output".to_string(),
            line: 0,
            col: 0,
            message: format!("Failed to read WASM output: {}", e),
        }]
    })?;

    Ok((wasm_bytes, vec![]))
}

/// Compile AssemblyScript to WASM
async fn compile_assemblyscript(
    code: &str,
    optimize: bool,
) -> Result<(Vec<u8>, Vec<String>), Vec<CompileError>> {
    info!("📜 Compiling AssemblyScript code...");

    // Create temporary directory
    let temp_dir = TempDir::new().map_err(|e| {
        vec![CompileError {
            file: "system".to_string(),
            line: 0,
            col: 0,
            message: format!("Failed to create temp dir: {}", e),
        }]
    })?;

    let source_file = temp_dir.path().join("contract.ts");
    let wasm_file = temp_dir.path().join("contract.wasm");

    // Write source file
    fs::write(&source_file, code).map_err(|e| {
        vec![CompileError {
            file: "contract.ts".to_string(),
            line: 0,
            col: 0,
            message: format!("Failed to write source file: {}", e),
        }]
    })?;

    // Compile with asc (F7.4: safe path conversion, F7.10: timeout)
    let source_str = path_to_str(&source_file)?;
    let wasm_str = path_to_str(&wasm_file)?;

    let mut args = vec![
        source_str.as_str(),
        "-o",
        wasm_str.as_str(),
        "--exportRuntime",
    ];

    if optimize {
        args.push("-O3");
    }

    let mut child = Command::new("asc")
        .args(&args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| {
            vec![CompileError {
                file: "asc".to_string(),
                line: 0,
                col: 0,
                message: format!("Failed to run asc: {}", e),
            }]
        })?;

    let output = wait_with_timeout(&mut child, COMPILE_TIMEOUT).map_err(|e| {
        vec![CompileError {
            file: "asc".to_string(),
            line: 0,
            col: 0,
            message: e,
        }]
    })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let errors = parse_asc_errors(&stderr);
        return Err(errors);
    }

    // Read WASM file
    let wasm_bytes = fs::read(&wasm_file).map_err(|e| {
        vec![CompileError {
            file: "output".to_string(),
            line: 0,
            col: 0,
            message: format!("Failed to read WASM output: {}", e),
        }]
    })?;

    Ok((wasm_bytes, vec![]))
}

/// Optimize WASM with wasm-opt (F7.5: safe path conversion)
fn optimize_wasm(wasm: &[u8]) -> Result<Vec<u8>, String> {
    let temp_dir = TempDir::new().map_err(|e| e.to_string())?;
    let input_file = temp_dir.path().join("input.wasm");
    let output_file = temp_dir.path().join("output.wasm");

    fs::write(&input_file, wasm).map_err(|e| e.to_string())?;

    let input_str = input_file
        .to_str()
        .ok_or_else(|| "Non-UTF8 temp path for wasm-opt input".to_string())?;
    let output_str = output_file
        .to_str()
        .ok_or_else(|| "Non-UTF8 temp path for wasm-opt output".to_string())?;

    let output = Command::new("wasm-opt")
        .args(&[
            "-Oz", // Optimize for size
            "--strip-debug",
            "--strip-producers",
            input_str,
            "-o",
            output_str,
        ])
        .output();

    match output {
        Ok(out) if out.status.success() => {
            fs::read(&output_file).map_err(|e| e.to_string())
        }
        Ok(out) => {
            warn!("wasm-opt failed, returning unoptimized WASM: {}",
                  String::from_utf8_lossy(&out.stderr));
            Ok(wasm.to_vec())
        }
        Err(e) => {
            // wasm-opt not installed — return unoptimized
            warn!("wasm-opt not available ({}), skipping optimization", e);
            Ok(wasm.to_vec())
        }
    }
}

// ===== Error Parsing =====

/// Parse cargo/rustc stderr to extract location-aware errors.
/// Rustc emits errors in the form:
///   error[E0308]: mismatched types
///     --> src/lib.rs:10:5
fn parse_cargo_errors_with_locations(stderr: &str) -> Vec<CompileError> {
    let mut errors = Vec::new();
    let lines: Vec<&str> = stderr.lines().collect();

    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if line.starts_with("error") {
            let message = line.to_string();
            let mut file = "lib.rs".to_string();
            let mut err_line: usize = 1;
            let mut err_col: usize = 1;

            // Look ahead for " --> file:line:col"
            if i + 1 < lines.len() {
                let next = lines[i + 1].trim();
                if let Some(loc) = next.strip_prefix("--> ") {
                    let parts: Vec<&str> = loc.rsplitn(3, ':').collect();
                    if parts.len() == 3 {
                        err_col = parts[0].parse().unwrap_or(1);
                        err_line = parts[1].parse().unwrap_or(1);
                        file = parts[2].to_string();
                    }
                }
            }

            errors.push(CompileError {
                file,
                line: err_line,
                col: err_col,
                message,
            });
        }
        i += 1;
    }

    if errors.is_empty() && stderr.contains("error") {
        errors.push(CompileError {
            file: "lib.rs".to_string(),
            line: 0,
            col: 0,
            message: "Compilation failed (see logs)".to_string(),
        });
    }

    errors
}

/// Parse warnings from cargo/rustc stderr output.
fn parse_cargo_warnings(stderr: &str) -> Vec<String> {
    let mut warnings = Vec::new();

    for line in stderr.lines() {
        if line.contains("warning") && !line.contains("generated") {
            warnings.push(line.to_string());
        }
    }

    warnings
}

/// Parse clang errors with location extraction.
/// Clang format: "file.c:10:5: error: ..."
fn parse_clang_errors(stderr: &str) -> Vec<CompileError> {
    let mut errors = Vec::new();

    for line in stderr.lines() {
        if line.contains("error:") {
            let parts: Vec<&str> = line.splitn(4, ':').collect();
            if parts.len() >= 4 {
                let file = parts[0].to_string();
                let err_line = parts[1].parse().unwrap_or(1);
                let err_col = parts[2].parse().unwrap_or(1);
                let message = parts[3..].join(":").trim().to_string();
                errors.push(CompileError {
                    file,
                    line: err_line,
                    col: err_col,
                    message,
                });
            } else {
                errors.push(CompileError {
                    file: "contract.c".to_string(),
                    line: 1,
                    col: 1,
                    message: line.to_string(),
                });
            }
        }
    }

    if errors.is_empty() {
        errors.push(CompileError {
            file: "contract.c".to_string(),
            line: 1,
            col: 1,
            message: stderr.to_string(),
        });
    }

    errors
}

/// Parse AssemblyScript compiler errors.
/// asc format: "ERROR TS2322: ... in contract.ts(10,5)"
fn parse_asc_errors(stderr: &str) -> Vec<CompileError> {
    let mut errors = Vec::new();

    for line in stderr.lines() {
        if line.starts_with("ERROR") || line.contains("error") {
            // Try to extract location from "in file.ts(line,col)"
            let mut file = "contract.ts".to_string();
            let mut err_line: usize = 1;
            let mut err_col: usize = 1;

            if let Some(in_pos) = line.rfind(" in ") {
                let loc_part = &line[in_pos + 4..];
                if let Some(paren) = loc_part.find('(') {
                    file = loc_part[..paren].to_string();
                    let coords = &loc_part[paren + 1..loc_part.len().saturating_sub(1)];
                    let coord_parts: Vec<&str> = coords.split(',').collect();
                    if coord_parts.len() >= 2 {
                        err_line = coord_parts[0].trim().parse().unwrap_or(1);
                        err_col = coord_parts[1].trim().parse().unwrap_or(1);
                    }
                }
            }

            errors.push(CompileError {
                file,
                line: err_line,
                col: err_col,
                message: line.to_string(),
            });
        }
    }

    if errors.is_empty() {
        errors.push(CompileError {
            file: "contract.ts".to_string(),
            line: 1,
            col: 1,
            message: stderr.to_string(),
        });
    }

    errors
}

/// Convert a PathBuf to &str, returning a compile error if the path contains non-UTF8.
fn path_to_str(path: &PathBuf) -> Result<String, Vec<CompileError>> {
    path.to_str().map(|s| s.to_string()).ok_or_else(|| {
        vec![CompileError {
            file: "system".to_string(),
            line: 0,
            col: 0,
            message: format!("Non-UTF8 path: {:?}", path),
        }]
    })
}

/// Wait for a child process with a timeout. Kills the process if it exceeds the deadline.
fn wait_with_timeout(
    child: &mut std::process::Child,
    timeout: Duration,
) -> Result<std::process::Output, String> {
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                // Process finished — collect output
                let stdout = child
                    .stdout
                    .take()
                    .map(|mut s| {
                        let mut buf = Vec::new();
                        std::io::Read::read_to_end(&mut s, &mut buf).ok();
                        buf
                    })
                    .unwrap_or_default();
                let stderr = child
                    .stderr
                    .take()
                    .map(|mut s| {
                        let mut buf = Vec::new();
                        std::io::Read::read_to_end(&mut s, &mut buf).ok();
                        buf
                    })
                    .unwrap_or_default();
                return Ok(std::process::Output {
                    status,
                    stdout,
                    stderr,
                });
            }
            Ok(None) => {
                // Still running
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    let _ = child.wait(); // reap
                    return Err(format!(
                        "Compilation timed out after {} seconds",
                        timeout.as_secs()
                    ));
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => {
                return Err(format!("Failed to wait for compiler process: {}", e));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── LEB128 ──────────────────────────────────────────────

    #[test]
    fn test_read_leb128_empty() {
        let (val, consumed) = read_leb128(&[]);
        assert_eq!(val, 0);
        assert_eq!(consumed, 0);
    }

    #[test]
    fn test_read_leb128_single_byte() {
        let (val, consumed) = read_leb128(&[0x05]);
        assert_eq!(val, 5);
        assert_eq!(consumed, 1);
    }

    #[test]
    fn test_read_leb128_multibyte() {
        // 624,485 = 0x98765 → LEB128: [0xE5, 0x8E, 0x26]
        let (val, consumed) = read_leb128(&[0xE5, 0x8E, 0x26]);
        assert_eq!(val, 624_485);
        assert_eq!(consumed, 3);
    }

    #[test]
    fn test_read_leb128_max_u64_does_not_overflow() {
        // 10 continuation bytes then a final byte — hits the shift >= 64 guard
        let data = [0xFF; 11];
        let (_, consumed) = read_leb128(&data);
        // Should terminate without panic, consuming up to 10+1 bytes
        assert!(consumed <= 11);
    }

    // ── WASM export extraction ──────────────────────────────

    #[test]
    fn test_extract_wasm_exports_too_short() {
        assert!(extract_wasm_exports(&[]).is_none());
        assert!(extract_wasm_exports(&[0; 4]).is_none());
    }

    #[test]
    fn test_extract_wasm_exports_bad_magic() {
        let mut data = vec![0; 8];
        data[0..4].copy_from_slice(b"NOPE");
        assert!(extract_wasm_exports(&data).is_none());
    }

    #[test]
    fn test_extract_wasm_exports_minimal_module() {
        // Build a minimal valid WASM module with one function export
        let mut module = Vec::new();
        module.extend_from_slice(b"\x00asm"); // magic
        module.extend_from_slice(&[1, 0, 0, 0]); // version 1

        // Type section (id=1): one function type () -> ()
        module.push(1); // section id
        module.push(4); // section size
        module.push(1); // 1 type
        module.push(0x60); // func type
        module.push(0); // 0 params
        module.push(0); // 0 results

        // Function section (id=3): one function referencing type 0
        module.push(3); // section id
        module.push(2); // section size
        module.push(1); // 1 function
        module.push(0); // type index 0

        // Export section (id=7): export "add" as function 0
        let name = b"add";
        let export_size = 1 /*count*/ + 1 /*name_len*/ + name.len() + 1 /*kind*/ + 1 /*index*/;
        module.push(7); // section id
        module.push(export_size as u8); // section size
        module.push(1); // 1 export
        module.push(name.len() as u8); // name length
        module.extend_from_slice(name); // name
        module.push(0); // kind = function
        module.push(0); // function index

        // Code section (id=10): one empty body
        module.push(10); // section id
        module.push(4); // section size
        module.push(1); // 1 body
        module.push(2); // body size
        module.push(0); // 0 locals
        module.push(0x0B); // end

        let exports = extract_wasm_exports(&module).expect("should find exports");
        assert_eq!(exports.len(), 1);
        assert_eq!(exports[0].name, "add");
        assert_eq!(exports[0].kind, "function");
    }

    // ── Error parsers ───────────────────────────────────────

    #[test]
    fn test_parse_cargo_errors_extracts_location() {
        let stderr = "error[E0308]: mismatched types\n  --> src/lib.rs:10:5\n  |\n";
        let errors = parse_cargo_errors_with_locations(stderr);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].file, "src/lib.rs");
        assert_eq!(errors[0].line, 10);
        assert_eq!(errors[0].col, 5);
        assert!(errors[0].message.contains("mismatched types"));
    }

    #[test]
    fn test_parse_cargo_errors_no_location() {
        let stderr = "error: could not compile `foo`\n";
        let errors = parse_cargo_errors_with_locations(stderr);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].file, "lib.rs");
        assert_eq!(errors[0].line, 1);
    }

    #[test]
    fn test_parse_clang_errors_with_location() {
        let stderr = "contract.c:15:3: error: expected ';'\n";
        let errors = parse_clang_errors(stderr);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].file, "contract.c");
        assert_eq!(errors[0].line, 15);
        assert_eq!(errors[0].col, 3);
    }

    #[test]
    fn test_parse_asc_errors_with_location() {
        let stderr = "ERROR TS2322: Type mismatch in contract.ts(10,5)\n";
        let errors = parse_asc_errors(stderr);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].file, "contract.ts");
        assert_eq!(errors[0].line, 10);
        assert_eq!(errors[0].col, 5);
    }

    #[test]
    fn test_parse_cargo_warnings_from_stderr() {
        let stderr = "warning: unused variable: `x`\n  --> src/lib.rs:3:9\nwarning: `foo` (lib) generated 1 warning\n";
        let warnings = parse_cargo_warnings(stderr);
        // Should include the actual warning but not the "generated N warnings" summary
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("unused variable"));
    }

    // ── Source size limit ───────────────────────────────────

    #[test]
    fn test_max_source_size_constant() {
        assert_eq!(MAX_SOURCE_SIZE, 512 * 1024);
    }

    // ── path_to_str ─────────────────────────────────────────

    #[test]
    fn test_path_to_str_valid() {
        let p = PathBuf::from("/tmp/foo.wasm");
        let result = path_to_str(&p);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "/tmp/foo.wasm");
    }

    // ── P9-INF-01: API key authentication ───────────────────

    #[test]
    fn test_validate_api_key_rejects_missing_header() {
        let state = AppState {
            api_key: Arc::new("test-key-long-enough".to_string()),
        };
        let headers = HeaderMap::new();
        assert!(validate_api_key(&headers, &state).is_err());
    }

    #[test]
    fn test_validate_api_key_rejects_wrong_key() {
        let state = AppState {
            api_key: Arc::new("correct-key-12345".to_string()),
        };
        let mut headers = HeaderMap::new();
        headers.insert(API_KEY_HEADER, "wrong-key-12345".parse().unwrap());
        assert!(validate_api_key(&headers, &state).is_err());
    }

    #[test]
    fn test_validate_api_key_accepts_correct_key() {
        let state = AppState {
            api_key: Arc::new("correct-key-12345".to_string()),
        };
        let mut headers = HeaderMap::new();
        headers.insert(API_KEY_HEADER, "correct-key-12345".parse().unwrap());
        assert!(validate_api_key(&headers, &state).is_ok());
    }
}