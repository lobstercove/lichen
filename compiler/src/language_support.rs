use std::fs;

use tracing::info;

use super::error_support::{
    parse_asc_errors, parse_cargo_errors_with_locations, parse_cargo_warnings, parse_clang_errors,
};
use super::process_support::{
    create_compiler_temp_dir, path_to_str, sandbox_path_for_host_path, spawn_compiler_process,
    wait_with_timeout,
};
use super::wasm_support::optimize_wasm;
use super::{CompileBackend, CompileError, COMPILE_TIMEOUT};

pub(super) async fn compile_rust(
    code: &str,
    optimize: bool,
    backend: &CompileBackend,
) -> Result<(Vec<u8>, Vec<String>), Vec<CompileError>> {
    info!("🦀 Compiling Rust code...");

    let temp_dir = create_compiler_temp_dir(backend)?;
    let project_dir = temp_dir.path();

    let cargo_toml = format!(
        r#"[package]
name = "wasm-contract"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
# Add lichen-contract-sdk here if needed

[profile.release]
opt-level = {}
lto = true
panic = "abort"
"#,
        if optimize { "\"z\"" } else { "1" }
    );

    fs::write(project_dir.join("Cargo.toml"), cargo_toml).map_err(|error| {
        vec![CompileError {
            file: "Cargo.toml".to_string(),
            line: 0,
            col: 0,
            message: format!("Failed to write Cargo.toml: {}", error),
        }]
    })?;

    fs::create_dir_all(project_dir.join("src")).map_err(|error| {
        vec![CompileError {
            file: "src".to_string(),
            line: 0,
            col: 0,
            message: format!("Failed to create src dir: {}", error),
        }]
    })?;

    fs::write(project_dir.join("src/lib.rs"), code).map_err(|error| {
        vec![CompileError {
            file: "lib.rs".to_string(),
            line: 0,
            col: 0,
            message: format!("Failed to write source file: {}", error),
        }]
    })?;

    let mut child = spawn_compiler_process(
        backend,
        "cargo",
        &["build", "--target", "wasm32-unknown-unknown", "--release"],
        temp_dir.path(),
        project_dir,
    )?;

    let output = wait_with_timeout(&mut child, COMPILE_TIMEOUT)
        .await
        .map_err(|error| {
            vec![CompileError {
                file: "cargo".to_string(),
                line: 0,
                col: 0,
                message: error,
            }]
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(parse_cargo_errors_with_locations(&stderr));
    }

    let wasm_path = project_dir.join("target/wasm32-unknown-unknown/release/wasm_contract.wasm");
    let wasm_bytes = fs::read(&wasm_path).map_err(|error| {
        vec![CompileError {
            file: "output".to_string(),
            line: 0,
            col: 0,
            message: format!("Failed to read WASM output: {}", error),
        }]
    })?;

    let stderr = String::from_utf8_lossy(&output.stderr);
    let warnings = parse_cargo_warnings(&stderr);

    let optimized_wasm = if optimize && matches!(backend, CompileBackend::Host) {
        optimize_wasm(&wasm_bytes).unwrap_or(wasm_bytes)
    } else {
        wasm_bytes
    };

    Ok((optimized_wasm, warnings))
}

pub(super) async fn compile_c(
    code: &str,
    optimize: bool,
    backend: &CompileBackend,
) -> Result<(Vec<u8>, Vec<String>), Vec<CompileError>> {
    info!("🔧 Compiling C/C++ code...");

    let temp_dir = create_compiler_temp_dir(backend)?;
    let source_file = temp_dir.path().join("contract.c");
    let wasm_file = temp_dir.path().join("contract.wasm");

    fs::write(&source_file, code).map_err(|error| {
        vec![CompileError {
            file: "contract.c".to_string(),
            line: 0,
            col: 0,
            message: format!("Failed to write source file: {}", error),
        }]
    })?;

    let wasm_str = match backend {
        CompileBackend::Host => path_to_str(&wasm_file)?,
        CompileBackend::Docker(_) => sandbox_path_for_host_path(temp_dir.path(), &wasm_file)?,
    };
    let source_str = match backend {
        CompileBackend::Host => path_to_str(&source_file)?,
        CompileBackend::Docker(_) => sandbox_path_for_host_path(temp_dir.path(), &source_file)?,
    };

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

    let mut child =
        spawn_compiler_process(backend, "clang", &args, temp_dir.path(), temp_dir.path())?;

    let output = wait_with_timeout(&mut child, COMPILE_TIMEOUT)
        .await
        .map_err(|error| {
            vec![CompileError {
                file: "clang".to_string(),
                line: 0,
                col: 0,
                message: error,
            }]
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(parse_clang_errors(&stderr));
    }

    let wasm_bytes = fs::read(&wasm_file).map_err(|error| {
        vec![CompileError {
            file: "output".to_string(),
            line: 0,
            col: 0,
            message: format!("Failed to read WASM output: {}", error),
        }]
    })?;

    Ok((wasm_bytes, vec![]))
}

pub(super) async fn compile_assemblyscript(
    code: &str,
    optimize: bool,
    backend: &CompileBackend,
) -> Result<(Vec<u8>, Vec<String>), Vec<CompileError>> {
    info!("📜 Compiling AssemblyScript code...");

    let temp_dir = create_compiler_temp_dir(backend)?;
    let source_file = temp_dir.path().join("contract.ts");
    let wasm_file = temp_dir.path().join("contract.wasm");

    fs::write(&source_file, code).map_err(|error| {
        vec![CompileError {
            file: "contract.ts".to_string(),
            line: 0,
            col: 0,
            message: format!("Failed to write source file: {}", error),
        }]
    })?;

    let source_str = match backend {
        CompileBackend::Host => path_to_str(&source_file)?,
        CompileBackend::Docker(_) => sandbox_path_for_host_path(temp_dir.path(), &source_file)?,
    };
    let wasm_str = match backend {
        CompileBackend::Host => path_to_str(&wasm_file)?,
        CompileBackend::Docker(_) => sandbox_path_for_host_path(temp_dir.path(), &wasm_file)?,
    };

    let mut args = vec![
        source_str.as_str(),
        "-o",
        wasm_str.as_str(),
        "--exportRuntime",
    ];

    if optimize {
        args.push("-O3");
    }

    let mut child =
        spawn_compiler_process(backend, "asc", &args, temp_dir.path(), temp_dir.path())?;

    let output = wait_with_timeout(&mut child, COMPILE_TIMEOUT)
        .await
        .map_err(|error| {
            vec![CompileError {
                file: "asc".to_string(),
                line: 0,
                col: 0,
                message: error,
            }]
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(parse_asc_errors(&stderr));
    }

    let wasm_bytes = fs::read(&wasm_file).map_err(|error| {
        vec![CompileError {
            file: "output".to_string(),
            line: 0,
            col: 0,
            message: format!("Failed to read WASM output: {}", error),
        }]
    })?;

    Ok((wasm_bytes, vec![]))
}
