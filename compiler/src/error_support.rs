use std::path::Path;

use super::CompileError;

/// Parse cargo/rustc stderr to extract location-aware errors.
/// Rustc emits errors in the form:
///   error[E0308]: mismatched types
///     --> src/lib.rs:10:5
pub(super) fn parse_cargo_errors_with_locations(stderr: &str) -> Vec<CompileError> {
    let mut errors = Vec::new();
    let lines: Vec<&str> = stderr.lines().collect();

    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        if line.starts_with("error") {
            let message = line.to_string();
            let mut file = "lib.rs".to_string();
            let mut err_line: usize = 1;
            let mut err_col: usize = 1;

            if index + 1 < lines.len() {
                let next = lines[index + 1].trim();
                if let Some(location) = next.strip_prefix("--> ") {
                    let parts: Vec<&str> = location.rsplitn(3, ':').collect();
                    if parts.len() == 3 {
                        err_col = parts[0].parse().unwrap_or(1);
                        err_line = parts[1].parse().unwrap_or(1);
                        file = sanitize_reported_path(parts[2]);
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
        index += 1;
    }

    if errors.is_empty() {
        errors.push(CompileError {
            file: "lib.rs".to_string(),
            line: 0,
            col: 0,
            message: "Compilation failed (see compiler logs)".to_string(),
        });
    }

    errors
}

/// Parse warnings from cargo/rustc stderr output.
pub(super) fn parse_cargo_warnings(stderr: &str) -> Vec<String> {
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
pub(super) fn parse_clang_errors(stderr: &str) -> Vec<CompileError> {
    let mut errors = Vec::new();

    for line in stderr.lines() {
        if line.contains("error:") {
            let parts: Vec<&str> = line.splitn(4, ':').collect();
            if parts.len() >= 4 {
                let file = sanitize_reported_path(parts[0]);
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
                let message = line
                    .split_once("error:")
                    .map(|(_, detail)| format!("error: {}", detail.trim()))
                    .unwrap_or_else(|| "Compilation failed (see compiler logs)".to_string());
                errors.push(CompileError {
                    file: "contract.c".to_string(),
                    line: 1,
                    col: 1,
                    message,
                });
            }
        }
    }

    if errors.is_empty() {
        errors.push(CompileError {
            file: "contract.c".to_string(),
            line: 1,
            col: 1,
            message: "Compilation failed (see compiler logs)".to_string(),
        });
    }

    errors
}

/// Parse AssemblyScript compiler errors.
/// asc format: "ERROR TS2322: ... in contract.ts(10,5)"
pub(super) fn parse_asc_errors(stderr: &str) -> Vec<CompileError> {
    let mut errors = Vec::new();

    for line in stderr.lines() {
        if line.starts_with("ERROR") || line.contains("error") {
            let mut file = "contract.ts".to_string();
            let mut err_line: usize = 1;
            let mut err_col: usize = 1;
            let mut message = line.trim().to_string();

            if let Some(index) = line.rfind(" in ") {
                let location = &line[index + 4..];
                message = line[..index].trim().to_string();
                if let Some(paren) = location.find('(') {
                    file = sanitize_reported_path(&location[..paren]);
                    let coords = &location[paren + 1..location.len().saturating_sub(1)];
                    let parts: Vec<&str> = coords.split(',').collect();
                    if parts.len() >= 2 {
                        err_line = parts[0].trim().parse().unwrap_or(1);
                        err_col = parts[1].trim().parse().unwrap_or(1);
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
    }

    if errors.is_empty() {
        errors.push(CompileError {
            file: "contract.ts".to_string(),
            line: 1,
            col: 1,
            message: "Compilation failed (see compiler logs)".to_string(),
        });
    }

    errors
}

fn sanitize_reported_path(raw: &str) -> String {
    let trimmed = raw.trim().trim_matches(|ch| matches!(ch, '"' | '\'' | '`'));
    if trimmed.is_empty() {
        return "source".to_string();
    }

    let components: Vec<String> = Path::new(trimmed)
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(part) => part.to_str().map(|value| value.to_string()),
            _ => None,
        })
        .collect();

    if components.is_empty() {
        return trimmed.to_string();
    }

    for anchor in ["src", "tests", "examples"] {
        if let Some(index) = components.iter().rposition(|part| part == anchor) {
            return components[index..].join("/");
        }
    }

    components
        .last()
        .cloned()
        .unwrap_or_else(|| trimmed.to_string())
}
