use lichen_core::MAX_CONTRACT_CODE;
use std::{fs, process::Command};
use tempfile::TempDir;
use tracing::warn;

use super::{CompileError, WasmExport};

pub(super) fn validate_wasm_output_size(size: usize) -> Result<(), Vec<CompileError>> {
    if size > MAX_CONTRACT_CODE {
        return Err(vec![CompileError {
            file: "output".to_string(),
            line: 0,
            col: 0,
            message: format!(
                "Compiled WASM output too large: {} bytes (max {} bytes / 512 KB)",
                size, MAX_CONTRACT_CODE
            ),
        }]);
    }

    Ok(())
}

pub(super) fn extract_wasm_exports(wasm_bytes: &[u8]) -> Option<Vec<WasmExport>> {
    if wasm_bytes.len() < 8 {
        return None;
    }
    if &wasm_bytes[0..4] != b"\x00asm" {
        return None;
    }

    let mut exports = Vec::new();
    let mut pos = 8;

    while pos < wasm_bytes.len() {
        if pos + 1 >= wasm_bytes.len() {
            break;
        }
        let section_id = wasm_bytes[pos];
        pos += 1;

        let (section_size, bytes_read) = read_leb128(&wasm_bytes[pos..]);
        pos += bytes_read;
        let section_end = pos + section_size as usize;

        if section_id == 7 {
            let mut export_pos = pos;
            let (export_count, bytes_read) = read_leb128(&wasm_bytes[export_pos..]);
            export_pos += bytes_read;

            for _ in 0..export_count {
                if export_pos >= section_end {
                    break;
                }

                let (name_len, bytes_read) = read_leb128(&wasm_bytes[export_pos..]);
                export_pos += bytes_read;
                let name_end = export_pos + name_len as usize;
                if name_end > section_end {
                    break;
                }
                let name = String::from_utf8_lossy(&wasm_bytes[export_pos..name_end]).to_string();
                export_pos = name_end;

                if export_pos >= section_end {
                    break;
                }
                let kind_byte = wasm_bytes[export_pos];
                export_pos += 1;
                let (_index, bytes_read) = read_leb128(&wasm_bytes[export_pos..]);
                export_pos += bytes_read;

                let kind = match kind_byte {
                    0 => "function",
                    1 => "table",
                    2 => "memory",
                    3 => "global",
                    _ => "unknown",
                };

                if !name.starts_with("__") && name != "memory" {
                    exports.push(WasmExport {
                        name,
                        kind: kind.to_string(),
                    });
                }
            }
            break;
        }

        pos = section_end;
    }

    if exports.is_empty() {
        None
    } else {
        Some(exports)
    }
}

pub(super) fn read_leb128(data: &[u8]) -> (u64, usize) {
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

pub(super) fn optimize_wasm(wasm: &[u8]) -> Result<Vec<u8>, String> {
    let temp_dir = TempDir::new().map_err(|error| error.to_string())?;
    let input_file = temp_dir.path().join("input.wasm");
    let output_file = temp_dir.path().join("output.wasm");

    fs::write(&input_file, wasm).map_err(|error| error.to_string())?;

    let input_str = input_file
        .to_str()
        .ok_or_else(|| "Non-UTF8 temp path for wasm-opt input".to_string())?;
    let output_str = output_file
        .to_str()
        .ok_or_else(|| "Non-UTF8 temp path for wasm-opt output".to_string())?;

    let output = Command::new("wasm-opt")
        .args([
            "-Oz",
            "--strip-debug",
            "--strip-producers",
            input_str,
            "-o",
            output_str,
        ])
        .output();

    match output {
        Ok(output) if output.status.success() => {
            fs::read(&output_file).map_err(|error| error.to_string())
        }
        Ok(output) => {
            warn!(
                "wasm-opt failed, returning unoptimized WASM: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            Ok(wasm.to_vec())
        }
        Err(error) => {
            warn!("wasm-opt not available ({}), skipping optimization", error);
            Ok(wasm.to_vec())
        }
    }
}
