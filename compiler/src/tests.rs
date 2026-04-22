use super::*;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

struct EnvRestore(Vec<(String, Option<String>)>);

impl Drop for EnvRestore {
    fn drop(&mut self) {
        for (key, value) in self.0.drain(..) {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }
}

fn with_env_lock<T>(callback: impl FnOnce() -> T) -> T {
    static ENV_LOCK: Mutex<()> = Mutex::new(());
    let _guard = ENV_LOCK.lock().expect("env test lock poisoned");
    callback()
}

fn override_env(values: &[(&str, Option<&str>)]) -> EnvRestore {
    let mut saved = Vec::with_capacity(values.len());
    for (key, value) in values {
        saved.push(((*key).to_string(), std::env::var(key).ok()));
        match value {
            Some(value) => std::env::set_var(key, value),
            None => std::env::remove_var(key),
        }
    }
    EnvRestore(saved)
}

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
    let (val, consumed) = read_leb128(&[0xE5, 0x8E, 0x26]);
    assert_eq!(val, 624_485);
    assert_eq!(consumed, 3);
}

#[test]
fn test_read_leb128_max_u64_does_not_overflow() {
    let data = [0xFF; 11];
    let (_, consumed) = read_leb128(&data);
    assert!(consumed <= 11);
}

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
    let mut module = Vec::new();
    module.extend_from_slice(b"\x00asm");
    module.extend_from_slice(&[1, 0, 0, 0]);

    module.push(1);
    module.push(4);
    module.push(1);
    module.push(0x60);
    module.push(0);
    module.push(0);

    module.push(3);
    module.push(2);
    module.push(1);
    module.push(0);

    let name = b"add";
    let export_size = 1 + 1 + name.len() + 1 + 1;
    module.push(7);
    module.push(export_size as u8);
    module.push(1);
    module.push(name.len() as u8);
    module.extend_from_slice(name);
    module.push(0);
    module.push(0);

    module.push(10);
    module.push(4);
    module.push(1);
    module.push(2);
    module.push(0);
    module.push(0x0B);

    let exports = extract_wasm_exports(&module).expect("should find exports");
    assert_eq!(exports.len(), 1);
    assert_eq!(exports[0].name, "add");
    assert_eq!(exports[0].kind, "function");
}

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
fn test_parse_cargo_errors_fallback_without_structured_error_output() {
    let stderr = "invalid argument for docker run\n";
    let errors = parse_cargo_errors_with_locations(stderr);
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].message, "Compilation failed (see compiler logs)");
}

#[test]
fn test_parse_cargo_errors_redacts_absolute_temp_path() {
    let stderr = "error[E0308]: mismatched types\n  --> /tmp/compiler-123/src/lib.rs:10:5\n  |\n";
    let errors = parse_cargo_errors_with_locations(stderr);
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].file, "src/lib.rs");
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
fn test_parse_clang_errors_redacts_absolute_temp_path() {
    let stderr = "/tmp/compiler-123/contract.c:15:3: error: expected ';'\n";
    let errors = parse_clang_errors(stderr);
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].file, "contract.c");
    assert!(!errors[0].message.contains("/tmp/compiler-123"));
}

#[test]
fn test_parse_clang_errors_fallback_avoids_raw_stderr() {
    let stderr = "/tmp/private/project/contract.c: note: build failed\n";
    let errors = parse_clang_errors(stderr);
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].message, "Compilation failed (see compiler logs)");
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
fn test_parse_asc_errors_redacts_absolute_temp_path() {
    let stderr = "ERROR TS2322: Type mismatch in /tmp/compiler-123/contract.ts(10,5)\n";
    let errors = parse_asc_errors(stderr);
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].file, "contract.ts");
    assert_eq!(errors[0].message, "ERROR TS2322: Type mismatch");
}

#[test]
fn test_parse_asc_errors_fallback_avoids_raw_stderr() {
    let stderr = "/tmp/private/project/contract.ts failed\n";
    let errors = parse_asc_errors(stderr);
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].message, "Compilation failed (see compiler logs)");
}

