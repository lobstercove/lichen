// MoltChain Auto-Update System
//
// Production-grade automatic update module for validators.
// Checks GitHub Releases for new versions, downloads the binary,
// verifies integrity (Ed25519 signature + SHA256 hash), and performs
// a graceful binary swap with rollback guard.

use anyhow::{anyhow, bail, Context, Result};
use moltchain_core::{Keypair, Pubkey};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{self, Write};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
#[cfg(target_os = "windows")]
use std::process::Command;
use std::time::Duration;
use tokio::task::JoinHandle;
use tracing::{info, warn};

// ── Constants ───────────────────────────────────────────────────────────────

/// Current binary version (set by Cargo at compile time)
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// GitHub repository for release checks
const GITHUB_REPO: &str = "lobstercove/moltchain";

/// GitHub API base URL
const GITHUB_API: &str = "https://api.github.com";

/// Exit code that tells the supervisor to restart the process
/// (must match EXIT_CODE_RESTART in main.rs)
const EXIT_CODE_RESTART: i32 = 75;

/// Maximum download size (500 MB) to prevent resource exhaustion
const MAX_DOWNLOAD_BYTES: u64 = 500 * 1024 * 1024;

/// Number of consecutive fast-crash cycles before triggering rollback
const ROLLBACK_CRASH_THRESHOLD: u32 = 3;

/// If the validator crashes within this many seconds of an update,
/// it counts as a "fast crash" for rollback purposes
const ROLLBACK_CRASH_WINDOW_SECS: u64 = 60;

// ── Release Signing Public Key ──────────────────────────────────────────────
//
// This is the Ed25519 public key used to verify release signatures.
// Generated once with `scripts/generate-release-keys.sh` and embedded here.
// To rotate: generate new keypair, update this constant, release a signed
// build with the OLD key, then switch to signing with the NEW key.
//
// PLACEHOLDER — replace with actual key after running generate-release-keys.sh
// AUDIT-FIX V5.5: Replaced placeholder all-zeros key with real Ed25519
// public key. Private seed stored in keypairs/release-signing-key.json.
const RELEASE_SIGNING_PUBKEY_HEX: &str =
    "dd34731c7bc7e9317ed0f83991930c3859b05ecf2d74f10c4dc08de6b6bad332";

// ── Types ───────────────────────────────────────────────────────────────────

/// Auto-update operational mode
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateMode {
    /// No update checking (default)
    Off,
    /// Check for updates and log, but don't download
    Check,
    /// Download + verify, stage as .pending but don't apply
    Download,
    /// Full automatic: check → download → verify → swap → restart
    Apply,
}

impl UpdateMode {
    pub fn parse_mode(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "check" => Self::Check,
            "download" => Self::Download,
            "apply" => Self::Apply,
            _ => Self::Off,
        }
    }
}

impl std::fmt::Display for UpdateMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Off => write!(f, "off"),
            Self::Check => write!(f, "check"),
            Self::Download => write!(f, "download"),
            Self::Apply => write!(f, "apply"),
        }
    }
}

/// Configuration for the update checker
#[derive(Debug, Clone)]
pub struct UpdateConfig {
    pub mode: UpdateMode,
    /// Seconds between update checks
    pub check_interval_secs: u64,
    /// Release channel filter (e.g., "stable", "beta")
    pub channel: String,
    /// If true, download + verify but don't apply (requires manual restart)
    pub no_auto_restart: bool,
    /// Maximum random jitter added to check interval (seconds)
    pub jitter_max_secs: u64,
    /// Which binary to extract from the release archive.
    /// Defaults to "moltchain-validator".  Set to "moltchain-faucet",
    /// "molt-cli", or "moltchain-custody" for other services.
    pub target_binary: String,
    /// Optional list of companion binaries to also update from the same
    /// release archive.  Each entry is a (binary_name, install_path) pair.
    /// Example: `("moltchain-faucet", "/usr/local/bin/moltchain-faucet")`
    /// Only updated when mode == Apply and the primary binary updates
    /// successfully. Companion update failures are logged but don't block
    /// the primary update.
    pub companion_binaries: Vec<(String, PathBuf)>,
}

