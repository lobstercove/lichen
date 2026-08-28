# Shielded Scheme 0x01 Fail-Closed Review and Transition Plan

Date: 2026-08-28

Status: legacy-scheme release blocker closed locally by fail-closed enforcement;
privacy reactivation and legacy custody remain blocked. No release, tag,
deployment, governance transaction, or live validator mutation was performed
as part of this review.

## Decision

The `plonky3-fri-poseidon2` proof format identified as scheme `0x01` is not a
sound authorization proof and must not be accepted for Shield, Unshield,
ShieldedTransfer, or Neo reserve/liability verification.

The verifier retains the identifier solely for decoding historical envelopes
and regression fixtures. Every proof type returns a disabled-scheme error.
Public pool/history queries remain available, while proof generation,
secret-bearing hash helpers, proof verification, CLI proof commands, wallet
actions, and REST submission fail closed.

## Evidence

- Unshield, Transfer, and ReserveLiability use `ConstantTraceAir`; the trace is
  only repeated public values.
- Host-side prover checks do not constrain a malicious prover, which can create
  a proof directly from arbitrary public inputs.
- `ShieldAir` constrains only a small public amount/value relation and does not
  bind the commitment to the private value and blinding.
- A local regression constructs a public-input-only Unshield proof that the old
  verification path accepts. The production verifier now rejects the same
  envelope as disabled.

The last supplied read-only live observation at slot 12,265,929 reported 21
commitments, 14.112020000 LICN, 16 nullifiers, 21 shields, 16 unshields, and no
transfers. This observation is not a new live verification and must not be
treated as current without another authorized read-only check.

## Current Fail-Closed Contract

- Scheme `0x01` is rejected for every proof type before proof deserialization.
- RPC proof and private hash helpers return JSON-RPC `-32090` before parsing
  witness parameters.
- Shielded REST POST routes return HTTP 503 before transaction decoding.
- CLI proof commands exit with status 2 before reading witness files.
- Web and extension wallets do not send spending keys, blindings, serials, or
  Merkle witnesses to validator RPC methods.
- The Shielded protocol-module pause applies to Shield, Unshield, and Transfer.
  Exit-only mode is unsafe while ownership verification is unsound.
- Custody mutations require one `StateBatch`; account, commitment, encrypted
  payload, nullifier, pool, and transaction effects roll back together.
- Public RPC history reads validate tree capacity and root encoding, abort on
  missing commitment indices or root conflicts, reject malformed or
  commitment-conflicting note payloads, and clamp pagination at the exact tip.
- Replay uses checked counters and indices and aborts on malformed, zero,
  non-canonical, duplicate, missing, root-conflicting, or over-capacity state.
- The depth-20 commitment tree has an exact 1,048,576-leaf capacity.

## Local Verification

The shielded remediation currently passes:

- `cargo fmt --all -- --check`;
- strict `cargo clippy` with all targets/features and `-D warnings` for core,
  RPC, and CLI;
- 78 core ZK tests, 7 lifecycle tests, 6 privileged-action assurance tests,
  51 shielded RPC integration tests, and 4 standalone compatibility-contract
  tests;
- wallet audit (136), extension audit (135), extension signing/provider E2E
  (9), and frontend asset-integrity checks (380), all with zero failures.

The formerly failing reward-adjustment and contract-ABI fixtures were corrected
in the same v0.5.264 candidate. The complete workspace all-feature test suite,
strict workspace Clippy, 336 RPC unit tests, 254 RPC full-coverage tests, all 34
native contract suites, all 33 release-WASM contract workflows, and the
169-case ABI/WASM dispatch gate are green.

The exact clean-build four-validator gate also passed with four independently
owned genesis voters, all four observed as proposers, fresh join, quorum loss,
96 queued transactions and drain, a 140-slot live gap, individual and
coordinated restarts, Archive V2 corruption/source-outage paths, strict volume,
launchpad, and terminal slot-7,000 parity. Every validator matched
public-history root
`05c85c39e4e5ec572e813574df4ceda1166006b60f40d9a73a4dd39655db9e64`
and state root
`8a5a5c79fa2420debcadce20abe72dc58e78be4dbc93d2f4bca12dd1fa5d117b`.
This qualifies the fail-closed implementation locally; it does not authorize a
shielded successor, a legacy-note claim process, or a live deployment.

## Versioned Successor Requirements

Do not reactivate scheme `0x01`. A successor must receive a new scheme ID only
after its statement, implementation, resource bounds, and activation rules are
fixed and independently reviewed.

The proof domain must commit to at least:

```text
H(
  "LICHEN_SHIELDED_PROOF_V2",
  chain_id,
  genesis_hash,
  proof_type,
  scheme_id,
  verifier_version
)
```

Every field must be reconstructed by the validator rather than trusted from a
caller-supplied proof object.

The successor note commitment must bind the value, owner/spending-authority
commitment, serial or serial commitment, blinding, note version, and network
domain. It must not reuse the legacy `Poseidon2(value, blinding)` statement as
an ownership commitment.

Required constraints include:

- Shield: exact public amount, 64-bit non-zero value range, commitment opening,
  owner binding, canonical field encodings, and output uniqueness.
- Unshield: note opening, spending-authority derivation, Merkle inclusion,
  anchor policy, nullifier derivation, non-zero/distinct nullifier, exact amount,
  64-bit range, and recipient binding.
- Transfer: two distinct owned inputs, inclusion under the same permitted
  anchor, derived/distinct nullifiers, two valid and distinct output openings,
  non-zero/range policy, and exact checked input/output value conservation.
- All operations: proof-type and network separation, canonical encodings,
  bounded proof/instruction sizes, bounded trace dimensions, deterministic
  verifier work/memory limits, and rejection of trailing or ambiguous data.

The initial anchor policy should require the current canonical pool root at
execution. A historical-root window may be introduced only as a separately
versioned consensus rule with a deterministic retained-root set and replay
tests.

## Legacy Custody Migration Constraint

The 21 observed legacy commitments do not cryptographically bind an owner,
spending key, or serial. A new sound circuit therefore cannot permissionlessly
prove ownership of those legacy notes from `Poseidon2(value, blinding)` alone.
Encrypted wallet payloads are recovery metadata, not consensus authorization.

Consequently:

1. Keep the legacy pool read-only and preserve its complete canonical history.
2. Do not reinterpret a scheme `0x01` proof as a migration authorization.
3. Do not silently map legacy commitments into the successor tree.
4. Define any restitution or claim process as a separately governed,
   source-backed, conflict-aborting migration with a public snapshot, explicit
   claimant evidence rules, conservation accounting, replay protection, and an
   independent review.
5. If no sound ownership evidence can be specified, the legacy balance must
   remain locked or be handled by an explicit governance decision; software
   must not invent ownership.

## Activation Gates

Reactivation requires all of the following in addition to normal release gates:

- a frozen formal statement and wire/domain specification;
- constrained AIR/circuit implementation with no host-only security checks;
- independent cryptographic review and remediation sign-off;
- adversarial proof tests built without the blessed prover;
- malformed, zero, canonical, duplicate, range, conservation, stale-root,
  cross-proof-type, cross-chain, and resource-exhaustion tests;
- deterministic multi-validator replay and shielded-state rebuild parity;
- wallet proof generation that is local or uses an explicitly documented
  custody model, never an implicit validator witness service;
- a reviewed legacy custody decision and activation height/version;
- coordinated consensus deployment with signed rollback artifacts.

Until every gate is satisfied, public surfaces must report the subsystem as
`disabled_insecure_verifier` and no release may describe it as active privacy.
