# MoltChain Auto-Update System — Detailed Design

## Overview

Production-grade automatic update system for MoltChain validators. Validators
automatically check for new releases on GitHub, download the binary, verify its
integrity (Ed25519 signature + SHA256 hash), perform a graceful binary swap, and
restart through the existing supervisor. Designed for both testnet and mainnet.

---

## Architecture

```
┌─────────────────────┐
│   GitHub Releases   │  ← Official release artifacts
│  (lobstercove/molt- │    • moltchain-validator-{os}-{arch}.tar.gz
│   chain)            │    • SHA256SUMS
│                     │    • SHA256SUMS.sig  (Ed25519)
└─────────┬───────────┘
          │ HTTPS (reqwest)
          ▼
┌─────────────────────┐
│   Updater Module    │  validator/src/updater.rs
│                     │
│  • Version check    │  GET /repos/.../releases/latest
│  • Download binary  │  Stream to temp file
│  • Verify SHA256    │  Hash downloaded archive
│  • Verify Ed25519   │  Signature over SHA256SUMS
│  • Extract binary   │  tar.gz → executable
│  • Swap + restart   │  Atomic rename + exit(75)
│  • Rollback guard   │  Revert on repeated crashes
└─────────┬───────────┘
          │ exit(75) — EXIT_CODE_RESTART
          ▼
┌─────────────────────┐
│   Supervisor Loop   │  Already exists in main.rs
│  (restarts child)   │  Picks up new binary on re-exec
└─────────────────────┘
```

---

## Release Artifact Structure

Each GitHub Release tag (e.g., `v0.2.0`) must contain:

| File | Description |
|------|-------------|
| `moltchain-validator-linux-x86_64.tar.gz` | Linux amd64 binary |
| `moltchain-validator-linux-aarch64.tar.gz` | Linux arm64 binary |
| `moltchain-validator-darwin-x86_64.tar.gz` | macOS Intel binary |
| `moltchain-validator-darwin-aarch64.tar.gz` | macOS Apple Silicon binary |
| `SHA256SUMS` | SHA256 hash file: `<hash>  <filename>` per line |
| `SHA256SUMS.sig` | Ed25519 signature of SHA256SUMS (64 bytes, hex-encoded) |

### SHA256SUMS format
```
a1b2c3...  moltchain-validator-linux-x86_64.tar.gz
d4e5f6...  moltchain-validator-linux-aarch64.tar.gz
...
```

### Archive structure
```
moltchain-validator-{os}-{arch}.tar.gz
  └── moltchain-validator          # single statically-linked binary
```

---

## Security Model

### Ed25519 Release Signing

1. **Offline keypair generation**: `scripts/generate-release-keys.sh` creates a
   keypair using the same Ed25519 scheme as the chain itself (`Keypair::generate()`).
2. **Public key embedded in binary**: The release signing public key is compiled
   into the validator binary as a constant (`RELEASE_SIGNING_PUBKEY`). This means
   only binaries from the official repo can verify releases.
3. **Signing workflow**: After CI builds artifacts and computes SHA256SUMS, a
   maintainer signs SHA256SUMS offline with the private key.
4. **Verification**: The updater downloads SHA256SUMS + SHA256SUMS.sig, verifies
   the signature against the embedded public key, then verifies the downloaded
   archive hash matches SHA256SUMS.

### Trust chain
```
Embedded pubkey → verifies SHA256SUMS.sig → authenticates SHA256SUMS → verifies archive hash
```

### Rollback Guard

If the validator crashes within 60 seconds of an update (3 consecutive times),
the updater restores the previous binary from `<binary>.rollback` and marks the
update as failed. This prevents bricked validators from bad releases.

---

## CLI Flags

| Flag | Default | Description |
|------|---------|-------------|
| `--auto-update` | `off` | Update mode: `off`, `check`, `download`, `apply` |
| `--update-check-interval` | `300` | Seconds between update checks |
| `--update-channel` | `stable` | Release channel: `stable`, `beta`, `edge` |
| `--no-auto-restart` | false | Download + verify but don't apply (manual restart required) |

### Update modes

- **`off`**: No update checking (default for safety)
- **`check`**: Check for updates and log availability, but don't download
- **`download`**: Download + verify, but don't apply (staged in `<binary>.pending`)
- **`apply`**: Full automatic: check → download → verify → swap → restart