impl Default for UpdateConfig {
    fn default() -> Self {
        Self {
            mode: UpdateMode::Off,
            check_interval_secs: 300,
            channel: "stable".to_string(),
            no_auto_restart: false,
            jitter_max_secs: 60,
            target_binary: "moltchain-validator".to_string(),
            companion_binaries: Vec::new(),
        }
    }
}

/// GitHub Release API response (subset of fields)
#[derive(Debug, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    #[serde(default)]
    #[allow(dead_code)]
    prerelease: bool,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    assets: Vec<GitHubAsset>,
}

/// GitHub Release asset
#[derive(Debug, Deserialize)]
struct GitHubAsset {
    name: String,
    browser_download_url: String,
    size: u64,
}

/// Metadata file tracking update state (persisted to disk)
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct UpdateState {
    /// Version that was applied (or attempted)
    last_update_version: String,
    /// Timestamp of the update
    update_timestamp: u64,
    /// Number of consecutive fast crashes after this update
    crash_count: u32,
    /// Whether this update has been rolled back
    rolled_back: bool,
}

// ── Public API ──────────────────────────────────────────────────────────────

/// Spawn the background update checker task.
/// Returns a JoinHandle that runs until the validator shuts down.
pub fn spawn_update_checker(config: UpdateConfig) -> JoinHandle<()> {
    tokio::spawn(async move {
        if config.mode == UpdateMode::Off {
            info!("🔄 Auto-updater: disabled (mode=off)");
            return;
        }
        info!(
            "🔄 Auto-updater: enabled (mode={}, interval={}s, channel={})",
            config.mode, config.check_interval_secs, config.channel
        );

        // Check rollback guard on startup
        if let Err(e) = check_rollback_guard().await {
            warn!("⚠️  Rollback guard check failed: {}", e);
        }

        let base_interval = Duration::from_secs(config.check_interval_secs);

        loop {
            // Add random jitter to prevent thundering herd
            let jitter = jitter_duration(config.jitter_max_secs);
            tokio::time::sleep(base_interval + jitter).await;

            match check_and_update(&config).await {
                Ok(Some(new_version)) => {
                    info!("✅ Update applied: v{} → v{}", VERSION, new_version);
                    if config.mode == UpdateMode::Apply && !config.no_auto_restart {
                        info!("🔄 Requesting supervisor restart to pick up new binary...");
                        std::process::exit(EXIT_CODE_RESTART);
                    }
                }
                Ok(None) => {
                    // No update available or mode doesn't apply
                }
                Err(e) => {
                    warn!("⚠️  Update check failed: {}", e);
                }
            }
        }
    })
}

