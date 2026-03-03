# Custody Multi-Signer Setup Guide

**Applies to:** MoltChain Custody Service (`custody/src/main.rs`)  
**Date:** February 14, 2026

This guide covers the complete setup process for multi-signer custody, including FROST Ed25519 threshold signing for Solana and Gnosis Safe multisig for EVM chains.

---

## Overview

The custody service supports two signing modes:

| Mode | When | How |
|------|------|-----|
| **Single-signer** | 1 signer endpoint configured | One machine holds the full private key and signs directly |
| **Multi-signer** | 2+ signer endpoints configured | Threshold signing — no single machine ever holds the full key |

Multi-signer mode uses **different protocols per chain**:

- **Solana (Ed25519):** FROST threshold signatures — a single valid Ed25519 signature is produced by combining shares from multiple signers. On-chain, it looks identical to a normal signature.
- **EVM (secp256k1):** Gnosis Safe multisig — each signer produces an independent ECDSA signature, and the custody service packs them into a Safe `execTransaction` call.

---

## Part 1: FROST Ed25519 Setup (Solana)

### What is FROST?

FROST (Flexible Round-Optimized Schnorr Threshold) is a threshold signing protocol for Ed25519. With a `t-of-n` setup (e.g., 2-of-3), any `t` signers can cooperate to produce a valid Ed25519 signature, but fewer than `t` cannot sign anything — and no single signer ever sees or holds the full private key.

MoltChain uses `frost-ed25519` v2.2 from the ZCash Foundation.

### Step 1: Plan Your Signer Topology

Decide on your threshold parameters:

| Network | Recommended | Description |
|---------|-------------|-------------|
| Testnet | 2-of-3 | 3 signer machines, any 2 can sign |
| Mainnet | 3-of-5 | 5 signer machines, any 3 can sign |

Each signer runs an independent HTTP service that implements two endpoints:
- `POST /frost/commit` — Round 1: generate nonce, return commitment
- `POST /frost/sign` — Round 2: receive signing package, return signature share

**Requirements per signer machine:**
- Separate physical or virtual machine (different failure domains)
- Secure key storage (encrypted disk or HSM)
- Network access to/from the custody coordinator
- Unique auth token for API authentication

### Step 2: Run the Distributed Key Generation (DKG) Ceremony

