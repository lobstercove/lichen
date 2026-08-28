# LichenAuction Accounting V3 Migration

Accounting V3 replaces implicit auction custody with exact ledgers for active
bids, active offers, unpaid payouts, and withdrawable platform fees. It also
seals immutable payment-token and royalty terms for every legacy row. This is a
governed protocol migration; keep LichenAuction paused until every verification
and fixed-tip parity gate passes.

## Invariants

- Manifest capture requires version 2, `paused=true`,
  `migration_locked=true`, `manifest_sealed=false`, and zero migrated auction
  and offer counters.
- The contract-reported escrow identity must be the contract program address.
  If any liability remains attributed to another legacy escrow, abort for an
  explicit source-backed custody recovery; never rewrite the manifest.
- Every auction, offer, unpaid-payout, and platform-fee row is structurally
  validated and hash-bound. Active bid/offer amounts and ledger liabilities are
  independently rederived with checked arithmetic.
- The manifest binds chain ID, source slot, contract, canonical storage hash,
  row counts, liability totals, immutable royalty terms, and its own SHA-256.
- Migration is row-keyed, simulation-first, resumable from durable receipts,
  and conflict-aborting. Completion requires exact on-chain counters and leaves
  the contract paused for post-state verification.

## Required evidence

Preserve the signed release and rollback artifacts, deployed contract hash,
chain ID, fixed source slot and archive manifest, pre/post LichenAuction status,
the source manifest and SHA-256, governed payloads, durable receipts, custody
and liability totals, and four-validator state/public-history parity. Never put
key material or keypair passwords in evidence or command-line arguments.

Use placeholders below. Resolve the actual RPC, contract, authority, and
operator paths only inside the approved execution environment.

## Procedure

1. Install only the signed candidate that passed hosted and clean-build
   four-validator gates. At one fixed canonical tip, preserve source-backed
   state/archive evidence and verify the deployed LichenAuction code hash.
2. Pause LichenAuction through the approved governance path. Generate the
   governed begin payload:

   ```sh
   cargo run -p lichen-cli --locked --bin lichenauction_v3_migrate -- \
     begin-args --authority AUTHORITY
   ```

3. Execute `begin_v3_migration` through governance. Require version 2,
   `paused=true`, `migration_locked=true`, `manifest_sealed=false`, and zero
   migrated counters.
4. Capture the exact source manifest while the contract remains frozen:

   ```sh
   cargo run -p lichen-cli --locked --bin lichenauction_v3_migrate -- \
     --rpc-url RPC manifest --contract AUCTION --output lichenauction-v3.json
   ```

   Independently review its chain, slot, storage hash, escrow identities, row
   counts, active-bid/offer liabilities, unpaid payouts, platform fees, and
   royalty snapshots.
5. Generate and execute the governed manifest seal:

   ```sh
   cargo run -p lichen-cli --locked --bin lichenauction_v3_migrate -- \
     seal-args --authority AUTHORITY --manifest lichenauction-v3.json
   ```

   Require the on-chain manifest hash and expected auction/offer counts to
   match the reviewed file exactly.
6. Simulate every row without `--execute`:

   ```sh
   cargo run -p lichen-cli --locked --bin lichenauction_v3_migrate -- \
     --rpc-url RPC migrate --contract AUCTION --manifest lichenauction-v3.json \
     --keypair OPERATOR --receipts lichenauction-v3-receipts.json
   ```

   Stop on any simulation, source-row, signer-authority, or seal mismatch.
7. Execute the same sealed rows with the authorized migration signer. The
   receipt file is atomically updated after confirmed rows and may be reused to
   resume safely:

   ```sh
   cargo run -p lichen-cli --locked --bin lichenauction_v3_migrate -- \
     --rpc-url RPC migrate --contract AUCTION --manifest lichenauction-v3.json \
     --keypair OPERATOR --receipts lichenauction-v3-receipts.json --execute
   ```

8. Generate and execute the governed completion payload:

   ```sh
   cargo run -p lichen-cli --locked --bin lichenauction_v3_migrate -- \
     complete-args --authority AUTHORITY
   ```

9. Verify the completed migration while the contract remains paused:

   ```sh
   cargo run -p lichen-cli --locked --bin lichenauction_v3_migrate -- \
     --rpc-url RPC verify --contract AUCTION --manifest lichenauction-v3.json
   ```

   Require version 3, unlocked sealed status, exact counters, escrow identity,
   row hashes, royalty terms, liabilities, and custody. Then repeat fixed-tip
   four-validator state-root and genesis-to-tip public-history parity and run
   native/MT-20 auction, offer, settlement, failed-transfer, payout, and fee
   withdrawal E2E checks. Unpause only through a separate governed action after
   all evidence is accepted.

## Abort conditions

Abort without hand-editing state if validators disagree, the source tip or
storage changes after capture, a row is malformed, any identity/count/hash or
royalty term differs, custody is assigned to a non-contract escrow, arithmetic
overflows, simulation or confirmation fails, receipts conflict, counters skip
or stall, a liability is undercollateralized, or post-migration parity fails.
Keep the contract paused and preserve all evidence for reconciliation.