/// Check for a new version and optionally download/apply it.
/// Returns Some(version_string) if an update was applied, None otherwise.
async fn check_and_update(config: &UpdateConfig) -> Result<Option<String>> {
    // 1. Fetch latest release info from GitHub
    let release = fetch_latest_release(&config.channel).await?;
    let remote_version = parse_version(&release.tag_name)?;

    // 2. Compare with current version
    let current = parse_version(&format!("v{}", VERSION))?;
    if remote_version <= current {
        info!(
            "🔄 Up to date (current: v{}, latest: {})",
            VERSION, release.tag_name
        );
        return Ok(None);
    }

    info!(
        "🆕 New version available: {} (current: v{})",
        release.tag_name, VERSION
    );

    if config.mode == UpdateMode::Check {
        return Ok(None);
    }

    // 3. Download and verify the release
    let exe_path = std::env::current_exe().context("Cannot determine current executable path")?;
    let staging_path = exe_path.with_extension("staging");
    let pending_path = exe_path.with_extension("pending");

    // Download SHA256SUMS and SHA256SUMS.sig
    let sums_asset = find_asset(&release.assets, "SHA256SUMS")
        .ok_or_else(|| anyhow!("Release missing SHA256SUMS"))?;
    let sig_asset = find_asset(&release.assets, "SHA256SUMS.sig")
        .ok_or_else(|| anyhow!("Release missing SHA256SUMS.sig"))?;

    let sha256sums = download_text(&sums_asset.browser_download_url).await?;
    let sig_hex = download_text(&sig_asset.browser_download_url).await?;

    // 4. Verify Ed25519 signature on SHA256SUMS
    verify_signature(&sha256sums, sig_hex.trim())?;
    info!("✅ SHA256SUMS signature verified");

    // 5. Find platform-specific archive
    let platform_name = platform_asset_name();
    let archive_asset = find_asset(&release.assets, &platform_name)
        .ok_or_else(|| anyhow!("No release artifact for platform: {}", platform_name))?;

    // Verify archive isn't absurdly large
    if archive_asset.size > MAX_DOWNLOAD_BYTES {
        bail!(
            "Release artifact too large: {} bytes (max {})",
            archive_asset.size,
            MAX_DOWNLOAD_BYTES
        );
    }

    // 6. Look up expected SHA256 for this archive
    let expected_hash = find_hash_in_sums(&sha256sums, &platform_name)
        .ok_or_else(|| anyhow!("No SHA256 entry for {} in SHA256SUMS", platform_name))?;

    // 7. Download archive
    info!(
        "📦 Downloading {} ({} bytes)...",
        platform_name, archive_asset.size
    );
    let archive_data =
        download_binary(&archive_asset.browser_download_url, archive_asset.size).await?;

    // 8. Verify SHA256
    let actual_hash = sha256_hex(&archive_data);
    if actual_hash != expected_hash {
        bail!(
            "SHA256 mismatch for {}: expected {}, got {}",
            platform_name,
            expected_hash,
            actual_hash
        );
    }
    info!("✅ SHA256 verified for {}", platform_name);

    // 9. Extract binary from archive
    extract_binary_from_archive(&archive_data, &staging_path, &config.target_binary)?;
    info!(
        "📦 Extracted {} to {}",
        config.target_binary,
        staging_path.display()
    );

    // 10. Make executable
    #[cfg(unix)]
    {
        let mut perms = fs::metadata(&staging_path)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&staging_path, perms)?;
    }

    if config.mode == UpdateMode::Download {
        // Stage only — move to .pending for manual application
        fs::rename(&staging_path, &pending_path)
            .context("Failed to move staged binary to .pending")?;
        info!(
            "📦 Update staged at {} — restart manually to apply",
            pending_path.display()
        );
        return Ok(Some(remote_version.to_string()));
    }

    #[cfg(target_os = "windows")]
    {
        schedule_windows_update_swap(&exe_path, &staging_path, &pending_path)?;

        let update_state = UpdateState {
            last_update_version: remote_version.to_string(),
            update_timestamp: now_secs(),
            crash_count: 0,
            rolled_back: false,
        };
        save_update_state(&exe_path, &update_state)?;

        info!(
            "📦 Windows update staged at {} — restart requested to apply",
            pending_path.display()
        );

        return Ok(Some(remote_version.to_string()));
    }

    // 11. Apply: atomic binary swap
    let rollback_path = exe_path.with_extension("rollback");

    // Back up current binary
    fs::copy(&exe_path, &rollback_path).context("Failed to back up current binary to .rollback")?;

    // Swap staging → current
    fs::rename(&staging_path, &exe_path).context("Failed to swap in new binary")?;

    // Record update state for rollback guard
    let update_state = UpdateState {
        last_update_version: remote_version.to_string(),
        update_timestamp: now_secs(),
        crash_count: 0,
        rolled_back: false,
    };
    save_update_state(&exe_path, &update_state)?;

    info!(
        "✅ Binary swapped: v{} → v{}",
        VERSION,
        remote_version.to_string()
    );

    // 12. Update companion binaries (faucet, custody, cli) from the same archive
    for (companion_name, install_path) in &config.companion_binaries {
        let companion_staging = install_path.with_extension("staging");
        match extract_binary_from_archive(&archive_data, &companion_staging, companion_name) {
            Ok(()) => {
                #[cfg(unix)]
                {
                    if let Ok(meta) = fs::metadata(&companion_staging) {
                        let mut perms = meta.permissions();
                        perms.set_mode(0o755);
                        let _ = fs::set_permissions(&companion_staging, perms);
                    }
                }
                // Back up existing binary if present
                if install_path.exists() {
                    let companion_rollback = install_path.with_extension("rollback");
                    if let Err(e) = fs::copy(install_path, &companion_rollback) {
                        warn!(
                            "⚠️  Failed to back up {} — skipping update: {}",
                            companion_name, e
                        );
                        let _ = fs::remove_file(&companion_staging);
                        continue;
                    }
                }
                match fs::rename(&companion_staging, install_path) {
                    Ok(()) => info!(
                        "✅ Companion binary updated: {} at {}",
                        companion_name,
                        install_path.display()
                    ),
                    Err(e) => warn!(
                        "⚠️  Failed to swap companion binary {}: {}",
                        companion_name, e
                    ),
                }
            }
            Err(e) => {
                warn!(
                    "⚠️  Failed to extract companion binary {} from archive: {}",
                    companion_name, e
                );
            }
        }
    }

    Ok(Some(remote_version.to_string()))
}

