# MoltChain Comprehensive File-by-File Audit

**Scope**: All Rust source files across `validator/`, `p2p/`, `rpc/`, `cli/`, `compiler/`, `custody/`, `faucet/` crates  
**Files Audited**: 40+ source files (~38,000 lines of Rust)  
**Categories**: Stubs/placeholders, security, atomicity, performance, dead code, missing features, error handling, wiring issues, naming, network security

---

## CRITICAL (5 findings)

### C-1 · Prediction market `post_create` bypasses consensus
- **File**: [rpc/src/prediction.rs](rpc/src/prediction.rs#L890)
- **Category**: Security / Consensus bypass
- **Description**: `post_create` writes directly to `CF_CONTRACT_STORAGE` via `state.state.put_contract_storage(...)` instead of going through a signed transaction processed by the block production pipeline. In a multi-validator network, this state mutation only applies to the RPC-serving node, causing state root divergence and consensus failure. Other prediction market write endpoints (`post_buy`, `post_sell`, `post_resolve`) have the same pattern.

### C-2 · FROST multi-signer custody not production-ready
- **File**: [custody/src/main.rs](custody/src/main.rs#L395)
- **Category**: Security / Missing feature
- **Description**: The startup warning `R-C3: "FROST 2-round signing NOT fully production-tested for n>1"` is accurate. The `collect_frost_signatures` function (line ~2700) implements FROST commit → sign rounds, but the sweep and withdrawal worker pipelines use `collect_signatures` (single-round, line ~2800) which sends to `/sign` not `/frost/commit` + `/frost/sign`. The FROST 2-round protocol is only used in `assemble_signed_solana_tx` for aggregation AFTER signatures are already collected via the single-round path. Multi-signer deployments with >1 signer will produce invalid threshold signatures.

### C-3 · EVM compatibility: gasPrice × estimateGas = fee² (not fee)
- **File**: [rpc/src/lib.rs](rpc/src/lib.rs#L7690)
- **Category**: Wiring / EVM compatibility
- **Description**: `eth_gasPrice` (line ~7690) returns `base_fee` and `eth_estimateGas` (line ~7660) also returns `base_fee`. MetaMask and other EVM wallets compute `total_cost = gasPrice × gas = base_fee × base_fee = base_fee²`. For a `base_fee` of 100,000 shells, wallets display 10,000,000,000 shells (10 MOLT) instead of the actual 100,000 shells (0.0001 MOLT). The docstring on `eth_gasPrice` says "gasPrice is 1" but the implementation returns `base_fee`. Fix: return `1` from `eth_gasPrice` or return `1` from `eth_estimateGas` and return `base_fee` from the other.

### C-4 · EVM eth_getLogs uses SHA-256 instead of Keccak-256 for topic hashes
- **File**: [rpc/src/lib.rs](rpc/src/lib.rs#L8095)
- **Category**: EVM compatibility
- **Description**: The `eth_getLogs` handler computes event topic[0] using `sha2::Sha256` instead of `sha3::Keccak256`. EVM standard requires `keccak256("EventName(type1,type2)")` for indexed event signatures. Tools like Ethers.js, web3.js, and The Graph filter logs by keccak256 topic hashes — they will never match MoltChain's SHA-256 hashes, making the entire event log system invisible to standard EVM tooling.

### C-5 · Direct-state-write RPC endpoints in multi-validator mode
- **File**: [rpc/src/lib.rs](rpc/src/lib.rs#L5045) (deployContract), [rpc/src/lib.rs](rpc/src/lib.rs#L5310) (upgradeContract)
- **Category**: Consensus bypass
- **Description**: `handle_deploy_contract` and `handle_upgrade_contract` debit deployer/owner, credit treasury, and store contracts directly in the state store, bypassing block production and consensus. They are gated by `require_single_validator` (H16 fix), which correctly prevents use in multi-validator mode. However, `handle_request_airdrop` (line ~9120) performs the same direct state mutations (treasury debit + recipient credit) WITHOUT the `require_single_validator` guard. The testnet-only guard prevents mainnet use, but on a multi-validator testnet the state would diverge. Mitigated by testnet-only scope but architecturally unsound.

---

## HIGH (11 findings)

### H-1 · Oracle price feeder writes to contract storage directly
- **File**: [validator/src/main.rs](validator/src/main.rs#L3500)
- **Category**: Consensus bypass
- **Description**: The background oracle price feeder task writes candle data directly to `dex_core`, `dex_analytics`, and `moltoracle` contract storage via `state.put_contract_storage()`. These writes occur outside of block processing, so in a multi-validator network, only the validator running the oracle task has this state. The resulting state root divergence causes consensus failure. All oracle price writes should be encapsulated in system transactions included in blocks.

### H-2 · Threshold signer auth token predictable when env var not set
- **File**: [validator/src/threshold_signer.rs](validator/src/threshold_signer.rs#L50)
- **Category**: Security
- **Description**: When `SIGNER_AUTH_TOKEN` is not set, the fallback auth token is `SHA256(pubkey_bytes + pid_bytes)`. The public key is broadcast via P2P announcements and the PID is typically predictable (small integer, often guessable). An attacker who knows the validator's public key can brute-force PIDs to derive the auth token and submit unauthorized signing requests.

### H-3 · Compiler service: no sandboxing for code compilation
- **File**: [compiler/src/main.rs](compiler/src/main.rs#L200)
- **Category**: Security
- **Description**: The compiler service spawns `cargo build`, `clang`, and `asc` subprocesses with no sandboxing (no seccomp, no namespace isolation, no resource limits beyond a 120-second timeout). A malicious Rust crate with a `build.rs` script can execute arbitrary code on the compiler host. The compiler runs as a network-accessible HTTP service, making this a remote code execution vector.

### H-4 · All custody deposit keys derived from single master seed
- **File**: [custody/src/main.rs](custody/src/main.rs#L850)
- **Category**: Security / Key management
- **Description**: All Solana and EVM deposit addresses are derived via `HMAC-SHA256(master_seed, path)`. Compromise of the master seed (stored as a file or environment variable) exposes every deposit private key simultaneously. The `M2` fix prefers file-based storage and clears the env var after reading, but there's no HSM integration and no key rotation mechanism. A single breach compromises all funds across all deposit addresses.

### H-5 · Snapshot response can drain treasury via fake stake entries
- **File**: [validator/src/main.rs](validator/src/main.rs#L8100)
- **Category**: Security / Economic
- **Description**: The snapshot response handler merges remote validators from peer-provided data. For unknown validators with on-chain stake > 0, it creates treasury-funded bootstrap accounts (AUDIT-FIX 2.11 verifies on-chain stake first). However, a malicious peer could broadcast a snapshot with many synthetic validator entries that happen to have small on-chain stakes, causing the receiving validator to create numerous treasury-funded bootstrap grants. The per-epoch cap (10 bootstraps via `1.5c`) mitigates this but doesn't prevent slow drain.

### H-6 · EVM rebalance: approve + swap without confirmation wait
- **File**: [custody/src/main.rs](custody/src/main.rs#L5400)
- **Category**: Atomicity / Race condition
- **Description**: `execute_ethereum_rebalance_swap` sends an ERC-20 `approve` transaction (nonce N) immediately followed by a Uniswap `exactInputSingle` transaction (nonce N+1) without waiting for the approve to confirm. If the approve is not mined before the swap, the swap will revert due to insufficient allowance. The retry logic will re-attempt but with the same nonce, which may also fail if the approve is still pending.

### H-7 · Withdrawal burn verification depends on RPC response format
- **File**: [custody/src/main.rs](custody/src/main.rs#L5580)
- **Category**: Security / Fragility
- **Description**: The AUDIT-FIX R-C1 burn verification checks `result.get("contract_address")`, `result.get("caller")`, `result.get("method")`, and `result.get("amount")` from MoltChain's `getTransaction` RPC response. If the RPC response format changes or these fields are renamed, burn verification silently passes (it `continue`s, not errors, for any mismatch, but missing fields cause none of the validation checks to fire). The absence of a `contract_address` field would skip ALL verification. Should explicitly fail if expected fields are missing.

### H-8 · P2P block receiver accepts blocks from any validator in the set
- **File**: [validator/src/main.rs](validator/src/main.rs#L6500)
- **Category**: Network security
- **Description**: The block receiver validates that the block's validator is in the validator set (T2.2, C5), but does not verify that the block was produced by the current slot leader. Any validator in the set can broadcast a block for any slot. The fork choice mechanism eventually resolves conflicts, but this allows a malicious validator to fill the mempool with competing blocks for slots it doesn't own, wasting bandwidth and storage.

### H-9 · Self-signed TLS for P2P QUIC transport
- **File**: [p2p/src/network.rs](p2p/src/network.rs#L50)
- **Category**: Network security
- **Description**: P2P connections use self-signed TLS certificates with `danger_accept_invalid_certs` and `danger_accept_invalid_hostnames`. This provides encryption but no authentication — any node can impersonate any other node. A man-in-the-middle can intercept P2P traffic including votes, blocks, and transactions. Nodes should authenticate peers using their Ed25519 validator identity keys.

### H-10 · LazyLock BURN_LOCKS map uses std::sync::Mutex inside async context
- **File**: [custody/src/main.rs](custody/src/main.rs#L4260)
- **Category**: Performance / Correctness
- **Description**: The `BURN_LOCKS` static map uses `std::sync::Mutex` (not `tokio::sync::Mutex`) to guard insertion/lookup. While the inner per-job lock is correctly a `tokio::sync::Mutex`, the outer map lock is a blocking `std::sync::Mutex` used inside an async handler (`submit_burn_signature`). Under contention, this blocks the Tokio worker thread. The F8.7 pruning (retain entries with `strong_count > 1`) mitigates unbounded growth but the blocking lock remains.

### H-11 · MoltyID profile/vouches endpoint scans entire contract storage
- **File**: [rpc/src/lib.rs](rpc/src/lib.rs#L6200)
- **Category**: Performance / DoS
- **Description**: `handle_get_moltyid_vouches` and `handle_get_moltyid_profile` iterate over the entire contract storage BTreeMap to find "given" vouches (checking every `id:*` entry, then checking all vouch records for each identity). With N identities having M vouches each, this is O(N×M) per request. A single API call can consume significant CPU. No rate limiting is specific to these expensive endpoints beyond the global rate limiter.

---

## MEDIUM (25 findings)

### M-1 · Monolithic file sizes
- **Files**: [validator/src/main.rs](validator/src/main.rs) (9100 lines), [rpc/src/lib.rs](rpc/src/lib.rs) (9676 lines), [custody/src/main.rs](custody/src/main.rs) (7455 lines)
- **Category**: Maintainability
- **Description**: Three files exceed 7,000 lines each. These should be split into modules (e.g., `rpc/src/moltyid.rs`, `rpc/src/evm_compat.rs`, `rpc/src/prediction.rs` already exists but `lib.rs` still has prediction handlers at line 9300+, `custody/src/withdrawal.rs`, `custody/src/rebalance.rs`, `validator/src/consensus.rs`, `validator/src/genesis.rs`).

### M-2 · `let _ = put_contract_storage(...)` silently ignores write errors
- **File**: [validator/src/main.rs](validator/src/main.rs#L2800) and many other locations
- **Category**: Error handling
- **Description**: Throughout the oracle seeding, genesis auto-deploy, and oracle price feeder code, contract storage writes use `let _ = state.put_contract_storage(...)` which discards errors. A failed write (e.g., RocksDB disk full) would silently drop state without any error propagation. At least 20 instances across validator and RPC code.

### M-3 · CLI governance uses hardcoded DAO marker address
- **File**: [cli/src/main.rs](cli/src/main.rs#L1700)
- **Category**: Wiring
- **Description**: The CLI governance commands use `Pubkey([0xDA; 32])` as a hardcoded DAO marker address for proposal creation and voting. This address is not derived from any configuration and would need to match the on-chain governance contract's expected address exactly. If the governance contract uses a different address, all CLI governance operations silently fail.

### M-4 · DEX orderbook handler scans up to 10,000 orders
- **File**: [rpc/src/dex.rs](rpc/src/dex.rs#L830)
- **Category**: Performance
- **Description**: The `get_orderbook` handler iterates up to 10,000 orders from contract storage, deserializes each, filters by pair and status, and sorts into bids/asks. With many active pairs, this is O(10K) per request regardless of the requested depth.

### M-5 · Faucet CORS wildcard on money-dispensing endpoint
- **File**: [faucet/src/main.rs](faucet/src/main.rs#L225)
- **Category**: Security
- **Description**: The faucet service uses `CorsLayer::permissive()` which sets `Access-Control-Allow-Origin: *`. While testnet-only, this allows any webpage to trigger airdrop requests on behalf of users visiting the page, enabling CSRF-style attacks that drain the faucet.

### M-6 · CLOB quote scans up to 10,000 orders
- **File**: [rpc/src/dex.rs](rpc/src/dex.rs#L1800)
- **Category**: Performance
- **Description**: `quote_clob_swap` walks the order book scanning up to 10,000 orders to compute a fill quote. This is a read-only endpoint but the O(10K) scan can be used for DoS amplification.

### M-7 · `handle_set_contract_abi` writes directly to state
- **File**: [rpc/src/lib.rs](rpc/src/lib.rs#L4800)
- **Category**: Consensus bypass (mitigated)
- **Description**: `handle_set_contract_abi` modifies contract account data directly. Mitigated by `require_single_validator` (H16) and admin auth, but the pattern is architecturally wrong — ABI updates should go through consensus.

### M-8 · MoltyID agent directory scans entire contract storage
- **File**: [rpc/src/lib.rs](rpc/src/lib.rs#L6850)
- **Category**: Performance
- **Description**: `handle_get_moltyid_agent_directory` iterates all `id:*` entries in the MoltyID contract storage, loads availability and rate for each, then sorts by reputation. With many registered identities, this is O(N) per request with significant per-item overhead.

### M-9 · MoltyID stats scans entire contract storage
- **File**: [rpc/src/lib.rs](rpc/src/lib.rs#L7010)
- **Category**: Performance
- **Description**: `handle_get_moltyid_stats` iterates all `id:*` entries to compute tier distribution. Should use pre-computed counters stored in the contract.

### M-10 · MoltyID name search scans entire contract storage
- **File**: [rpc/src/lib.rs](rpc/src/lib.rs#L6630)
- **Category**: Performance
- **Description**: `handle_search_molt_names` iterates all `name:*` entries for prefix matching. With many registered names, this becomes slow. Should use a prefix index or trie.

### M-11 · Airdrop handler writes to state without single-validator guard
- **File**: [rpc/src/lib.rs](rpc/src/lib.rs#L9120)
- **Category**: Consensus bypass (mitigated)
- **Description**: `handle_request_airdrop` debits treasury and credits recipient directly in the state store. Unlike `deployContract`/`upgradeContract`, it does NOT use `require_single_validator`. Protected by a testnet-only guard (`network_id` must contain "testnet", "devnet", or "local"), but on a multi-validator testnet, this causes state root divergence.

### M-12 · EVM event log data encoding is non-standard
- **File**: [rpc/src/lib.rs](rpc/src/lib.rs#L8120)
- **Category**: EVM compatibility
- **Description**: In `eth_getLogs`, event data is encoded as concatenated `value.as_bytes()` from a `BTreeMap<String, String>`. Standard EVM encodes event data as ABI-encoded `bytes32` slots. EVM tools (Ethers, web3.js) expecting ABI-encoded data will fail to decode these logs.

### M-13 · EVM block formatting uses state_root as block hash
- **File**: [rpc/src/lib.rs](rpc/src/lib.rs#L7560)
- **Category**: EVM compatibility
- **Description**: `format_evm_block` uses `block.header.state_root` as the `hash` field. EVM block hashes are typically `keccak256(RLP(header))`. Using state_root means the block hash changes if state is recomputed, and tools that compare block hashes across nodes will see mismatches.

### M-14 · EVM transaction formatting truncates pubkeys to last 20 bytes
- **File**: [rpc/src/lib.rs](rpc/src/lib.rs#L7575)
- **Category**: EVM compatibility
- **Description**: `format_evm_block` converts native 32-byte pubkeys to EVM 20-byte addresses by taking bytes `[12..32]`. This truncation can cause address collisions (different native pubkeys mapping to the same EVM address) and breaks the round-trip from EVM address back to native pubkey.

### M-15 · Custody rebalance uses hardcoded chains
- **File**: [custody/src/main.rs](custody/src/main.rs#L5150)
- **Category**: Extensibility
- **Description**: `check_rebalance_thresholds` iterates over `["solana", "ethereum"]` hardcoded. Adding a new chain requires code change rather than configuration.

### M-16 · P2P message size limit is 16 MB
- **File**: [p2p/src/network.rs](p2p/src/network.rs#L30)
- **Category**: Network security
- **Description**: The `MAX_MESSAGE_SIZE` of 16 MB allows a single peer to send very large messages that consume memory. While bincode deserialization has some bounds, a malicious peer could send a block with many large transactions close to 16 MB, consuming significant memory during parsing.

### M-17 · Gossip cache is unbounded HashMap
- **File**: [p2p/src/gossip.rs](p2p/src/gossip.rs#L25)
- **Category**: Memory
- **Description**: The gossip `seen` cache is a `HashMap<Hash, Instant>` with periodic pruning (every 60 seconds, removes entries older than 5 minutes). Between pruning cycles, under high transaction load, this map can grow unboundedly. Should use an LRU cache with a size cap.

### M-18 · P2P peer scoring uses simple u32 counter
- **File**: [p2p/src/peer.rs](p2p/src/peer.rs#L15)
- **Category**: Network security
- **Description**: Peer reputation is a simple `score: u32` incremented/decremented. There's no decay over time, no weighted scoring by violation type, and no minimum score threshold for automatic disconnection (banning is separate). A peer with a high score can commit many violations before being banned.

### M-19 · Supervisor restart loop has no jitter
- **File**: [validator/src/main.rs](validator/src/main.rs#L4200)
- **Category**: Reliability
- **Description**: The supervisor's exponential backoff (2s, 4s, 8s, 16s, 30s cap) does not include jitter. If multiple validators crash simultaneously (e.g., due to a network partition), they all restart at the same intervals, causing thundering herd reconnections.

### M-20 · Fork choice holds three locks simultaneously
- **File**: [validator/src/main.rs](validator/src/main.rs#L1200)
- **Category**: Performance
- **Description**: `run_fork_choice` acquires `last_finalized_slot`, `finality_tracker`, and `vote_aggregator` locks in a single scope (PERF-OPT 6). While correct for consistency, this creates a wide critical section that blocks all vote processing and finality checking during fork choice evaluation.

### M-21 · Genesis auto-deploys 27 contracts without error aggregation
- **File**: [validator/src/main.rs](validator/src/main.rs#L2500)
- **Category**: Error handling
- **Description**: Genesis initialization deploys 27 contracts in dependency order (treasury, system programs, DEX core, AMM, etc.). Each deployment failure is logged with `error!()` but execution continues. A failed deployment leaves the chain in a partially initialized state with broken cross-contract dependencies. Should collect all errors and abort genesis if critical contracts fail.

### M-22 · WebSocket broadcast channels have fixed capacity
- **File**: [validator/src/main.rs](validator/src/main.rs#L7800)
- **Category**: Performance
- **Description**: WebSocket broadcast channels use `broadcast::channel(1000)`. When subscribers are slow, lagged events are dropped. Under high block production rates, slow explorer WebSocket connections miss blocks with no retry mechanism. The custody WebSocket has the same pattern.

### M-23 · DashMap iteration in P2P code
- **File**: [p2p/src/peer_store.rs](p2p/src/peer_store.rs#L40)
- **Category**: Performance
- **Description**: Several P2P functions iterate the entire `DashMap<PeerId, PeerInfo>` (e.g., `get_peers`, `cleanup`, peer selection). DashMap iteration acquires one shard lock at a time but under high peer counts this can become slow and block peer operations.

### M-24 · CLI wallet removal permanently deletes keypair file
- **File**: [cli/src/wallet.rs](cli/src/wallet.rs#L175)
- **Category**: UX / Safety
- **Description**: `wallet remove` calls `std::fs::remove_file()` on the keypair file with no backup, no confirmation prompt, and no recovery mechanism. If the user accidentally removes their validator keypair, they lose their identity and staked funds.

### M-25 · Custody deposit cleanup scans all deposit events
- **File**: [custody/src/main.rs](custody/src/main.rs#L5010)
- **Category**: Performance
- **Description**: The deposit cleanup loop, when pruning event entries for expired deposits, performs a full scan of `CF_DEPOSIT_EVENTS` (line ~5045: `iterator_cf(cf, IteratorMode::Start)`) because event entries are keyed by `event_id` not `deposit_id`. With many events, this becomes slow. Should maintain a secondary index of events by deposit_id.

---

## LOW (20 findings)

### L-1 · CLI token balance creates throwaway Keypair
- **File**: [cli/src/main.rs](cli/src/main.rs#L1580)
- **Category**: Dead code
- **Description**: The `balances` command creates `Keypair::new()` then discards it. This is leftover from an earlier implementation that needed a keypair for signing.

### L-2 · `jitter_duration` uses nanosecond-based pseudo-random
- **File**: [validator/src/updater.rs](validator/src/updater.rs#L120)
- **Category**: Quality
- **Description**: `jitter_duration` computes jitter from `SystemTime::now().duration_since(UNIX_EPOCH).subsec_nanos() % max_ms`. This is not cryptographically random but adequate for the jitter use case (preventing update check thundering herd).

### L-3 · Compiler `wait_with_timeout` uses busy-wait polling
- **File**: [compiler/src/main.rs](compiler/src/main.rs#L950)
- **Category**: Performance
- **Description**: The timeout mechanism polls child process status in a loop with `thread::sleep(100ms)` between checks. Should use `tokio::time::timeout` with an async child process wait.

### L-4 · Compiler uses `CorsLayer::permissive()`
- **File**: [compiler/src/main.rs](compiler/src/main.rs#L91)
- **Category**: Security
- **Description**: The compiler service uses permissive CORS. While internal-only, this allows any web origin to submit compilation requests.

### L-5 · `read_leb128` returns `(0, 0)` for empty data
- **File**: [compiler/src/main.rs](compiler/src/main.rs#L280)
- **Category**: Error handling
- **Description**: `read_leb128` returns `(value=0, bytes_consumed=0)` for empty input. Callers don't distinguish between "value is 0" and "no data was available", which could cause infinite loops if a caller uses `bytes_consumed` to advance a cursor.

### L-6 · EVM transaction wrapping uses sentinel values
- **File**: [rpc/src/lib.rs](rpc/src/lib.rs#L7490)
- **Category**: Naming / Clarity
- **Description**: `handle_eth_send_raw_transaction` uses `Hash([0xEE; 32])` as a sentinel blockhash and `[0u8; 64]` as a placeholder signature for EVM-wrapped transactions (AUDIT-FIX 2.15). These magic values should be named constants with documentation explaining the sentinel protocol.

### L-7 · P2P ban list uses in-memory `HashMap` only
- **File**: [p2p/src/peer_ban.rs](p2p/src/peer_ban.rs#L10)
- **Category**: Durability
- **Description**: The P2P ban list is an in-memory `HashMap<PeerId, BanEntry>`. Banned peers are allowed to reconnect after a node restart. The validator-level slashing tracker IS persisted (M7), but P2P-level bans (for protocol violations like oversized messages) are lost.

### L-8 · Sync module `request_blocks_from_peer` doesn't verify peer identity
- **File**: [validator/src/sync.rs](validator/src/sync.rs#L30)
- **Category**: Network security
- **Description**: Block sync requests are sent to peers identified by `SocketAddr` only. The response is not authenticated against the peer's validator identity, so a MITM on the P2P connection could serve fabricated blocks during initial sync. Blocks are validated after receipt (signature + validator set), so this is mitigated but adds unnecessary attack surface.

### L-9 · CLI config uses `~/.moltchain/config.toml` hardcoded
- **File**: [cli/src/config.rs](cli/src/config.rs#L10)
- **Category**: Flexibility
- **Description**: The CLI configuration path is hardcoded to `~/.moltchain/config.toml` with no `--config` flag override. Users running multiple environments (testnet + devnet) must manually swap config files.

### L-10 · Keypair encryption uses PBKDF2 with 100K iterations
- **File**: [cli/src/keypair_manager.rs](cli/src/keypair_manager.rs#L80)
- **Category**: Security (low risk)
- **Description**: PBKDF2 with 100K iterations is below OWASP's 2023 recommendation of 600K iterations for SHA-256. The current setting provides adequate protection for interactive use but could be brute-forced on GPU hardware.

### L-11 · Keygen outputs secret key to stdout
- **File**: [cli/src/keygen.rs](cli/src/keygen.rs#L30)
- **Category**: Security
- **Description**: The `keygen` command prints the secret key hex to stdout. If terminal scrollback or logging captures this output, the key is exposed. Should write the key directly to a file.

### L-12 · Faucet `requestAirdrop` RPC delegation has no retry
- **File**: [faucet/src/main.rs](faucet/src/main.rs#L150)
- **Category**: Reliability
- **Description**: The faucet forwards airdrop requests to the validator RPC with a single HTTP call. If the RPC is temporarily unavailable (e.g., during block production), the request fails with no retry.

### L-13 · DEX router `post_router_swap` is read-only despite POST method
- **File**: [rpc/src/dex.rs](rpc/src/dex.rs#L1900)
- **Category**: Naming / API design
- **Description**: Both `post_router_swap` and `post_router_quote` perform identical read-only operations (quote computation). The F14 fix removed WebSocket event emission from swap, making it functionally identical to quote. The POST method with "swap" name implies a state mutation but none occurs. Should be GET.

### L-14 · DEX margin funding rate uses hardcoded tier table
- **File**: [rpc/src/dex.rs](rpc/src/dex.rs#L2100)
- **Category**: Wiring
- **Description**: `get_margin_funding_rate` returns a constant funding rate from a hardcoded table based on pair name pattern matching (contains "MOLT", "SOL", "ETH"). Dynamic funding rates based on open interest imbalance are not implemented.

### L-15 · `_build_contract_burn_instruction` is unused
- **File**: [custody/src/main.rs](custody/src/main.rs#L3420)
- **Category**: Dead code
- **Description**: `_build_contract_burn_instruction` is defined but never called (prefixed with `_` to suppress warnings). The withdrawal flow expects the user to burn tokens client-side, making this function unnecessary. Should be removed or documented as a future utility.

### L-16 · `_build_system_transfer` is unused
- **File**: [custody/src/main.rs](custody/src/main.rs#L3450)
- **Category**: Dead code
- **Description**: `_build_system_transfer` builds a MoltChain system transfer instruction but is never called. Prefixed with `_`.

### L-17 · Vote aggregator prune interval is 30 seconds keeping 100 slots
- **File**: [validator/src/main.rs](validator/src/main.rs#L8500)
- **Category**: Performance
- **Description**: The vote aggregator is pruned every 30 seconds, keeping the most recent 100 slots. With 400ms slot times, 100 slots is 40 seconds of history. During network partitions where slots accumulate slowly, the 100-slot window may be insufficient for finality resolution.

### L-18 · Block range request handler uses `HashMap` for rate limiting
- **File**: [validator/src/main.rs](validator/src/main.rs#L7001)
- **Category**: Performance
- **Description**: Block range request rate limiting uses a `HashMap<SocketAddr, (u64, Instant)>` with entries pruned every 60 seconds (M5). Between pruning cycles, if many unique peers request blocks, the map grows unboundedly. Should use an LRU map or cap entry count.

### L-19 · Compiler export extraction regex doesn't validate WASM format
- **File**: [compiler/src/main.rs](compiler/src/main.rs#L300)
- **Category**: Error handling
- **Description**: `extract_wasm_exports` parses the custom name section using manual byte offset arithmetic. If the WASM binary has a malformed custom section, the function silently returns an empty export list instead of reporting an error.

### L-20 · Custody `find_program_address` uses SHA-256 instead of Ed25519 curve
- **File**: [custody/src/main.rs](custody/src/main.rs#L3900)
- **Category**: Compatibility
- **Description**: `find_program_address` uses `SHA-256` hash to derive PDAs, then checks if the result falls on the Ed25519 curve by attempting `VerifyingKey::from_bytes()`. Solana's actual PDA derivation uses a different algorithm (SHA-256 with "ProgramDerivedAddress" suffix, which this code does include). However, the hash input ordering may differ from Solana's implementation, causing PDA mismatches when interacting with standard Solana programs.

---

## GOOD PATTERNS (50+ observed)

| # | Pattern | Location |
|---|---------|----------|
| 1 | Fee distribution uses atomic `WriteBatch` (AUDIT-FIX 0.6) | [validator/main.rs](validator/src/main.rs#L2000) |
| 2 | Keypair encryption: AES-256-GCM with 100K-iteration PBKDF2 | [cli/keypair_manager.rs](cli/src/keypair_manager.rs#L80) |
| 3 | Supervisor: backoff (2s→30s), restart limits, health-based reset | [validator/main.rs](validator/src/main.rs#L4200) |
| 4 | P2P: bounded channels, ban lists, per-IP scoring, 16MB limit | [p2p/network.rs](p2p/src/network.rs) |
| 5 | Block validation: signature + structure + validator set (T2.2, C5) | [validator/main.rs](validator/src/main.rs#L6500) |
| 6 | Fork choice acquires all 3 locks in single scope (PERF-OPT 6) | [validator/main.rs](validator/src/main.rs#L1200) |
| 7 | Bootstrap grants from treasury, not ex nihilo (H13) | [validator/main.rs](validator/src/main.rs#L5700) |
| 8 | Per-epoch bootstrap cap + announcement re-verification (1.5a-d) | [validator/main.rs](validator/src/main.rs#L5800) |
| 9 | CORS explicit host allowlist (AUDIT-FIX 2.14, H14) | [rpc/lib.rs](rpc/src/lib.rs#L600) |
| 10 | Admin token hot-reload from env var (30s) | [rpc/lib.rs](rpc/src/lib.rs#L650) |
| 11 | Transaction fallback scan capped at 1000 slots (M20) | [rpc/lib.rs](rpc/src/lib.rs#L1800) |
| 12 | Slashing tracker persisted to disk (AUDIT-FIX M7) | [validator/main.rs](validator/src/main.rs#L5400) |
| 13 | P2P transaction validation includes sig + balance + structure (1.6) | [validator/main.rs](validator/src/main.rs#L6700) |
| 14 | `molt_to_shells` has saturating arithmetic with comprehensive tests | core crate |
| 15 | AUDIT-FIX 2.11: snapshot validator merge verifies on-chain stake | [validator/main.rs](validator/src/main.rs#L8100) |
| 16 | AUDIT-FIX C2: credit job ONLY after sweep confirmation | [custody/main.rs](custody/src/main.rs#L2400) |
| 17 | AUDIT-FIX M4: write-ahead intent log for crash idempotency | [custody/main.rs](custody/src/main.rs#L400) |
| 18 | AUDIT-FIX M1: status index for O(active) queries | [custody/main.rs](custody/src/main.rs#L380) |
| 19 | AUDIT-FIX C1: Solana tx fee deducted from sweep amount | [custody/main.rs](custody/src/main.rs#L2500) |
| 20 | AUDIT-FIX M6: dynamic gas estimation with 20% buffer fallback | [custody/main.rs](custody/src/main.rs#L1900) |
| 21 | M16: gas funding from treasury for EVM token sweeps | [custody/main.rs](custody/src/main.rs#L2900) |
| 22 | FIX-FORK-1: double-check slot availability before production | [validator/main.rs](validator/src/main.rs#L8900) |
| 23 | Watchdog with configurable timeout + EXIT_CODE_RESTART | [validator/main.rs](validator/src/main.rs#L8600) |
| 24 | H15: Solana TX cache AFTER submit not before | [rpc/lib.rs](rpc/src/lib.rs#L3300) |
| 25 | Pre-mempool validation: empty sigs, zero sigs, empty ixs, balance+fee | [rpc/lib.rs](rpc/src/lib.rs#L2700) |
| 26 | Preflight simulation for contract calls (skippable) | [rpc/lib.rs](rpc/src/lib.rs#L2800) |
| 27 | C5: bounded sync update (500 slots ahead max) | [validator/main.rs](validator/src/main.rs#L7200) |
| 28 | Block range request: rate limiting, size cap, ban escalation | [validator/main.rs](validator/src/main.rs#L7001) |
| 29 | Leader election cache (PERF-OPT 5) | [validator/main.rs](validator/src/main.rs#L8800) |
| 30 | Event-driven wakeup with Arc<Notify> (PERF-OPT 1) | [validator/main.rs](validator/src/main.rs#L8700) |
| 31 | R-C1: burn verification validates contract + method + amount + caller | [custody/main.rs](custody/src/main.rs#L5580) |
| 32 | Constant-time admin token comparison (AUDIT-FIX 0.12) | [rpc/lib.rs](rpc/src/lib.rs#L200) |
| 33 | Withdrawal rate limiting: per-minute count, per-hour value, per-address | [custody/main.rs](custody/src/main.rs#L4000) |
| 34 | Deposit rate limiting: per-minute, per-user 10s cooldown (W-H4) | [custody/main.rs](custody/src/main.rs#L800) |
| 35 | HMAC-SHA256 deposit address derivation (C8 fix) | [custody/main.rs](custody/src/main.rs#L3800) |
| 36 | Webhook HMAC-SHA256 signed payloads for delivery verification | [custody/main.rs](custody/src/main.rs#L7300) |
| 37 | Webhook delivery: 3 retries with exponential backoff (1s, 2s, 4s) | [custody/main.rs](custody/src/main.rs#L7250) |
| 38 | F8.1: constant-time API auth for custody endpoints | [custody/main.rs](custody/src/main.rs#L7370) |
| 39 | F8.2: constant-time WebSocket auth token comparison | [custody/main.rs](custody/src/main.rs#L7080) |
| 40 | F8.7: BURN_LOCKS map pruning when exceeding 10K entries | [custody/main.rs](custody/src/main.rs#L4270) |
| 41 | F8.8: destination address format validation before processing | [custody/main.rs](custody/src/main.rs#L4100) |
| 42 | F8.10: deposit cleanup uses status index instead of full-table scan | [custody/main.rs](custody/src/main.rs#L4900) |
| 43 | H2: max retry cap (10 attempts) with permanently_failed state | [custody/main.rs](custody/src/main.rs#L3120) |
| 44 | R-H3: serialized burn signature submission (per-job mutex) | [custody/main.rs](custody/src/main.rs#L4260) |
| 45 | M14: configurable slippage tolerance for rebalance swaps | [custody/main.rs](custody/src/main.rs#L5500) |
| 46 | M14: parse actual swap output from on-chain tx (don't assume 1:1) | [custody/main.rs](custody/src/main.rs#L5300) |
| 47 | C3: Uniswap swap recipient is treasury address (not zero) | [custody/main.rs](custody/src/main.rs#L5450) |
| 48 | 0.11: deposit event dedup markers prevent duplicate sweep jobs | [custody/main.rs](custody/src/main.rs#L3650) |
| 49 | Contract deploy/upgrade: signature verification (SHA-256 of code) | [rpc/lib.rs](rpc/src/lib.rs#L5120) |
| 50 | Contract upgrade: version bump + previous_code_hash tracking | [rpc/lib.rs](rpc/src/lib.rs#L5400) |
| 51 | Comprehensive custody unit tests (reserve ledger, gas funding, auth, validation) | [custody/main.rs](custody/src/main.rs#L7100) |

---

## Summary Statistics

| Severity | Count |
|----------|-------|
| CRITICAL | 5 |
| HIGH | 11 |
| MEDIUM | 25 |
| LOW | 20 |
| **Total Findings** | **61** |
| Good Patterns | 51+ |

### Top Priority Fixes

1. **C-3**: Fix `eth_gasPrice`/`eth_estimateGas` — return `gasPrice=1` or `gas=1` (not both as `base_fee`)
2. **C-4**: Replace SHA-256 with Keccak-256 in `eth_getLogs` topic hashing
3. **C-1**: Route prediction market writes through signed transactions / block pipeline
4. **C-2**: Wire FROST 2-round protocol into sweep/withdrawal worker pipelines for multi-signer mode
5. **H-1**: Wrap oracle price writes in system transactions processed by consensus
6. **H-6**: Add confirmation wait between EVM approve and swap in rebalance
7. **H-3**: Add sandboxing (seccomp/namespace) for compiler subprocess execution
