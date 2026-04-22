use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use tempfile::TempDir;

use super::{CompileBackend, CompileError, SANDBOX_WORKSPACE};

fn compile_process_error(file: &str, message: impl Into<String>) -> Vec<CompileError> {
    vec![CompileError {
        file: file.to_string(),
        line: 0,
        col: 0,
        message: message.into(),
    }]
}

fn compiler_temp_root(backend: &CompileBackend) -> Result<Option<PathBuf>, Vec<CompileError>> {
    let configured = std::env::var("COMPILER_TEMP_ROOT")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);

    let root = match (backend, configured) {
        (_, Some(path)) => Some(path),
        (CompileBackend::Docker(_), None) => {
            let cwd = std::env::current_dir().map_err(|error| {
                compile_process_error(
                    "system",
                    format!("Failed to resolve compiler working directory: {}", error),
                )
            })?;
            Some(cwd.join(".compiler-work"))
        }
        (CompileBackend::Host, None) => None,
    };

    if let Some(path) = root.as_ref() {
        fs::create_dir_all(path).map_err(|error| {
            compile_process_error(
                "system",
                format!(
                    "Failed to create compiler temp root '{}': {}",
                    path.display(),
                    error
                ),
            )
        })?;
    }

    Ok(root)
}

pub(super) fn create_compiler_temp_dir(
    backend: &CompileBackend,
) -> Result<TempDir, Vec<CompileError>> {
    if let Some(root) = compiler_temp_root(backend)? {
        TempDir::new_in(root).map_err(|error| {
            compile_process_error(
                "system",
                format!("Failed to create sandbox temp dir: {}", error),
            )
        })
    } else {
        TempDir::new().map_err(|error| {
            compile_process_error("system", format!("Failed to create temp dir: {}", error))
        })
    }
}

fn prepare_host_command(command: &mut Command, temp_root: &Path) -> Result<(), Vec<CompileError>> {
    let temp_home = temp_root.join("home");
    fs::create_dir_all(&temp_home).map_err(|error| {
        compile_process_error(
            "system",
            format!("Failed to create isolated compiler home: {}", error),
        )
    })?;

    let original_home = std::env::var_os("HOME").map(PathBuf::from);
    let cargo_home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| original_home.as_ref().map(|path| path.join(".cargo")));
    let rustup_home = std::env::var_os("RUSTUP_HOME")
        .map(PathBuf::from)
        .or_else(|| original_home.as_ref().map(|path| path.join(".rustup")));

    command.env_clear();
    if let Some(path) = std::env::var_os("PATH") {
        command.env("PATH", path);
    }
    command.env("HOME", &temp_home);
    command.env("TMPDIR", temp_root);

    if let Some(path) = cargo_home {
        command.env("CARGO_HOME", path);
    }
    if let Some(path) = rustup_home {
        command.env("RUSTUP_HOME", path);
    }

    Ok(())
}

pub(super) fn sandbox_path_for_host_path(
    host_root: &Path,
    host_path: &Path,
) -> Result<String, Vec<CompileError>> {
    let relative = host_path.strip_prefix(host_root).map_err(|_| {
        compile_process_error(
            "system",
            format!(
                "Sandbox path '{}' escaped compiler workspace",
                host_path.display()
            ),
        )
    })?;

    let relative_str = path_to_str(relative)?;
    if relative_str.is_empty() {
        return Ok(SANDBOX_WORKSPACE.to_string());
    }

    Ok(format!(
        "{}/{}",
        SANDBOX_WORKSPACE,
        relative_str.replace('\\', "/")
    ))
}

pub(super) fn spawn_compiler_process(
    backend: &CompileBackend,
    program: &str,
    args: &[&str],
    temp_root: &Path,
    working_dir: &Path,
) -> Result<std::process::Child, Vec<CompileError>> {
    match backend {
        CompileBackend::Host => {
            let mut command = Command::new(program);
            command.args(args).current_dir(working_dir);
            prepare_host_command(&mut command, temp_root)?;
            command
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|error| {
                    compile_process_error(program, format!("Failed to run {}: {}", program, error))
                })
        }
        CompileBackend::Docker(sandbox) => {
            let host_root = path_to_str(temp_root)?;
            let guest_working_dir = sandbox_path_for_host_path(temp_root, working_dir)?;
            let mount = format!(
                "type=bind,source={},target={}",
                host_root, SANDBOX_WORKSPACE
            );

            let mut command = Command::new(&sandbox.runtime);
            command
                .args([
                    "run",
                    "--rm",
                    "--network",
                    "none",
                    "--cap-drop",
                    "ALL",
                    "--security-opt",
                    "no-new-privileges:true",
                    "--pids-limit",
                    "256",
                    "--memory",
                    "1g",
                    "--cpus",
                    "2",
                    "--read-only",
                    "--tmpfs",
                    "/tmp:rw,exec,nosuid,size=256m",
                    "--mount",
                    &mount,
                    "--workdir",
                    &guest_working_dir,
                    "-e",
                    "HOME=/tmp/lichen",
                    "-e",
                    "TMPDIR=/tmp",
                    "-e",
                    "CARGO_HOME=/tmp/cargo-home",
                    "-e",
                    "CARGO_TARGET_DIR=/workspace/target",
                    "-e",
                    "RUSTUP_HOME=/usr/local/rustup",
                    &sandbox.image,
                    program,
                ])
                .args(args)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|error| {
                    compile_process_error(
                        program,
                        format!(
                            "Failed to run {} via {} sandbox: {}",
                            program, sandbox.runtime, error
                        ),
                    )
                })
        }
    }
}

/// Convert a path to a UTF-8 string, returning a compile error if the path contains non-UTF8.
pub(super) fn path_to_str(path: &Path) -> Result<String, Vec<CompileError>> {
    path.to_str().map(|value| value.to_string()).ok_or_else(|| {
        vec![CompileError {
            file: "system".to_string(),
            line: 0,
            col: 0,
            message: "Non-UTF8 temporary path".to_string(),
        }]
    })
}

/// Wait for a child process with a timeout. Kills the process if it exceeds the deadline.
/// I-6: Uses tokio::time::sleep instead of std::thread::sleep to avoid blocking the async runtime.
pub(super) async fn wait_with_timeout(
    child: &mut std::process::Child,
    timeout: Duration,
) -> Result<std::process::Output, String> {
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let stdout = child
                    .stdout
                    .take()
                    .map(|mut stream| {
                        let mut buffer = Vec::new();
                        std::io::Read::read_to_end(&mut stream, &mut buffer).ok();
                        buffer
                    })
                    .unwrap_or_default();
                let stderr = child
                    .stderr
                    .take()
                    .map(|mut stream| {
                        let mut buffer = Vec::new();
                        std::io::Read::read_to_end(&mut stream, &mut buffer).ok();
                        buffer
                    })
                    .unwrap_or_default();
                return Ok(std::process::Output {
                    status,
                    stdout,
                    stderr,
                });
            }
            Ok(None) => {
                if start.elapsed() > timeout {
                    if let Err(error) = child.kill() {
                        tracing::warn!("failed to kill timed-out compiler process: {}", error);
                    }
                    if let Err(error) = child.wait() {
                        tracing::warn!("failed to reap timed-out compiler process: {}", error);
                    }
                    return Err(format!(
                        "Compilation timed out after {} seconds",
                        timeout.as_secs()
                    ));
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err(error) => {
                return Err(format!("Failed to wait for compiler process: {}", error));
            }
        }
    }
}