#[cfg(target_os = "windows")]
fn schedule_windows_update_swap(
    exe_path: &Path,
    staging_path: &Path,
    pending_path: &Path,
) -> Result<()> {
    if pending_path.exists() {
        let _ = fs::remove_file(pending_path);
    }

    fs::rename(staging_path, pending_path)
        .context("Failed to move staged binary to .pending on Windows")?;

    let rollback_path = exe_path.with_extension("rollback");
    let script_path = exe_path.with_extension("apply-update.cmd");
    let pid = std::process::id();

    let script = format!(
        "@echo off\r\nsetlocal\r\n:waitloop\r\ntasklist /FI \"PID eq {pid}\" 2>NUL | find /I \"{pid}\" >NUL\r\nif not errorlevel 1 (\r\n  timeout /t 1 /nobreak >NUL\r\n  goto waitloop\r\n)\r\nif exist \"{rollback}\" del /f /q \"{rollback}\" >NUL 2>&1\r\nif exist \"{exe}\" move /Y \"{exe}\" \"{rollback}\" >NUL 2>&1\r\nmove /Y \"{pending}\" \"{exe}\" >NUL 2>&1\r\ndel /f /q \"%~f0\" >NUL 2>&1\r\n",
        pid = pid,
        exe = exe_path.display(),
        pending = pending_path.display(),
        rollback = rollback_path.display(),
    );

    fs::write(&script_path, script).context("Failed to write Windows update helper script")?;

    Command::new("cmd")
        .args(["/C", "start", "", "/B", &script_path.to_string_lossy()])
        .spawn()
        .context("Failed to spawn Windows update helper")?;

    Ok(())
}

// ── Rollback Guard ──────────────────────────────────────────────────────────

/// On startup, check if we just crashed quickly after an update.
/// If crash_count >= threshold, rollback to the previous binary.
async fn check_rollback_guard() -> Result<()> {
    let exe_path = std::env::current_exe().context("Cannot determine executable path")?;
    let rollback_path = exe_path.with_extension("rollback");
    let state_path = update_state_path(&exe_path);

    if !state_path.exists() || !rollback_path.exists() {
        return Ok(());
    }

    let mut update_state = load_update_state(&exe_path)?;

    if update_state.rolled_back {
        return Ok(()); // Already rolled back, nothing to do
    }

    let elapsed = now_secs().saturating_sub(update_state.update_timestamp);

    if elapsed < ROLLBACK_CRASH_WINDOW_SECS {
        update_state.crash_count += 1;
        info!(
            "⚠️  Fast restart detected after update to v{} (crash {}/{})",
            update_state.last_update_version, update_state.crash_count, ROLLBACK_CRASH_THRESHOLD
        );

        if update_state.crash_count >= ROLLBACK_CRASH_THRESHOLD {
            // Rollback!
            warn!(
                "🔙 Rolling back from v{} — too many fast crashes",
                update_state.last_update_version
            );
            fs::copy(&rollback_path, &exe_path).context("Failed to restore rollback binary")?;
            update_state.rolled_back = true;
            save_update_state(&exe_path, &update_state)?;

            // Restart with the rolled-back binary
            std::process::exit(EXIT_CODE_RESTART);
        }

        save_update_state(&exe_path, &update_state)?;
    } else {
        // Ran long enough — the update is stable
        // Clean up rollback binary and state
        if rollback_path.exists() {
            let _ = fs::remove_file(&rollback_path);
        }
        if state_path.exists() {
            let _ = fs::remove_file(&state_path);
        }
        info!(
            "✅ Update to v{} confirmed stable — cleaned up rollback files",
            update_state.last_update_version
        );
    }

    Ok(())
}

