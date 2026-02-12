// MoltChain Compiler Service
// Compile Rust/C/AssemblyScript to WASM for smart contracts

use axum::{
    extract::Json,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
    Router,
};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::Instant;
use tempfile::TempDir;
use tower_http::cors::CorsLayer;
use tracing::{error, info};

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

    let port = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8900);

    // Build router
    let app = Router::new()
        .route("/compile", post(compile_handler))
        .route("/health", axum::routing::get(health_handler))
        .layer(CorsLayer::permissive());

    let addr = format!("0.0.0.0:{}", port);
    info!("🔨 MoltChain Compiler Service starting on {}", addr);

    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

/// Health check endpoint
async fn health_handler() -> Response {
    (StatusCode::OK, "OK").into_response()
}

/// Compile handler
async fn compile_handler(Json(req): Json<CompileRequest>) -> Response {
    info!("📝 Compile request: language={}", req.language);

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
            let wasm_base64 = base64::encode(&wasm_bytes);
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

/// Read a LEB128 unsigned integer, return (value, bytes_consumed)
fn read_leb128(data: &[u8]) -> (u64, usize) {
    let mut result: u64 = 0;
    let mut shift = 0;
    let mut pos = 0;
    loop {
        if pos >= data.len() {
            break;
        }
        let byte = data[pos];
        result |= ((byte & 0x7F) as u64) << shift;
        pos += 1;
        if byte & 0x80 == 0 {
            break;
        }
        shift += 7;
        if shift >= 64 {
            break;
        }
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

    // Run cargo build
    let output = Command::new("cargo")
        .args(&[
            "build",
            "--target",
            "wasm32-unknown-unknown",
            "--release",
        ])
        .current_dir(project_dir)
        .output()
        .map_err(|e| {
            vec![CompileError {
                file: "cargo".to_string(),
                line: 0,
                col: 0,
                message: format!("Failed to run cargo: {}", e),
            }]
        })?;

    if !output.status.success() {
        // Parse cargo errors
        let stderr = String::from_utf8_lossy(&output.stderr);
        let errors = parse_cargo_errors(&stderr);
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

    // Parse warnings
    let stdout = String::from_utf8_lossy(&output.stdout);
    let warnings = parse_cargo_warnings(&stdout);

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

    // Compile with clang
    let mut args = vec![
        "--target=wasm32",
        "-nostdlib",
        "-Wl,--no-entry",
        "-Wl,--export-all",
        "-o",
    ];

    args.push(wasm_file.to_str().unwrap());
    args.push(source_file.to_str().unwrap());

    if optimize {
        args.push("-O3");
    }

    let output = Command::new("clang")
        .args(&args)
        .output()
        .map_err(|e| {
            vec![CompileError {
                file: "clang".to_string(),
                line: 0,
                col: 0,
                message: format!("Failed to run clang: {}", e),
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

    // Compile with asc
    let mut args = vec![
        source_file.to_str().unwrap(),
        "-o",
        wasm_file.to_str().unwrap(),
        "--exportRuntime",
    ];

    if optimize {
        args.push("-O3");
    }

    let output = Command::new("asc")
        .args(&args)
        .output()
        .map_err(|e| {
            vec![CompileError {
                file: "asc".to_string(),
                line: 0,
                col: 0,
                message: format!("Failed to run asc: {}", e),
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

/// Optimize WASM with wasm-opt
fn optimize_wasm(wasm: &[u8]) -> Result<Vec<u8>, String> {
    let temp_dir = TempDir::new().map_err(|e| e.to_string())?;
    let input_file = temp_dir.path().join("input.wasm");
    let output_file = temp_dir.path().join("output.wasm");

    fs::write(&input_file, wasm).map_err(|e| e.to_string())?;

    let output = Command::new("wasm-opt")
        .args(&[
            "-Oz", // Optimize for size
            "--strip-debug",
            "--strip-producers",
            input_file.to_str().unwrap(),
            "-o",
            output_file.to_str().unwrap(),
        ])
        .output()
        .map_err(|e| e.to_string())?;

    if !output.status.success() {
        return Err(format!(
            "wasm-opt failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    fs::read(&output_file).map_err(|e| e.to_string())
}

// ===== Error Parsing =====

fn parse_cargo_errors(stderr: &str) -> Vec<CompileError> {
    let mut errors = Vec::new();

    for line in stderr.lines() {
        if line.contains("error") {
            // Try to parse cargo error format
            // Example: "error: expected `;`, found `}`"
            //          " --> src/lib.rs:10:5"

            if let Some(err) = parse_cargo_error_line(line) {
                errors.push(err);
            }
        }
    }

    if errors.is_empty() {
        errors.push(CompileError {
            file: "lib.rs".to_string(),
            line: 0,
            col: 0,
            message: "Compilation failed (see logs)".to_string(),
        });
    }

    errors
}

fn parse_cargo_error_line(line: &str) -> Option<CompileError> {
    // Simple parsing - in production, use proper parser
    Some(CompileError {
        file: "lib.rs".to_string(),
        line: 1,
        col: 1,
        message: line.to_string(),
    })
}

fn parse_cargo_warnings(stdout: &str) -> Vec<String> {
    let mut warnings = Vec::new();

    for line in stdout.lines() {
        if line.contains("warning") {
            warnings.push(line.to_string());
        }
    }

    warnings
}

fn parse_clang_errors(stderr: &str) -> Vec<CompileError> {
    vec![CompileError {
        file: "contract.c".to_string(),
        line: 1,
        col: 1,
        message: stderr.to_string(),
    }]
}

fn parse_asc_errors(stderr: &str) -> Vec<CompileError> {
    vec![CompileError {
        file: "contract.ts".to_string(),
        line: 1,
        col: 1,
        message: stderr.to_string(),
    }]
}