#[test]
fn test_parse_cargo_warnings_from_stderr() {
    let stderr = "warning: unused variable: `x`\n  --> src/lib.rs:3:9\nwarning: `foo` (lib) generated 1 warning\n";
    let warnings = parse_cargo_warnings(stderr);
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("unused variable"));
}

#[test]
fn test_max_source_size_constant() {
    assert_eq!(MAX_SOURCE_SIZE, 512 * 1024);
}

#[test]
fn test_validate_wasm_output_size_accepts_limit() {
    assert!(validate_wasm_output_size(MAX_CONTRACT_CODE).is_ok());
}

#[test]
fn test_validate_wasm_output_size_rejects_oversized_artifacts() {
    let errors = validate_wasm_output_size(MAX_CONTRACT_CODE + 1)
        .expect_err("oversized compiler output must be rejected");
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].file, "output");
    assert!(errors[0].message.contains("too large"));
}

#[test]
fn test_path_to_str_valid() {
    let path = PathBuf::from("/tmp/foo.wasm");
    let result = path_to_str(&path);
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), "/tmp/foo.wasm");
}

#[cfg(unix)]
#[test]
fn test_path_to_str_redacts_non_utf8_path() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let path = PathBuf::from(OsString::from_vec(vec![0x66, 0x6f, 0x80]));
    let errors = path_to_str(&path).expect_err("non-utf8 path must fail");
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].message, "Non-UTF8 temporary path");
}

#[test]
fn test_sandbox_path_for_host_path_maps_relative_workspace_paths() {
    let root = Path::new("/tmp/compiler-workspace");
    let path = root.join("src/lib.rs");
    let sandbox_path =
        sandbox_path_for_host_path(root, &path).expect("sandbox path should resolve");
    assert_eq!(sandbox_path, "/workspace/src/lib.rs");
}

#[test]
fn test_resolve_compile_backend_requires_opt_in_without_sandbox() {
    with_env_lock(|| {
        let _restore = override_env(&[
            ("LICHEN_LOCAL_DEV", None),
            ("COMPILER_ALLOW_UNSANDBOXED", None),
            ("COMPILER_SANDBOX_IMAGE", None),
            ("COMPILER_SANDBOX_RUNTIME", None),
        ]);

        let error = resolve_compile_backend().expect_err("unsandboxed production mode must fail");
        assert!(error.contains("COMPILER_SANDBOX_IMAGE"));
    });
}

#[test]
fn test_resolve_compile_backend_accepts_local_dev_host_mode() {
    with_env_lock(|| {
        let _restore = override_env(&[
            ("LICHEN_LOCAL_DEV", Some("1")),
            ("COMPILER_ALLOW_UNSANDBOXED", None),
            ("COMPILER_SANDBOX_IMAGE", None),
            ("COMPILER_SANDBOX_RUNTIME", None),
        ]);

        assert_eq!(resolve_compile_backend().unwrap(), CompileBackend::Host);
    });
}

#[test]
fn test_validate_api_key_rejects_missing_header() {
    let state = AppState {
        api_key: Arc::new("test-key-long-enough".to_string()),
        compile_backend: Arc::new(CompileBackend::Host),
    };
    let headers = HeaderMap::new();
    assert!(validate_api_key(&headers, &state).is_err());
}

#[test]
fn test_validate_api_key_rejects_wrong_key() {
    let state = AppState {
        api_key: Arc::new("correct-key-12345".to_string()),
        compile_backend: Arc::new(CompileBackend::Host),
    };
    let mut headers = HeaderMap::new();
    headers.insert(API_KEY_HEADER, "wrong-key-12345".parse().unwrap());
    assert!(validate_api_key(&headers, &state).is_err());
}

#[test]
fn test_validate_api_key_accepts_correct_key() {
    let state = AppState {
        api_key: Arc::new("correct-key-12345".to_string()),
        compile_backend: Arc::new(CompileBackend::Host),
    };
    let mut headers = HeaderMap::new();
    headers.insert(API_KEY_HEADER, "correct-key-12345".parse().unwrap());
    assert!(validate_api_key(&headers, &state).is_ok());
}