// ── GitHub API ──────────────────────────────────────────────────────────────

/// Fetch the latest release from GitHub.
/// Filters by channel: "stable" skips prereleases, "beta"/"edge" includes them.
async fn fetch_latest_release(channel: &str) -> Result<GitHubRelease> {
    let client = reqwest::Client::builder()
        .user_agent(format!("moltchain-validator/{}", VERSION))
        .timeout(Duration::from_secs(30))
        .build()?;

    let url = if channel == "stable" {
        // /releases/latest only returns non-prerelease, non-draft
        format!("{}/repos/{}/releases/latest", GITHUB_API, GITHUB_REPO)
    } else {
        // For beta/edge, fetch all and pick the first matching
        format!("{}/repos/{}/releases?per_page=10", GITHUB_API, GITHUB_REPO)
    };

    let resp = client
        .get(&url)
        .send()
        .await
        .context("Failed to reach GitHub API")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        bail!("GitHub API returned {}: {}", status, body);
    }

    if channel == "stable" {
        let release: GitHubRelease = resp.json().await?;
        if release.draft {
            bail!("Latest release is a draft");
        }
        Ok(release)
    } else {
        let releases: Vec<GitHubRelease> = resp.json().await?;
        // For beta: accept prereleases. For edge: accept anything that's not draft.
        let release = releases
            .into_iter()
            .find(|r| !r.draft)
            .ok_or_else(|| anyhow!("No suitable {} release found", channel))?;
        Ok(release)
    }
}

/// Download a text file (SHA256SUMS, SHA256SUMS.sig)
async fn download_text(url: &str) -> Result<String> {
    let client = reqwest::Client::builder()
        .user_agent(format!("moltchain-validator/{}", VERSION))
        .timeout(Duration::from_secs(30))
        .build()?;

    let resp = client.get(url).send().await?;
    if !resp.status().is_success() {
        bail!("Download failed ({}): {}", resp.status(), url);
    }
    Ok(resp.text().await?)
}

/// Download a binary file with size limit
async fn download_binary(url: &str, expected_size: u64) -> Result<Vec<u8>> {
    let client = reqwest::Client::builder()
        .user_agent(format!("moltchain-validator/{}", VERSION))
        .timeout(Duration::from_secs(600))
        .build()?;

    let resp = client.get(url).send().await?;
    if !resp.status().is_success() {
        bail!("Download failed ({}): {}", resp.status(), url);
    }

    let bytes = resp.bytes().await?;
    if bytes.len() as u64 > MAX_DOWNLOAD_BYTES {
        bail!("Downloaded file exceeds size limit");
    }

    info!(
        "📦 Downloaded {} bytes (expected {})",
        bytes.len(),
        expected_size
    );
    Ok(bytes.to_vec())
}

// ── Cryptographic Verification ──────────────────────────────────────────────

/// Verify Ed25519 signature over SHA256SUMS content
fn verify_signature(sha256sums_content: &str, sig_hex: &str) -> Result<()> {
    // Decode the release signing public key
    let pubkey_bytes = hex::decode(RELEASE_SIGNING_PUBKEY_HEX)
        .context("Invalid release signing public key hex")?;
    if pubkey_bytes.len() != 32 {
        bail!("Release signing public key must be 32 bytes");
    }

    let mut pubkey_arr = [0u8; 32];
    pubkey_arr.copy_from_slice(&pubkey_bytes);
    let pubkey = Pubkey(pubkey_arr);

    // Decode the signature
    let sig_bytes = hex::decode(sig_hex).context("Invalid signature hex encoding")?;
    if sig_bytes.len() != 64 {
        bail!("Signature must be 64 bytes, got {} bytes", sig_bytes.len());
    }
    let mut sig_arr = [0u8; 64];
    sig_arr.copy_from_slice(&sig_bytes);

    // Verify
    if !Keypair::verify(&pubkey, sha256sums_content.as_bytes(), &sig_arr) {
        bail!("Ed25519 signature verification FAILED — release may be tampered");
    }

    Ok(())
}

/// Compute SHA256 hex digest
fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

// ── Archive Extraction ──────────────────────────────────────────────────────