The DKG ceremony is a one-time interactive process where all `n` signers cooperate to generate:
- A **group public key** (the Solana address for the treasury)
- A **secret share** for each signer (never leaves that signer's machine)
- A **PublicKeyPackage** (shared, non-secret — needed by the coordinator)

**Important:** The DKG MUST be done interactively with all signer operators present. No single party generates the full key.

#### Using the frost-ed25519 crate directly:

Each signer machine runs the DKG participant code. Here's the protocol:

```bash
# Each signer needs the frost-ed25519 crate
cargo add frost-ed25519@2
```

```rust
use frost_ed25519 as frost;
use rand::thread_rng;

// Parameters (same on all signers)
let max_signers = 3;   // n = total signers
let min_signers = 2;    // t = threshold

// ── DKG ROUND 1 (each signer runs independently) ──
let mut rng = thread_rng();
let signer_id = frost::Identifier::try_from(1u16).unwrap(); // unique per signer: 1, 2, 3...

let (round1_secret, round1_package) =
    frost::keys::dkg::part1(signer_id, max_signers, min_signers, &mut rng)
        .expect("DKG round 1 failed");

// Each signer BROADCASTS their round1_package to all other signers
// (round1_secret stays on this machine)

// ── DKG ROUND 2 (after receiving all round 1 packages) ──
let mut received_round1: BTreeMap<frost::Identifier, frost::keys::dkg::round1::Package> =
    BTreeMap::new();
// ... insert packages received from other signers ...

let (round2_secret, round2_packages) =
    frost::keys::dkg::part2(round1_secret, &received_round1)
        .expect("DKG round 2 failed");

// Each signer sends their round2_packages[recipient_id] to each respective signer
// (these are TARGETED — each signer gets a different package)

// ── DKG ROUND 3 (after receiving all round 2 packages) ──
let mut received_round2: BTreeMap<frost::Identifier, frost::keys::dkg::round2::Package> =
    BTreeMap::new();
// ... insert packages received from other signers ...

let (key_package, public_key_package) =
    frost::keys::dkg::part3(&round2_secret, &received_round1, &received_round2)
        .expect("DKG round 3 failed");

// key_package      → SECRET: this signer's secret share. Store securely!
// public_key_package → PUBLIC: same on all signers. This is what the coordinator needs.
```

#### What each party stores:

| Data | Who stores it | Security |
|------|--------------|----------|
| `KeyPackage` (secret share) | Each signer, locally | CRITICAL — encrypt at rest, never transmit |
| `PublicKeyPackage` | All signers + coordinator | Public — safe to share |
| Group verifying key | Everyone | Public — this is the Solana treasury address |

### Step 3: Export the PublicKeyPackage

After DKG completes, serialize the `PublicKeyPackage` to hex:

```rust
use frost_ed25519 as frost;
use frost::serialize::Serialize;

let pkg_bytes = public_key_package
    .serialize()
    .expect("serialize public key package");
let pkg_hex = hex::encode(&pkg_bytes);

println!("CUSTODY_FROST_PUBKEY_PACKAGE={}", pkg_hex);
```

**Verify the group public key:**

```rust
let group_verifying_key = public_key_package.verifying_key();
println!("Group public key (Solana address): {}", bs58::encode(group_verifying_key.serialize()).into_string());
```

This public key is the Solana address that will hold custody funds.

### Step 4: Deploy Signer Services

Each signer machine runs an HTTP service that:
1. Loads its `KeyPackage` from encrypted storage at startup
2. Exposes two endpoints behind auth

**Signer API contract:**

```
POST /frost/commit
Request:  { "job_id": "...", "message_hex": "..." }
Response: { "status": "committed", "signer_id_hex": "...", "commitment_hex": "..." }

POST /frost/sign
Request:  { "job_id": "...", "message_hex": "...", "commitments": [{"signer_id_hex": "...", "commitment_hex": "..."}] }
Response: { "status": "signed", "signer_id_hex": "...", "share_hex": "..." }
```

- `signer_id_hex`: Hex-encoded `frost::Identifier` for this signer
- `commitment_hex`: Hex-encoded `frost::round1::SigningCommitments`
- `share_hex`: Hex-encoded `frost::round1::SignatureShare`

The signer service must:
- Generate fresh nonces for each `/frost/commit` request (never reuse!)
- Store the nonce/commitment pair keyed by `job_id` until `/frost/sign` is called
- Validate that the signing message matches what was committed to
- Require bearer token authentication on both endpoints

### Step 5: Configure the Custody Coordinator

Set these environment variables on the custody coordinator machine:

```bash
# Signer endpoints (comma-separated)
export CUSTODY_SIGNER_ENDPOINTS="https://signer1.internal:8443,https://signer2.internal:8443,https://signer3.internal:8443"

# Threshold (optional — auto-calculated as 2-of-3 for testnet, 3-of-5 for mainnet)
export CUSTODY_SIGNER_THRESHOLD=2

# Per-signer auth tokens (same order as endpoints)
export CUSTODY_SIGNER_AUTH_TOKENS="token-for-signer1,token-for-signer2,token-for-signer3"

# FROST public key package from DKG (hex-encoded)
export CUSTODY_FROST_PUBKEY_PACKAGE="0a1b2c3d..."

# API auth token for the custody service itself
export CUSTODY_API_AUTH_TOKEN="your-secret-api-token"
```

**Startup verification:** The custody service logs on startup:

```
Multi-signer mode: 2-of-3 threshold (FROST Ed25519 for Solana, packed ECDSA for EVM)
  FROST public key package loaded for Solana threshold signing
```

If `CUSTODY_FROST_PUBKEY_PACKAGE` is not set, you'll see a warning:

```
  WARNING: No FROST public key package configured. Multi-signer Solana withdrawals will fail until FROST DKG is completed.
```

### Signing Flow (Automatic)

Once configured, the custody service handles signing automatically:

```
1. Withdrawal requested → custody builds unsigned Solana transaction
2. Round 1: POST /frost/commit to each signer → collect nonce commitments
3. Check: got ≥ threshold commitments? If not → fail
4. Round 2: POST /frost/sign to each signer (with all commitments) → collect signature shares
5. Check: got ≥ threshold shares? If not → fail
6. Coordinator: frost::aggregate(signing_package, shares, pubkey_package) → group signature
7. Assemble signed Solana transaction → broadcast to Solana RPC
```

The resulting Ed25519 signature is indistinguishable from a single-signer signature.

---

## Part 2: Gnosis Safe Setup (EVM)

### What is Gnosis Safe?

Gnosis Safe (now Safe{Wallet}) is a battle-tested EVM multisig contract. Transactions require `t-of-n` ECDSA signatures from authorized owners. The custody service packs these signatures into the Safe's `execTransaction` function call.

### Step 1: Deploy a Gnosis Safe

**Option A: Safe{Wallet} Web UI** (recommended)

1. Go to [app.safe.global](https://app.safe.global)
2. Connect with each signer's wallet
3. Create new Safe:
   - **Network:** Ethereum mainnet (or target L2)
   - **Owners:** Add all signer Ethereum addresses
   - **Threshold:** Set to desired `t` (e.g., 2-of-3)
4. Deploy the Safe contract
5. Copy the Safe address (e.g., `0x1234...abcd`)
6. Fund the Safe with ETH for gas + the assets to custody (USDT, USDC, etc.)

**Option B: Safe CLI / SDK**

```bash
# Using Safe{Core} SDK
npm install @safe-global/protocol-kit @safe-global/api-kit

# Deploy via script
const { SafeFactory } = require('@safe-global/protocol-kit');
const factory = await SafeFactory.create({ ethAdapter });
const safe = await factory.deploySafe({
  safeAccountConfig: {
    owners: [
      '0xSigner1Address',
      '0xSigner2Address', 
      '0xSigner3Address'
    ],
    threshold: 2,
  }
});
console.log('Safe address:', await safe.getAddress());
```

### Step 2: Deploy Signer Services

Each EVM signer runs an HTTP service that:
1. Loads its Ethereum private key from encrypted storage
2. Exposes a single signing endpoint behind auth

**Signer API contract:**

```
POST /sign
Request:  { "job_id": "...", "chain": "ethereum", "message_hex": "...", "tx_hex": "..." }
Response: { "status": "signed", "signature": "<65-byte hex>", "signer_address": "0x..." }
```

- `signature`: 65-byte ECDSA signature (r[32] + s[32] + v[1])
- `signer_address`: Ethereum address of this signer (for verification)

The signer service must:
- Sign the provided message hash using its private key
- Return the raw ECDSA signature (not EIP-712 — the custody coordinator handles encoding)
- Require bearer token authentication

### Step 3: Configure the Custody Coordinator

```bash
# Signer endpoints (comma-separated, same services as FROST if running both)
export CUSTODY_SIGNER_ENDPOINTS="https://signer1.internal:8443,https://signer2.internal:8443,https://signer3.internal:8443"

# Threshold
export CUSTODY_SIGNER_THRESHOLD=2

# Per-signer auth tokens
export CUSTODY_SIGNER_AUTH_TOKENS="token-for-signer1,token-for-signer2,token-for-signer3"

# Gnosis Safe address on the target EVM chain
export CUSTODY_EVM_MULTISIG_ADDRESS="0x1234567890abcdef1234567890abcdef12345678"

# API auth token
export CUSTODY_API_AUTH_TOKEN="your-secret-api-token"
```

### Signing Flow (Automatic)

```
1. EVM withdrawal requested → custody builds Safe execTransaction data
2. POST /sign to each signer → collect individual ECDSA signatures
3. Check: got ≥ threshold signatures? If not → fail
4. Sort signatures by signer address (ascending) — Gnosis Safe requires this
5. Pack signatures into execTransaction calldata:
   - Selector: 0x6a761202
   - Parameters: to, value, data, operation, safeTxGas, baseGas, gasPrice, gasToken, refundReceiver
   - Packed signatures: sorted [r(32) + s(32) + v(1)] concatenated
6. Build transaction: [safe_address][calldata] → broadcast to EVM RPC
```

---

## Part 3: Combined Setup (Both Chains)

If your custody service handles both Solana and EVM withdrawals, set all variables together:

```bash
# ── Signer topology (shared) ──
export CUSTODY_SIGNER_ENDPOINTS="https://signer1.internal:8443,https://signer2.internal:8443,https://signer3.internal:8443"
export CUSTODY_SIGNER_THRESHOLD=2
export CUSTODY_SIGNER_AUTH_TOKENS="token1,token2,token3"

# ── Solana: FROST ──
export CUSTODY_FROST_PUBKEY_PACKAGE="<hex from DKG ceremony>"

# ── EVM: Gnosis Safe ──
export CUSTODY_EVM_MULTISIG_ADDRESS="0x<safe contract address>"

# ── Custody API ──
export CUSTODY_API_AUTH_TOKEN="<secret>"
export CUSTODY_MASTER_SEED="<secret master seed for key derivation>"
```

Each signer service must implement **both** sets of endpoints:
- `/frost/commit` and `/frost/sign` for Solana (FROST protocol)
- `/sign` for EVM (individual ECDSA)

### Single-Signer Fallback

If only 1 endpoint is configured (or threshold is 1), the custody service skips both FROST and Safe multisig entirely:
- **Solana:** The signer returns a fully signed transaction directly
- **EVM:** The signer returns a signed raw transaction directly

No `CUSTODY_FROST_PUBKEY_PACKAGE` or `CUSTODY_EVM_MULTISIG_ADDRESS` needed.

---

## Security Checklist

### DKG Ceremony
- [ ] All signer operators are present (in-person or secure video)
- [ ] Communication channel between signers is authenticated and encrypted
- [ ] Each signer generates randomness locally (never shared)
- [ ] `KeyPackage` (secret share) encrypted at rest on each signer
- [ ] `PublicKeyPackage` verified by all signers (all see the same group key)
- [ ] Group public key recorded and verified against expected Solana address
- [ ] Backup of each `KeyPackage` stored in separate secure location (not with the signer)

### Signer Services
- [ ] Each signer on separate machine / availability zone / cloud account
- [ ] Private keys never logged or transmitted
- [ ] TLS on all signer endpoints (HTTPS)
- [ ] Bearer token auth on all endpoints
- [ ] Per-signer auth tokens (`CUSTODY_SIGNER_AUTH_TOKENS`), not shared
- [ ] Request logging for audit trail (log job_id, NOT key material)
- [ ] Rate limiting on signing endpoints

### Gnosis Safe
- [ ] Safe deployed and verified on block explorer
- [ ] Owner addresses match signer service addresses
- [ ] Threshold matches `CUSTODY_SIGNER_THRESHOLD`
- [ ] Safe funded with sufficient gas + custody assets
- [ ] Safe address matches `CUSTODY_EVM_MULTISIG_ADDRESS`

### Coordinator
- [ ] `CUSTODY_API_AUTH_TOKEN` is set (custody panics without it)
- [ ] `CUSTODY_MASTER_SEED` is set (used for deterministic key derivation)
- [ ] All env vars loaded from secure vault / secrets manager (not .env files)
- [ ] Coordinator does NOT have access to any signer's private key or KeyPackage

---

## Key Rotation

### FROST Key Rotation (Solana)
1. Run a new DKG ceremony with the updated signer set
2. Transfer all funds from old group address to new group address
3. Update `CUSTODY_FROST_PUBKEY_PACKAGE` with the new hex value
4. Restart custody service
5. Old KeyPackages can be destroyed after funds are transferred

### Safe Owner Rotation (EVM)
1. Use the Safe UI or SDK to add/remove owners and adjust threshold
2. If the Safe address changes (new deployment), update `CUSTODY_EVM_MULTISIG_ADDRESS`
3. Restart custody service

---

## Troubleshooting

| Symptom | Cause | Fix |
|---------|-------|-----|
| `FROST round 1 failed: only N commitments received` | Signers unreachable or auth failure | Check signer connectivity and tokens |
| `FROST public key package not configured` | Missing env var | Set `CUSTODY_FROST_PUBKEY_PACKAGE` |
| `EVM multisig address not configured` | Missing env var | Set `CUSTODY_EVM_MULTISIG_ADDRESS` |
| `signer_threshold exceeds configured signer count` | Config mismatch | Ensure threshold ≤ number of endpoints |
| Safe `execTransaction` reverts | Signatures not sorted by address | Bug — file an issue (sorting is automatic) |
| `CRITICAL: CUSTODY_API_AUTH_TOKEN must be set` | Missing API token | Set `CUSTODY_API_AUTH_TOKEN` |