---

## P2P Version Gossip

The `ValidatorAnnounce` message gains a `version: String` field containing the
running validator's semver version. This enables:

- Dashboard display of network version distribution
- Alerting operators running outdated versions
- Coordinated upgrade monitoring

The `version` field is informational — it does NOT gate consensus behavior.

---

## Update Flow (mode=apply)

```
1. Sleep(check_interval + random_jitter(0..60s))
2. GET https://api.github.com/repos/lobstercove/moltchain/releases/latest
3. Parse tag_name → semver, compare with CARGO_PKG_VERSION
4. If newer:
   a. Download SHA256SUMS + SHA256SUMS.sig
   b. Verify Ed25519(SHA256SUMS.sig, SHA256SUMS) against embedded pubkey
   c. Determine platform asset name (os + arch)
   d. Download archive, streaming to temp file
   e. Compute SHA256 of downloaded file, compare with SHA256SUMS entry
   f. Extract binary from tar.gz into <binary>.staging
   g. chmod +x the staged binary
   h. Rename current binary → <binary>.rollback
   i. Rename <binary>.staging → current binary
   j. Log "Update applied: v0.1.0 → v0.2.0"
   k. Exit with EXIT_CODE_RESTART (75) — supervisor picks up new binary
5. If not newer: sleep and loop
```

---

## Staggered Rollout

To prevent all validators from restarting simultaneously (which would halt the
network), the updater adds a random jitter of 0–60 seconds to each check
interval. Combined with natural clock drift, this spaces out restarts across
the validator set.

For mainnet with many validators, the jitter window should be configurable and
defaults to 300s (5 minutes).

---

## Module Design: `validator/src/updater.rs`

### Public API

```rust
pub enum UpdateMode { Off, Check, Download, Apply }

pub struct UpdateConfig {
    pub mode: UpdateMode,
    pub check_interval: Duration,
    pub channel: String,
    pub no_auto_restart: bool,
    pub jitter_secs: u64,
}

/// Spawns the background update checker task.
/// Returns a JoinHandle that runs until the validator exits.
pub async fn spawn_update_checker(config: UpdateConfig) -> JoinHandle<()>
```

### Dependencies added to `validator/Cargo.toml`

```toml
reqwest = { version = "0.12", features = ["stream"] }
semver = "1.0"
flate2 = "1.0"
tar = "0.4"
rand = "0.8"
```

Note: Ed25519 verification uses the existing `moltchain_core::Keypair`/`Pubkey`
types — no need for `ed25519-dalek` as a direct dependency.

---

## CI/CD Release Pipeline

### `.github/workflows/release.yml`

Triggered by pushing a tag matching `v*`:

```
1. Build matrix: linux-x86_64, linux-aarch64, darwin-x86_64, darwin-aarch64
2. cargo build --release --bin moltchain-validator
3. Strip binary, compress: tar czf moltchain-validator-{os}-{arch}.tar.gz
4. Compute SHA256 → SHA256SUMS
5. Upload artifacts to GitHub Release (draft)
6. Maintainer signs SHA256SUMS offline → uploads SHA256SUMS.sig
7. Publish release
```

### `scripts/generate-release-keys.sh`

One-time script to generate the Ed25519 keypair for release signing.
Outputs `release-signing-keypair.json` (SECRET — keep offline) and prints
the public key hex to embed in the source code.

### `scripts/sign-release.sh`

Signs a SHA256SUMS file with the release signing key:
```bash
./scripts/sign-release.sh <path-to-SHA256SUMS> <path-to-keypair.json>
# Outputs: SHA256SUMS.sig
```

---

## Testing Strategy

1. **Unit tests**: Version comparison, SHA256 verification, signature verification
2. **Integration test**: Mock GitHub API → full download+verify+stage cycle
3. **E2E test**: Build two versions, publish mock release, verify auto-update applies

---

## Mainnet Safety

- Default mode is `off` — operators must explicitly opt in
- Signature verification is mandatory — no unsigned updates
- Rollback guard prevents bricking
- Staggered jitter prevents network-wide simultaneous restarts
- Operators can use `--auto-update=download` to stage updates for manual review
- The `version` P2P gossip lets dashboards show upgrade progress across the network