/// Extract a named binary from a .tar.gz archive
fn extract_binary_from_archive(
    archive_data: &[u8],
    output_path: &Path,
    target_binary: &str,
) -> Result<()> {
    let decoder = flate2::read::GzDecoder::new(archive_data);
    let mut archive = tar::Archive::new(decoder);

    let binary_name = if cfg!(target_os = "windows") {
        format!("{}.exe", target_binary)
    } else {
        target_binary.to_string()
    };

    for entry_result in archive.entries()? {
        let mut entry = entry_result?;
        let path = entry.path()?.to_path_buf();

        // P10-VAL-08: Validate tar entry paths to prevent directory traversal attacks.
        // Reject entries containing ".." components or absolute paths.
        let path_str = path.to_string_lossy();
        if path_str.contains("..") || path.is_absolute() {
            warn!("⚠️  Skipping tar entry with suspicious path: {}", path_str);
            continue;
        }

        // Look for the binary — either at root or in a subdirectory
        let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

        if file_name == binary_name {
            // Extract to output path
            let mut output_file =
                fs::File::create(output_path).context("Failed to create staging file")?;

            io::copy(&mut entry, &mut output_file)
                .context("Failed to extract binary from archive")?;

            output_file.flush()?;
            info!("📦 Extracted {} from archive", binary_name);
            return Ok(());
        }
    }

    bail!("Binary '{}' not found in archive", binary_name)
}

// ── Platform Detection ──────────────────────────────────────────────────────

/// Determine the correct release asset name for this platform
fn platform_asset_name() -> String {
    let os = if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "macos") {
        "darwin"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        "unknown"
    };

    let arch = if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        "unknown"
    };

    format!("moltchain-validator-{}-{}.tar.gz", os, arch)
}

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Find a specific asset by name in the release
fn find_asset<'a>(assets: &'a [GitHubAsset], name: &str) -> Option<&'a GitHubAsset> {
    assets.iter().find(|a| a.name == name)
}

/// Parse SHA256SUMS file and find hash for a specific filename
fn find_hash_in_sums(sums_content: &str, filename: &str) -> Option<String> {
    for line in sums_content.lines() {
        // Format: "<hash>  <filename>" (two spaces)
        let parts: Vec<&str> = line.splitn(2, |c: char| c.is_whitespace()).collect();
        if parts.len() == 2 {
            let hash = parts[0].trim();
            let file = parts[1].trim();
            if file == filename {
                return Some(hash.to_string());
            }
        }
    }
    None
}

/// Parse version from tag name (strips leading 'v')
fn parse_version(tag: &str) -> Result<semver::Version> {
    let clean = tag.strip_prefix('v').unwrap_or(tag);
    semver::Version::parse(clean).with_context(|| format!("Invalid semver: {}", tag))
}

/// Generate a random jitter duration
fn jitter_duration(max_secs: u64) -> Duration {
    if max_secs == 0 {
        return Duration::ZERO;
    }
    // Simple pseudo-random using system time nanoseconds
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    let jitter_secs = (nanos as u64) % max_secs;
    Duration::from_secs(jitter_secs)
}

/// Current time as seconds since epoch
fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Path to the update state file (alongside the binary)
fn update_state_path(exe_path: &Path) -> PathBuf {
    exe_path.with_extension("update-state.json")
}

/// Save update state to disk
fn save_update_state(exe_path: &Path, state: &UpdateState) -> Result<()> {
    let path = update_state_path(exe_path);
    let json = serde_json::to_string_pretty(state)?;
    fs::write(&path, json).context("Failed to write update state")?;
    Ok(())
}

