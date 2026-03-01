# MoltChain Quantum Resistance Plan

> **Status:** Planning  
> **Author:** MoltChain Core Team  
> **Created:** 2026-03-01  
> **Target Completion:** Phased rollout (2026 – 2029)

---

## Table of Contents

1. [Executive Summary](#1-executive-summary)
2. [Threat Model](#2-threat-model)
3. [Current Cryptographic Inventory](#3-current-cryptographic-inventory)
4. [Migration Strategy Overview](#4-migration-strategy-overview)
5. [Phase 1 — Hybrid Signature Scheme](#5-phase-1--hybrid-signature-scheme-q4-2026)
6. [Phase 2 — Post-Quantum ZK Proof System](#6-phase-2--post-quantum-zk-proof-system-2027)
7. [Phase 3 — Address and Key Format Migration](#7-phase-3--address-and-key-format-migration-h1-2028)
8. [Phase 4 — P2P and Transport Layer](#8-phase-4--p2p-and-transport-layer-h2-2028)
9. [Phase 5 — Full Deprecation of Classical Crypto](#9-phase-5--full-deprecation-of-classical-crypto-2029)
10. [Candidate PQC Algorithms](#10-candidate-pqc-algorithms)
11. [Performance Budget](#11-performance-budget)
12. [Backwards Compatibility and Hard Fork Strategy](#12-backwards-compatibility-and-hard-fork-strategy)
13. [SDK and Wallet Migration](#13-sdk-and-wallet-migration)
14. [Testing and Validation](#14-testing-and-validation)
15. [Risk Register](#15-risk-register)
16. [References](#16-references)

---

## 1. Executive Summary

Cryptographically Relevant Quantum Computers (CRQCs) capable of breaking elliptic-curve discrete-log problems via Shor's algorithm are projected to emerge between 2030–2040. The "harvest now, decrypt later" threat means that blockchain transactions recorded today could be retroactively broken once CRQCs exist — exposing private keys from public keys visible on-chain.

MoltChain currently relies on **Ed25519** (signatures), **Groth16/BN254** (ZK proofs), and **Pedersen commitments** (shielded pool) — all of which are vulnerable to quantum attack. This document defines a phased migration plan to post-quantum cryptography (PQC) that:

- Preserves backwards compatibility through a hybrid transition period
- Prioritizes the highest-risk components first (signatures > ZK > transport)
- Follows NIST PQC standardization (FIPS 203/204/205, published August 2024)
- Keeps transaction sizes and verification times within acceptable bounds
- Avoids a single catastrophic hard fork by spreading changes across 5 phases

**Key principle:** We adopt a **hybrid approach** — every signature and proof includes both a classical and a post-quantum component during the transition. This protects against both quantum attacks *and* the possibility of undiscovered weaknesses in new PQC algorithms.

---

## 2. Threat Model

### 2.1 Shor's Algorithm

| Target | Attack | Impact |
|--------|--------|--------|
| Ed25519 (Curve25519 DLP) | Shor's: ~2330 logical qubits | Private key recovery from any public key ever broadcast on-chain |
| BN254 (pairing DLP) | Shor's: ~3000 logical qubits | Forge ZK proofs, break shielded pool privacy |
| secp256k1 (ECDLP) | Shor's: ~2330 logical qubits | Forge EVM bridge signatures |
| ECDH / X25519 (key exchange) | Shor's | Decrypt captured P2P handshakes retroactively |

### 2.2 Grover's Algorithm

| Target | Attack | Impact |
|--------|--------|--------|
| SHA-256 (256-bit) | Grover's: reduces to 128-bit security | Still adequate — no migration needed |
| AES-256 (symmetric) | Grover's: reduces to 128-bit security | Still adequate |
| ChaCha20-Poly1305 (256-bit key) | Grover's: reduces to 128-bit security | Still adequate |
| Poseidon (BN254 Fr, 256-bit) | Grover's: ~128-bit security | Hash itself safe; **but** it lives inside BN254 circuits which are broken |

### 2.3 "Harvest Now, Decrypt Later" (HNDL)

This is the most relevant near-term threat. Every transaction broadcast today exposes Ed25519 public keys. An adversary recording blockchain data could, once CRQCs exist:

1. Derive private keys from every public key ever seen
2. Drain any wallet that ever signed a transaction
3. Forge historical shielded proofs, compromising privacy retroactively

**Mitigation:** Make addresses quantum-safe *before* CRQCs exist, so attackers cannot derive keys from on-chain data.

### 2.4 Timeline Estimate

| Source | Projected CRQC Date |
|--------|-------------------|
| NIST PQC Project (2024) | "Plan migration now" |
| IBM Quantum Roadmap | 100K qubits by 2033 |
| Google Willow (2024) | Demonstrates error correction breakthrough |
| BSI (Germany) | Recommends PQC by 2030 |
| NSA CNSA 2.0 (2022) | Mandates PQC for national security by 2035 |
| Consensus estimate | **2032–2040 for cryptographically relevant** |

**MoltChain position:** Begin migration in 2026 to be fully quantum-safe by 2029, well before projected CRQC availability.

---

## 3. Current Cryptographic Inventory

### 3.1 Quantum-Vulnerable Components (Must Migrate)

| Component | Algorithm | Location | Risk Level |
|-----------|-----------|----------|------------|
| Transaction signatures | Ed25519 (EdDSA/Curve25519) | `core/src/account.rs`, `core/src/transaction.rs` | **CRITICAL** |
| Block producer signatures | Ed25519 | `core/src/block.rs` | **CRITICAL** |
| Validator identity | Ed25519 | `validator/src/keypair_loader.rs` | **CRITICAL** |
| P2P gossip authentication | Ed25519 | `p2p/src/message.rs` | **HIGH** |
| ZK proof system | Groth16 over BN254 | `core/src/zk/prover.rs`, `verifier.rs`, `setup.rs` | **CRITICAL** |
| Pedersen commitments | BN254 G1 scalar multiply | `core/src/zk/pedersen.rs` | **CRITICAL** |
| Shielded Merkle tree (in-circuit) | Poseidon over BN254 Fr | `core/src/zk/merkle.rs` (used inside Groth16 circuits) | **HIGH** |
| Shielded keypairs | BN254 scalar multiply (spending → viewing) | `core/src/zk/keys.rs` | **CRITICAL** |
| Nullifier derivation | Poseidon over BN254 Fr | `core/src/zk/note.rs` | **HIGH** |
| FROST threshold signatures | FROST-Ed25519 (t-of-n) | `custody/src/main.rs` | **HIGH** |
| EVM bridge signing | secp256k1/ECDSA (k256) | `custody/Cargo.toml` | **MEDIUM** |
| TLS handshake (P2P) | X25519/P-256 ECDH via rustls + QUIC | `p2p/src/peer.rs` | **MEDIUM** |
| Wallet signing (browser) | Ed25519 via TweetNaCl | `wallet/js/crypto.js`, `sdk/js/src/keypair.ts` | **CRITICAL** |

### 3.2 Quantum-Safe Components (No Migration Needed)

| Component | Algorithm | Location | Notes |
|-----------|-----------|----------|-------|
| Block hashing | SHA-256 | `core/src/hash.rs` | 128-bit post-quantum security |
| Transaction hashing | SHA-256 | `core/src/transaction.rs` | Safe |
| Note encryption | ChaCha20-Poly1305 | `core/src/zk/note.rs` | 128-bit post-quantum security |
| Wallet key storage | AES-256-GCM | `wallet/js/crypto.js` | Safe |
| Mnemonic → seed | PBKDF2-HMAC-SHA512 | `wallet/js/crypto.js` | Safe |
| Password → AES key | PBKDF2-HMAC-SHA256 (100K rounds) | `wallet/js/crypto.js` | Safe |
| EVM address hashing | Keccak-256 | `core/src/account.rs` | Safe |
| State root | SHA-256 | `core/src/block.rs` | Safe |

---

## 4. Migration Strategy Overview

```
  2026-Q4         2027           2028-H1        2028-H2        2029
 ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐
 │ PHASE 1  │  │ PHASE 2  │  │ PHASE 3  │  │ PHASE 4  │  │ PHASE 5  │
 │ Hybrid   │  │ PQ ZK    │  │ Address  │  │ P2P/TLS  │  │ Full     │
 │ Sigs     │  │ Proofs   │  │ Migration│  │ Hardening│  │ Cutover  │
 │          │  │          │  │          │  │          │  │          │
 │ ML-DSA + │  │ Lattice  │  │ New addr │  │ ML-KEM   │  │ Deprecate│
 │ Ed25519  │  │ SNARK or │  │ format,  │  │ in QUIC  │  │ classical│
 │ dual sig │  │ STARK    │  │ wallet   │  │ handshake│  │ codepaths│
 └──────────┘  └──────────┘  │ migration│  └──────────┘  └──────────┘
                              └──────────┘
```

**Core principles:**

1. **Hybrid-first:** Every phase introduces PQC alongside classical crypto, never replacing it outright until Phase 5.
2. **Fail-safe:** If a PQC algorithm is broken, the classical component still provides security.
3. **Incremental hard forks:** Each phase requires a coordinated network upgrade, but they are independent.
4. **Wallet migration window:** Users get 12+ months to migrate to new address formats.

---

## 5. Phase 1 — Hybrid Signature Scheme (Q4 2026)

### 5.1 Objective

Replace Ed25519-only transaction and block signatures with a **hybrid Ed25519 + ML-DSA-65** dual-signature scheme. A transaction is valid only if **both** signatures verify.

### 5.2 Algorithm Selection

| Algorithm | NIST Standard | Type | Sig Size | PK Size | SK Size | Verify Speed |
|-----------|--------------|------|----------|---------|---------|--------------|
| **ML-DSA-65** (Dilithium3) | FIPS 204 | Lattice (Module-LWE) | 3,309 B | 1,952 B | 4,032 B | ~0.5 ms |
| SLH-DSA-128f (SPHINCS+) | FIPS 205 | Hash-based (stateless) | 17,088 B | 32 B | 64 B | ~5 ms |
| FALCON-512 | Round 4 finalist | Lattice (NTRU) | 666 B | 897 B | 1,281 B | ~0.1 ms |

**Decision: ML-DSA-65 (Dilithium3)** — NIST's primary recommendation. Best balance of signature size, verification speed, and maturity. SLH-DSA kept as backup hash-based option (conservative fallback if lattice assumptions break).

### 5.3 Transaction Format Change

Current:
```
Transaction {
    signatures: Vec<[u8; 64]>,     // Ed25519 (64 bytes each)
    message: TransactionMessage,
}
```

Phase 1:
```
Transaction {
    signatures: Vec<HybridSignature>,
    message: TransactionMessage,
}

struct HybridSignature {
    /// Classical Ed25519 signature (64 bytes)
    ed25519_sig: [u8; 64],
    /// Post-quantum ML-DSA-65 signature (3,309 bytes)
    pq_sig: Vec<u8>,
    /// Signature scheme version (for future algorithm agility)
    scheme_version: u8,
}
```

### 5.4 Block Header Change

```
BlockHeader {
    // ... existing fields ...
    signature: [u8; 64],           // Ed25519 (kept for light clients)
    pq_signature: Vec<u8>,         // ML-DSA-65 (new)
    pq_validator_pk: [u8; 1952],   // Validator's PQ public key
}
```

### 5.5 Implementation Steps

| # | Task | Files Affected | Effort |
|---|------|---------------|--------|
| 1.1 | Add `pqcrypto-dilithium` or `ml-dsa` crate to `core/Cargo.toml` | `core/Cargo.toml` | S |
| 1.2 | Define `HybridKeypair` struct wrapping `(Ed25519Keypair, MlDsaKeypair)` | `core/src/account.rs` | M |
| 1.3 | Update `Keypair::sign()` to produce `HybridSignature` | `core/src/account.rs` | M |
| 1.4 | Update `verify_transaction_signatures()` to require both sigs | `core/src/transaction.rs` | M |
| 1.5 | Update `BlockHeader` to include `pq_signature` field | `core/src/block.rs` | M |
| 1.6 | Update validator keypair generation and loading | `validator/src/keypair_loader.rs` | M |
| 1.7 | Update CLI keypair manager to generate dual keypairs | `cli/src/keypair_manager.rs` | M |
| 1.8 | Serialize/deserialize hybrid sigs in RPC layer | `rpc/src/lib.rs` | L |
| 1.9 | Update P2P gossip message signing (dual sig) | `p2p/src/message.rs` | M |
| 1.10 | Update FROST custody to produce hybrid threshold sigs | `custody/src/main.rs` | L |
| 1.11 | Add ML-DSA to JS SDK (`pqc-dilithium` WASM module) | `sdk/js/`, `wallet/js/` | L |
| 1.12 | Add ML-DSA to Python SDK | `sdk/python/` | M |
| 1.13 | Add ML-DSA to Rust SDK | `sdk/rust/` | M |
| 1.14 | Update transaction size limits and fee calculations | `core/src/transaction.rs`, `rpc/src/lib.rs` | M |
| 1.15 | Genesis block includes PQ public keys for initial validators | `core/src/genesis.rs` | M |
| 1.16 | Hard fork activation: slot-based cutover | `core/src/block.rs` | M |
| 1.17 | E2E tests for hybrid sig verification | `tests/` | L |

### 5.6 Size Impact

| Metric | Before | After | Δ |
|--------|--------|-------|---|
| Single signature | 64 B | 3,374 B | +3,310 B (+52x) |
| Typical TX (1 sig) | ~250 B | ~3,560 B | +3,310 B |
| Block header | ~300 B | ~5,560 B | +5,260 B |
| Public key (on-chain) | 32 B | 1,984 B | +1,952 B |
| Block throughput @ 10 MB/block | ~40K TX | ~2,800 TX | Needs block size increase |

**Mitigation:** Increase max block size to 40 MB or introduce signature aggregation for blocks (a single validator ML-DSA signature covers all included TXs via Merkle root signing).

### 5.7 Rust Crate Options

| Crate | Status | Notes |
|-------|--------|-------|
| `pqcrypto-dilithium` | Mature, C bindings | Battle-tested reference implementation |
| `ml-dsa` | Pure Rust (RustCrypto) | `ml-dsa = "0.2"` — FIPS 204 compliant, no C deps |
| `fips204` | Pure Rust | NIST FIPS 204 direct implementation |

**Recommendation:** Use `ml-dsa` (RustCrypto project) for Rust, `pqc-dilithium` WASM for JS/browser.

---

## 6. Phase 2 — Post-Quantum ZK Proof System (2027)

### 6.1 Objective

Replace Groth16/BN254 with a **quantum-resistant zero-knowledge proof system** for the shielded pool. This is the most complex phase.

### 6.2 The BN254/Groth16 Problem

Groth16 is elegant (128-byte proofs, ~2ms verification), but its security rests entirely on the hardness of the discrete-log problem on BN254's pairing groups. A quantum computer breaks this completely — an attacker could:

1. Forge proofs for any statement (mint shielded MOLT from nothing)
2. Derive spending keys from viewing keys
3. Break Pedersen commitment hiding (reveal shielded amounts)
4. Compute nullifiers without the spending key (track shielded recipients)

### 6.3 Candidate Replacement Systems

| System | Type | Proof Size | Verify Time | Trusted Setup | PQ-Safe | Maturity |
|--------|------|-----------|-------------|---------------|---------|----------|
| **STARKs** (e.g., Plonky3) | Hash-based (FRI) | 50–200 KB | 5–50 ms | **No** (transparent) | **Yes** | High |
| **Lattice SNARKs** (e.g., LAT-ZK) | Lattice-based | 10–50 KB | 10–30 ms | Yes (lattice SRS) | **Yes** | Low (research) |
| **Hash-based SNARKs** (e.g., Ligero++) | MPC-in-the-head | 100–500 KB | 20–100 ms | **No** | **Yes** | Medium |
| **Recursive STARKs** (Plonky3 + FRI) | Hash-based recursive | 30–80 KB (after folding) | 10–30 ms | **No** | **Yes** | High |

**Decision: STARKs (Plonky3/Goldilocks or custom FRI-based)**

Rationale:
- No trusted setup (eliminates CRS vulnerability)
- Quantum-safe by construction (relies only on hash function collision resistance)
- Proof sizes are larger but acceptable for MoltChain's block size
- Mature enough by 2027 (StarkNet, Polygon Miden, and others are deploying today)
- Can use Poseidon hash (our existing tree hash) over a PQ-safe field

### 6.4 Migration Plan

#### Phase 2a: Circuit Redesign (Q1–Q2 2027)

Rewrite the three ZK circuits in a STARK-friendly constraint system:

| Circuit | Current (Groth16 R1CS) | Target (STARK AIR) |
|---------|----------------------|-------------------|
| **shield.rs** | ~370 R1CS constraints | ~2,000 AIR constraints |
| **unshield.rs** | ~600 R1CS constraints | ~3,500 AIR constraints |
| **transfer.rs** (2-in-2-out) | ~1,800 R1CS constraints | ~10,000 AIR constraints |

The circuits prove the same statements:
- **Shield:** "I know `(value, blinding)` such that `commitment = Poseidon(value, blinding)` and `value` matches the deposited amount"
- **Unshield:** "I know a Merkle path proving my commitment exists in the tree, and I know the spending key that derives the nullifier"
- **Transfer:** "I know spending keys for 2 input notes, and the output commitments conserve value"

#### Phase 2b: Hash Function Migration (Q2 2027)

Replace Poseidon-over-BN254-Fr with Poseidon2 or Rescue over a 64-bit field (Goldilocks p = 2^64 − 2^32 + 1) for STARK-friendliness. Alternatively, keep Poseidon but over a hash-friendly field.

| Option | Field | STARK-friendly | Notes |
|--------|-------|---------------|-------|
| **Poseidon2/Goldilocks** | p = 2^64 − 2^32 + 1 | Excellent | Plonky3 native; very fast in hardware |
| **Poseidon/BabyBear** | p = 2^31 − 1 | Excellent | Risc0/SP1 native |
| **Rescue-XLIX** | Any prime | Good | Alternative algebraic hash |
| **Blake3** (non-algebraic) | N/A | Poor (expensive in-circuit) | Conservative but impractical |

**Decision:** Poseidon2 over Goldilocks is the strongest candidate — native to Plonky3, hardware-acceleratable, and quantum-safe.

#### Phase 2c: Pedersen Commitment Replacement (Q3 2027)

Replace BN254 Pedersen commitments with one of:

| Replacement | Type | Size | PQ-Safe |
|-------------|------|------|---------|
| **Poseidon-based commitment** | `Poseidon(value ‖ blinding)` | 32 B | Yes (hash-based) |
| **Lattice commitment** | Module-LWE hiding | ~3 KB | Yes |
| **SWIFFT commitment** | Lattice hash | ~512 B | Yes |

**Decision:** Poseidon-based commitment `C = Poseidon(value, blinding, owner_pk)` — already used in our note scheme, just formalize it as the canonical commitment without Pedersen.

#### Phase 2d: Verifier Integration (Q4 2027)

| # | Task | Effort |
|---|------|--------|
| 2.1 | Add STARK verifier crate (`plonky3` or `winterfell`) to `core/Cargo.toml` | M |
| 2.2 | Implement AIR constraints for shield/unshield/transfer circuits | XL |
| 2.3 | Replace `core/src/zk/prover.rs` with STARK prover | L |
| 2.4 | Replace `core/src/zk/verifier.rs` with STARK verifier | L |
| 2.5 | Update proof format in `ZkProof` struct (128 B → ~80 KB) | M |
| 2.6 | Replace `core/src/zk/pedersen.rs` with Poseidon commitment | M |
| 2.7 | Replace `core/src/zk/keys.rs` — derive shielded keys from hash-based scheme | M |
| 2.8 | Replace `core/src/zk/note.rs` — nullifier derivation via Poseidon2 | M |
| 2.9 | Update Merkle tree to Poseidon2/Goldilocks field | L |
| 2.10 | New trusted-setup-free ceremony (just publish parameters) | S |
| 2.11 | Update RPC shielded handlers for new proof format | L |
| 2.12 | Update wallet ZK proof generation (WASM STARK prover) | XL |
| 2.13 | Dual-verification period: accept both Groth16 and STARK proofs | L |
| 2.14 | Migration tool: re-shield existing notes under new commitment scheme | L |
| 2.15 | E2E tests for STARK-based shielded transactions | L |

### 6.5 Shielded Pool Migration Flow

Users must migrate existing shielded notes from the Groth16 pool to the STARK pool:

```
  Old Pool (Groth16/BN254)           New Pool (STARK/Poseidon2)
  ┌─────────────────────┐            ┌─────────────────────┐
  │  Merkle Root (old)  │            │  Merkle Root (new)  │
  │  BN254 Pedersen     │   migrate  │  Poseidon2 commit   │
  │  commitments        │  ───────>  │  commitments        │
  │                     │  (unshield │                     │
  │  Groth16 proofs     │   + re-    │  STARK proofs       │
  └─────────────────────┘  shield)   └─────────────────────┘
```

1. User unshields from old pool (Groth16 proof, valid during transition)
2. User immediately re-shields into new pool (STARK proof)
3. After transition window closes: old pool frozen, only new pool accepts deposits
4. Remaining old-pool balance becomes claimable via governance vote

---

## 7. Phase 3 — Address and Key Format Migration (H1 2028)

### 7.1 Objective

Introduce a new **quantum-resistant address format** so that wallets holding funds in never-spent addresses are protected (currently, the Base58-encoded Ed25519 public key is the address — directly vulnerable).

### 7.2 New Address Scheme

```
Current:  address = Base58(ed25519_pubkey)             → 32 bytes, quantum-vulnerable
New:      address = Base58Check(hash(hybrid_pubkey))   → 32 bytes, quantum-safe
```

Where `hybrid_pubkey = ed25519_pk ‖ ml_dsa_pk` (32 + 1,952 = 1,984 bytes), and the address is `SHA-256(hybrid_pubkey)[0..32]`.

**Key insight:** The address is now a hash of the public key, not the public key itself. The full public keys are only revealed when a transaction is signed (same as Bitcoin's P2PKH model). This means:

- Unspent addresses are quantum-safe (attacker only sees the hash)
- Addresses that have sent a transaction expose the public key, but the hybrid signature still requires breaking both Ed25519 and ML-DSA

### 7.3 Address Version Byte

```
Version 0x00: Legacy Ed25519 (raw pubkey) — deprecated
Version 0x01: Hybrid Ed25519 + ML-DSA-65 (hash-of-pubkey)
Version 0x02: Pure ML-DSA-65 (hash-of-pubkey) — Phase 5
Version 0x03: Reserved for future PQC algorithm rotation
```

### 7.4 Implementation Steps

| # | Task | Effort |
|---|------|--------|
| 3.1 | Define `QuantumAddress` type with version byte | M |
| 3.2 | Update account model to store `hybrid_pubkey` on first use | L |
| 3.3 | Update RPC `getAccount` / `getBalance` for new addresses | M |
| 3.4 | Wallet migration flow: generate PQ keypair, register hybrid address | L |
| 3.5 | Faucet support for new address format | S |
| 3.6 | Explorer UI update: display address version, PQ status indicator | M |
| 3.7 | SDK updates: new address generation in JS/Python/Rust SDKs | L |
| 3.8 | Grace period: both address formats accepted for 12 months | M |

### 7.5 Wallet Migration UX

```
┌─────────────────────────────────────────────────┐
│  ⚠️ Quantum Upgrade Available                    │
│                                                   │
│  Your wallet uses a legacy address format that   │
│  may become vulnerable to future quantum          │
│  computers.                                       │
│                                                   │
│  Upgrade now to generate a quantum-resistant      │
│  keypair and migrate your funds to a new address. │
│                                                   │
│  Your mnemonic phrase stays the same — a new PQ   │
│  keypair is derived from it.                      │
│                                                   │
│  [Upgrade Now]          [Remind Me Later]         │
└─────────────────────────────────────────────────┘
```

---

## 8. Phase 4 — P2P and Transport Layer (H2 2028)

### 8.1 Objective

Replace X25519/P-256 ECDH in the QUIC/TLS handshake with **ML-KEM-768** (Kyber) key encapsulation.

### 8.2 Algorithm

| Algorithm | NIST Standard | Type | Ciphertext | PK Size | SK Size |
|-----------|--------------|------|-----------|---------|---------|
| **ML-KEM-768** (Kyber768) | FIPS 203 | Lattice (Module-LWE) | 1,088 B | 1,184 B | 2,400 B |

### 8.3 Implementation

| # | Task | Effort |
|---|------|--------|
| 4.1 | Update `rustls` to a version with ML-KEM support (expected 2027) | M |
| 4.2 | Configure QUIC (`quinn`) to prefer ML-KEM-768 + X25519 hybrid KEM | M |
| 4.3 | Update validator announcement messages with PQ identity keys | M |
| 4.4 | Update peer discovery to include PQ capability flags | S |
| 4.5 | E2E test: PQ-handshake between validators | M |

**Note:** `rustls` is already tracking PQC (see [rustls#1913](https://github.com/rustls/rustls/issues/1913)). By 2028, hybrid X25519+ML-KEM should be a built-in `CipherSuite` option.

### 8.4 Validator Gossip Authentication

Replace Ed25519-only gossip signatures with hybrid dual-sig (already done in Phase 1). Phase 4 specifically hardens the transport layer underneath.

---

## 9. Phase 5 — Full Deprecation of Classical Crypto (2029)

### 9.1 Objective

Remove all classical-only code paths. After Phase 5, MoltChain is **purely post-quantum**.

### 9.2 Deprecation Schedule

| Milestone | Action |
|-----------|--------|
| 2029-Q1 | Consensus rejects v0x00 (legacy Ed25519-only) signatures |
| 2029-Q1 | Consensus rejects Groth16 proofs for new shielded operations |
| 2029-Q2 | Old shielded pool frozen — unshield-only via governance |
| 2029-Q3 | Remove Ed25519-only verification paths from validator code |
| 2029-Q4 | Drop `ed25519-dalek`, `ark-groth16`, `ark-bn254` from dependencies |

### 9.3 Final Cryptographic Stack

| Layer | Algorithm | Standard | PQ Security Level |
|-------|-----------|----------|-------------------|
| Signatures | ML-DSA-65 | FIPS 204 | NIST Level 3 (~128-bit PQ) |
| ZK Proofs | STARKs (FRI-based) | — | Hash-based (128-bit PQ) |
| Commitments | Poseidon2 | — | Hash-based (128-bit PQ) |
| Key exchange | ML-KEM-768 | FIPS 203 | NIST Level 3 |
| Hashing | SHA-256 + Poseidon2 | FIPS 180-4 | 128-bit PQ |
| Symmetric encryption | ChaCha20-Poly1305 / AES-256 | — | 128-bit PQ |
| Addresses | Hash-of-PQ-pubkey | — | Pre-image resistant (128-bit PQ) |

---

## 10. Candidate PQC Algorithms

### 10.1 NIST Standardized (August 2024)

| Algorithm | Standard | Type | Use Case |
|-----------|----------|------|----------|
| **ML-KEM** (Kyber) | FIPS 203 | Lattice KEM | Key encapsulation (TLS, P2P) |
| **ML-DSA** (Dilithium) | FIPS 204 | Lattice Signature | Transaction/block signing |
| **SLH-DSA** (SPHINCS+) | FIPS 205 | Hash-based Signature | Conservative backup (no lattice assumption) |

### 10.2 NIST Round 4 / Future Standards

| Algorithm | Type | Use Case | Status |
|-----------|------|----------|--------|
| **FALCON** | Lattice (NTRU) Signature | Compact signatures (666 B) | Expected standardization 2025–2026 |
| **BIKE** | Code-based KEM | Alternative KEM | Round 4 |
| **HQC** | Code-based KEM | Alternative KEM | Round 4 |
| **Classic McEliece** | Code-based KEM | Highly conservative | Round 4 (very large keys) |

### 10.3 Algorithm Agility

The `scheme_version` byte in `HybridSignature` and address version byte provide **algorithm agility** — if ML-DSA is broken or a better algorithm emerges, we can introduce `scheme_version = 2` without another full migration.

---

## 11. Performance Budget

### 11.1 Signature Verification

| Operation | Current (Ed25519) | Phase 1 (Hybrid) | Phase 5 (ML-DSA only) |
|-----------|-------------------|-------------------|----------------------|
| Sign | ~60 µs | ~120 µs (both) | ~80 µs |
| Verify | ~120 µs | ~220 µs (both) | ~100 µs |
| Sig size | 64 B | 3,374 B | 3,309 B |
| PK size | 32 B | 1,984 B | 1,952 B |

### 11.2 ZK Proof

| Operation | Current (Groth16) | Phase 2 (STARK) | Ratio |
|-----------|-------------------|-----------------|-------|
| Prove (shield) | ~2 s | ~5 s | 2.5x |
| Prove (transfer) | ~8 s | ~20 s | 2.5x |
| Verify | ~2 ms | ~15 ms | 7.5x |
| Proof size | 128 B | ~80 KB | ~625x |
| On-chain storage | Minimal | Moderate | Needs compression |

### 11.3 Block Size Impact

| Scenario | Current Size | Post-PQ Size | Notes |
|----------|-------------|--------------|-------|
| Avg TX (1 sig) | ~250 B | ~3,600 B | 14x increase |
| Block (1000 TXs) | ~250 KB | ~3.6 MB | Block limit increase needed |
| Block + 10 shielded TXs | ~252 KB | ~4.4 MB | STARK proofs add ~80 KB each |

**Mitigation strategies:**
1. **Signature aggregation:** Validator signs a Merkle root of all TX hashes — 1 ML-DSA sig per block instead of per-TX
2. **STARK proof aggregation/recursion:** Batch multiple shielded TXs into 1 recursive STARK proof
3. **Increase block size limit:** From 10 MB → 40 MB (network bandwidth allows it)
4. **Proof compression:** FRI folding reduces STARK proofs by 2-4x

---

## 12. Backwards Compatibility and Hard Fork Strategy

### 12.1 Fork Schedule

| Phase | Fork Name | Activation Method | Breaking Change |
|-------|-----------|------------------|-----------------|
| 1 | `quantum-shield-1` | Slot-based (epoch N) | New TX format, new block header field |
| 2 | `quantum-shield-2` | Slot-based | New proof format, new shielded pool |
| 3 | `quantum-shield-3` | Slot-based | New address format, old addresses deprecated |
| 4 | `quantum-shield-4` | Slot-based | New TLS ciphersuites required |
| 5 | `quantum-final` | Slot-based | Classical-only code removed |

### 12.2 Compatibility Rules During Transition

| Period | Ed25519-only TX | Hybrid TX | ML-DSA-only TX |
|--------|----------------|-----------|----------------|
| Pre-Phase 1 | ✅ Valid | N/A | N/A |
| Phase 1–4 | ✅ Valid (deprecated) | ✅ Valid | ❌ Rejected |
| Phase 5+ | ❌ Rejected | ❌ Rejected | ✅ Valid |

### 12.3 Validator Upgrade Requirements

- **Phase 1:** Validators must upgrade before fork slot. Non-upgraded validators will reject new TX format and fork off.
- **Minimum upgrade window:** 30 days notice, 2 testnet rehearsals before each mainnet fork.

---

## 13. SDK and Wallet Migration

### 13.1 JavaScript SDK / Wallet

| Task | Library | Notes |
|------|---------|-------|
| ML-DSA signing | `pqc-dilithium` (WASM) or `liboqs-js` | ~300 KB WASM bundle |
| ML-KEM key exchange | `pqc-kyber` (WASM) | ~200 KB WASM bundle |
| Address format update | Native JS | Minimal |
| STARK proof generation | `plonky3-wasm` or `winterfell-wasm` | ~2 MB WASM bundle, heavy |
| Browser extension (MV3) | Same WASM modules | CSP must allow WASM |

**WASM size mitigation:** Lazy-load PQC modules only when signing. Tree-shake unused algorithms.

### 13.2 Python SDK

| Task | Library | Notes |
|------|---------|-------|
| ML-DSA | `pqcrypto` (pip) or `liboqs-python` | C bindings, well-tested |
| ML-KEM | `pqcrypto` | Same package |
| STARK proofs | `winterfell-py` or FFI to Rust | Proof gen delegated to RPC node |

### 13.3 Rust SDK

| Task | Library | Notes |
|------|---------|-------|
| ML-DSA | `ml-dsa` (RustCrypto) | Pure Rust, FIPS 204 compliant |
| ML-KEM | `ml-kem` (RustCrypto) | Pure Rust, FIPS 203 compliant |
| STARK proofs | `plonky3` or `winterfell` | Native Rust, full speed |

---

## 14. Testing and Validation

### 14.1 Test Categories

| Category | Description | Phase |
|----------|-------------|-------|
| **Unit tests** | ML-DSA sign/verify, STARK prove/verify, new address generation | All |
| **Integration tests** | Hybrid TX through full pipeline (sign → propagate → validate → finalize) | 1 |
| **E2E matrix** | Extend 43-test matrix with PQ-specific scenarios | All |
| **Fuzz testing** | Fuzz hybrid signature parsing, proof deserialization | 1, 2 |
| **Interop tests** | Cross-SDK signing (Rust sign → JS verify, etc.) | 1, 3 |
| **Performance benchmarks** | Verify throughput regression stays within budget | All |
| **Testnet rehearsal** | Full fork rehearsal on testnet before each mainnet upgrade | All |
| **KAT vectors** | NIST Known Answer Tests for ML-DSA and ML-KEM | 1, 4 |

### 14.2 Acceptance Criteria

| Metric | Threshold |
|--------|-----------|
| Hybrid signature verification (1 sig) | < 500 µs |
| Block validation (1000 hybrid TXs) | < 500 ms |
| STARK proof generation (shield) | < 10 s |
| STARK verification (1 proof) | < 30 ms |
| Network throughput (TXs/sec) | ≥ 70% of pre-PQ baseline |
| All existing matrix tests | 43/43 pass (no regression) |
| NIST KAT compliance | 100% |

---

## 15. Risk Register

| ID | Risk | Impact | Likelihood | Mitigation |
|----|------|--------|------------|------------|
| R1 | ML-DSA lattice assumption broken | Critical — signatures forgeable | Low (well-studied since 2017) | SLH-DSA (hash-based) hot-swap via `scheme_version` |
| R2 | STARK proof sizes too large for mobile | High — poor wallet UX | Medium | Proof delegation to trusted node; recursive proof compression |
| R3 | WASM PQC bundles too large for browser | Medium — slow wallet load | Medium | Lazy loading; CDN caching; subset of algorithms |
| R4 | CRQCs arrive before Phase 5 complete | Critical — chain vulnerable | Low (before 2029) | Emergency fast-track: skip to ML-DSA-only, freeze old pool |
| R5 | Hard fork coordination failure | High — chain split | Medium | Testnet rehearsals; 30-day notice; automatic fork-detection |
| R6 | Performance regression exceeds budget | Medium — UX degradation | Medium | Signature aggregation; STARK recursion; hardware acceleration |
| R7 | `pqcrypto` supply chain attack (WASM/npm) | High — compromised signing | Low | Audit dependencies; reproducible builds; in-house WASM compilation |
| R8 | "Harvest now" attack on shielded pool | Medium — future privacy loss | Medium | Prioritize Phase 2 if CRQC timeline accelerates |

### Emergency Response Plan

If credible CRQC announcement occurs before migration completes:
1. **Immediate:** Governance proposal to freeze all non-PQ-upgraded accounts
2. **48 hours:** Emergency hard fork requiring hybrid signatures
3. **1 week:** Freeze old shielded pool; allow unshield-only
4. **1 month:** New-address-only mode (no legacy addresses)

---

## 16. References

| # | Reference |
|---|-----------|
| 1 | NIST FIPS 203 — ML-KEM (Kyber), August 2024 |
| 2 | NIST FIPS 204 — ML-DSA (Dilithium), August 2024 |
| 3 | NIST FIPS 205 — SLH-DSA (SPHINCS+), August 2024 |
| 4 | NSA CNSA 2.0 — Commercial National Security Algorithm Suite |
| 5 | Bernstein & Lange, "Post-quantum cryptography" (2017) |
| 6 | StarkWare, "Ethically sound STARK proofs" (2024) |
| 7 | Polygon Miden, "STARK-based rollup architecture" (2024) |
| 8 | RustCrypto `ml-dsa` crate — https://github.com/RustCrypto/signatures |
| 9 | RustCrypto `ml-kem` crate — https://github.com/RustCrypto/KEMs |
| 10 | Plonky3 — https://github.com/Plonky3/Plonky3 |
| 11 | Winterfell STARK prover — https://github.com/facebook/winterfell |
| 12 | NIST PQC Project — https://csrc.nist.gov/projects/post-quantum-cryptography |
| 13 | IBM Quantum Roadmap 2033 — https://www.ibm.com/quantum/roadmap |

---

## Appendix A: Glossary

| Term | Definition |
|------|-----------|
| **CRQC** | Cryptographically Relevant Quantum Computer — a quantum computer large enough to run Shor's algorithm against real-world key sizes |
| **HNDL** | Harvest Now, Decrypt Later — adversary records encrypted/signed data today for future quantum decryption |
| **ML-DSA** | Module Lattice Digital Signature Algorithm (formerly Dilithium) — NIST FIPS 204 |
| **ML-KEM** | Module Lattice Key Encapsulation Mechanism (formerly Kyber) — NIST FIPS 203 |
| **SLH-DSA** | Stateless Hash-based Digital Signature Algorithm (formerly SPHINCS+) — NIST FIPS 205 |
| **STARK** | Scalable Transparent ARgument of Knowledge — hash-based ZK proof system, no trusted setup |
| **SNARK** | Succinct Non-interactive ARgument of Knowledge — pairing-based ZK proof (quantum-vulnerable) |
| **FRI** | Fast Reed-Solomon Interactive Oracle Proof — core of STARK verification |
| **Poseidon2** | Next-generation SNARK/STARK-friendly hash, algebraic over prime fields |
| **BN254** | Barreto-Naehrig 254-bit pairing curve — used by current Groth16 proofs |
| **AIR** | Algebraic Intermediate Representation — constraint format for STARKs |
| **R1CS** | Rank-1 Constraint System — constraint format for Groth16 SNARKs |

---

*This document is a living plan. Algorithm selections will be revisited annually as the PQC landscape evolves.*