/// Load update state from disk
fn load_update_state(exe_path: &Path) -> Result<UpdateState> {
    let path = update_state_path(exe_path);
    let json = fs::read_to_string(&path).context("Failed to read update state")?;
    let state: UpdateState = serde_json::from_str(&json)?;
    Ok(state)
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_version() {
        assert_eq!(
            parse_version("v0.2.0").unwrap(),
            semver::Version::new(0, 2, 0)
        );
        assert_eq!(
            parse_version("0.1.0").unwrap(),
            semver::Version::new(0, 1, 0)
        );
        assert_eq!(
            parse_version("v1.0.0-beta.1").unwrap(),
            semver::Version::parse("1.0.0-beta.1").unwrap()
        );
    }

    #[test]
    fn test_version_comparison() {
        let v1 = parse_version("v0.1.0").unwrap();
        let v2 = parse_version("v0.2.0").unwrap();
        assert!(v2 > v1);

        let v3 = parse_version("v0.2.0").unwrap();
        assert!(v2 <= v3);
    }

    #[test]
    fn test_sha256_hex() {
        let hash = sha256_hex(b"hello world");
        assert_eq!(
            hash,
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }

    #[test]
    fn test_find_hash_in_sums() {
        let sums = "abc123  moltchain-validator-linux-x86_64.tar.gz\n\
                     def456  moltchain-validator-darwin-aarch64.tar.gz\n\
                     ghi789  moltchain-validator-windows-x86_64.tar.gz\n";
        assert_eq!(
            find_hash_in_sums(sums, "moltchain-validator-linux-x86_64.tar.gz"),
            Some("abc123".to_string())
        );
        assert_eq!(
            find_hash_in_sums(sums, "moltchain-validator-darwin-aarch64.tar.gz"),
            Some("def456".to_string())
        );
        assert_eq!(
            find_hash_in_sums(sums, "moltchain-validator-windows-x86_64.tar.gz"),
            Some("ghi789".to_string())
        );
        assert_eq!(find_hash_in_sums(sums, "nonexistent.tar.gz"), None);
    }

    #[test]
    fn test_platform_asset_name() {
        let name = platform_asset_name();
        assert!(name.starts_with("moltchain-validator-"));
        assert!(name.ends_with(".tar.gz"));
    }

    #[test]
    fn test_update_mode_from_str() {
        assert_eq!(UpdateMode::parse_mode("off"), UpdateMode::Off);
        assert_eq!(UpdateMode::parse_mode("check"), UpdateMode::Check);
        assert_eq!(UpdateMode::parse_mode("download"), UpdateMode::Download);
        assert_eq!(UpdateMode::parse_mode("apply"), UpdateMode::Apply);
        assert_eq!(UpdateMode::parse_mode("anything_else"), UpdateMode::Off);
    }

    #[test]
    fn test_jitter_duration() {
        let d = jitter_duration(60);
        assert!(d.as_secs() < 60);

        let d0 = jitter_duration(0);
        assert_eq!(d0, Duration::ZERO);
    }

    #[test]
    fn test_verify_signature_rejects_bad_sig() {
        let result = verify_signature("hello", "aa".repeat(64).as_str());
        assert!(result.is_err());
    }

    #[test]
    fn test_update_state_roundtrip() {
        let state = UpdateState {
            last_update_version: "0.2.0".to_string(),
            update_timestamp: 1700000000,
            crash_count: 1,
            rolled_back: false,
        };
        let json = serde_json::to_string(&state).unwrap();
        let loaded: UpdateState = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.last_update_version, "0.2.0");
        assert_eq!(loaded.crash_count, 1);
    }

    #[test]
    fn test_ed25519_sign_verify_roundtrip() {
        use moltchain_core::Keypair;

        // Generate a keypair, sign a message, then verify
        let kp = Keypair::new();
        let message = b"SHA256SUMS content here";
        let sig = kp.sign(message);

        assert!(Keypair::verify(&kp.pubkey(), message, &sig));

        // Tampered message fails
        assert!(!Keypair::verify(&kp.pubkey(), b"tampered", &sig));
    }

    /// AUDIT-FIX V5.5: Ensure the release signing public key is not the
    /// placeholder all-zeros value, which would make update verification
    /// non-functional.
    #[test]
    fn test_release_signing_pubkey_not_placeholder() {
        let all_zeros = "0".repeat(64);
        assert_ne!(
            RELEASE_SIGNING_PUBKEY_HEX, all_zeros,
            "Release signing public key must not be all-zeros placeholder"
        );
        // Must be valid 32-byte hex
        let bytes = hex::decode(RELEASE_SIGNING_PUBKEY_HEX).expect("Invalid hex in release pubkey");
        assert_eq!(
            bytes.len(),
            32,
            "Release signing public key must be 32 bytes"
        );
        // Must not be all zeros even after decode
        assert!(
            bytes.iter().any(|&b| b != 0),
            "Decoded release signing pubkey must not be all zeros"
        );
    }
}
