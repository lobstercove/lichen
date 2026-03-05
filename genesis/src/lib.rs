//! MoltChain Genesis — shared library for genesis block creation and contract deployment.
//!
//! This module contains all genesis-related logic extracted from the validator.
//! Used by:
//!   - `moltchain-genesis` CLI binary (creates a fresh chain DB)
//!   - `moltchain-validator` (replays genesis contract deployment on sync)

use moltchain_core::{
    Account, ContractAccount, ContractContext, ContractRuntime,
    Hash, ProgramCallActivity, Pubkey,
    SymbolRegistryEntry, StateStore,
};

use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use tracing::{error, info, warn};



pub const GENESIS_CONTRACT_CATALOG: &[(&str, &str, &str, &str)] = &[
    // Core token
    ("moltcoin", "MOLT", "MoltCoin", "token"),
    // Wrapped tokens
    ("musd_token", "MUSD", "Wrapped USD", "wrapped"),
    ("wsol_token", "WSOL", "Wrapped SOL", "wrapped"),
    ("weth_token", "WETH", "Wrapped ETH", "wrapped"),
    ("wbnb_token", "WBNB", "Wrapped BNB", "wrapped"),
    // DEX
    ("dex_core", "DEX", "MoltChain DEX Core", "dex"),
    ("dex_amm", "DEXAMM", "DEX AMM Engine", "dex"),
    ("dex_router", "DEXROUTER", "DEX Smart Router", "dex"),
    ("dex_margin", "DEXMARGIN", "DEX Margin Trading", "dex"),
    ("dex_rewards", "DEXREWARDS", "DEX Reward Distributor", "dex"),
    ("dex_governance", "DEXGOV", "DEX Governance", "dex"),
    ("dex_analytics", "ANALYTICS", "DEX Analytics", "dex"),
    // DeFi
    ("moltswap", "MOLTSWAP", "MoltSwap AMM", "defi"),
    ("moltbridge", "BRIDGE", "MoltBridge", "bridge"),
    ("moltmarket", "MARKET", "MoltMarket", "marketplace"),
    ("moltoracle", "ORACLE", "MoltOracle", "oracle"),
    ("moltauction", "AUCTION", "MoltAuction", "auction"),
    ("moltdao", "DAO", "MoltDAO Governance", "governance"),
    ("lobsterlend", "LEND", "LobsterLend", "lending"),
    // NFT / Identity
    ("moltpunks", "PUNKS", "MoltPunks NFT", "nft"),
    ("moltyid", "YID", "MoltyID Identity", "identity"),
    // Infrastructure
    ("clawpay", "CLAWPAY", "ClawPay Payments", "payments"),
    ("clawpump", "CLAWPUMP", "ClawPump Launchpad", "launchpad"),
    ("clawvault", "CLAWVAULT", "ClawVault", "vault"),
    ("bountyboard", "BOUNTY", "BountyBoard", "bounty"),
    ("compute_market", "COMPUTE", "Compute Market", "compute"),
    ("reef_storage", "REEF", "Reef Storage", "storage"),
    ("shielded_pool", "SHIELDED", "Shielded Pool", "shielded"),
    // Prediction Markets
    ("prediction_market", "PREDICT", "Prediction Markets", "defi"),
];

pub fn genesis_auto_deploy(state: &StateStore, deployer_pubkey: &Pubkey, label: &str) {
    info!("──────────────────────────────────────────────────────");
    info!("  {} Auto-deploying genesis contracts", label);
    info!("──────────────────────────────────────────────────────");

    let contracts_dir = PathBuf::from("contracts");
    if !contracts_dir.exists() {
        warn!("contracts/ directory not found — skipping auto-deploy");
        return;
    }

    let mut deployed: usize = 0;
    let mut failed: usize = 0;

    for &(dir_name, symbol, display_name, template) in GENESIS_CONTRACT_CATALOG {
        let wasm_path = contracts_dir
            .join(dir_name)
            .join(format!("{}.wasm", dir_name));
        if !wasm_path.exists() {
            warn!(
                "  SKIP {}: WASM not found at {}",
                symbol,
                wasm_path.display()
            );
            failed += 1;
            continue;
        }

        let wasm_bytes = match fs::read(&wasm_path) {
            Ok(bytes) => bytes,
            Err(e) => {
                error!("  FAIL {}: Cannot read WASM: {}", symbol, e);
                failed += 1;
                continue;
            }
        };

        // Derive deterministic program address: SHA-256(deployer + name + wasm)
        // Including the name ensures identical WASMs (e.g. wrapped token stubs)
        // get unique addresses.
        let mut hasher = Sha256::new();
        hasher.update(deployer_pubkey.0);
        hasher.update(dir_name.as_bytes());
        hasher.update(&wasm_bytes);
        let hash_result = hasher.finalize();
        let mut addr_bytes = [0u8; 32];
        addr_bytes.copy_from_slice(&hash_result[..32]);
        let program_pubkey = Pubkey(addr_bytes);

        // Check if already deployed (idempotent)
        if let Ok(Some(_)) = state.get_account(&program_pubkey) {
            info!(
                "  SKIP {}: already deployed at {}",
                symbol,
                program_pubkey.to_base58()
            );
            continue;
        }

        // Create ContractAccount
        let contract = ContractAccount::new(wasm_bytes, *deployer_pubkey);

        // Create executable Account with contract data
        let mut account = Account::new(0, program_pubkey);
        match serde_json::to_vec(&contract) {
            Ok(data) => account.data = data,
            Err(e) => {
                error!("  FAIL {}: Serialize error: {}", symbol, e);
                failed += 1;
                continue;
            }
        }
        account.executable = true;

        // Store the account
        if let Err(e) = state.put_account(&program_pubkey, &account) {
            error!("  FAIL {}: put_account error: {}", symbol, e);
            failed += 1;
            continue;
        }

        // Index in CF_PROGRAMS (makes it visible to getAllContracts)
        if let Err(e) = state.index_program(&program_pubkey) {
            warn!("  WARN {}: index_program error: {}", symbol, e);
        }

        // Register in symbol registry with rich token metadata
        let mut meta = serde_json::json!({
            "genesis_deploy": true,
            "wasm_size": account.data.len(),
        });
        // Enrich token/wrapped contracts with MT-20 metadata
        match template {
            "token" => {
                // MOLT native token: 1B fixed supply, 9 decimals, NOT mintable (deflationary via 50% fee burn)
                meta["total_supply"] = serde_json::json!(1_000_000_000_u64 * 1_000_000_000_u64);
                meta["decimals"] = serde_json::json!(9);
                meta["mintable"] = serde_json::json!(false);
                meta["burnable"] = serde_json::json!(true);
                meta["is_native"] = serde_json::json!(true);
                // Cosmetic profile metadata — shown in explorer contract page
                meta["description"] = serde_json::json!(
                    "The Native Home of Agents. Portable identity + rep tiers \u{2022} Agents run validators & earn \u{2022} DeFi \u{2022} DAO \u{2022} DApps \u{2022} DEX \u{2022} Oracles \u{2022} Storage \u{2022} Vault \u{2022} Pools \u{2022} Bounty"
                );
                meta["website"] = serde_json::json!("https://moltchain.network");
                meta["logo_url"] = serde_json::json!(
                    "https://moltchain.network/assets/img/coins/128x128/molt.png"
                );
                meta["icon_class"] = serde_json::json!("fas fa-fire");
                meta["twitter"] = serde_json::json!("https://x.com/MoltChainHQ");
                meta["telegram"] = serde_json::json!("https://t.me/moltchainhq");
                meta["discord"] = serde_json::json!("https://discord.gg/gkQmsHXRXp");
            }
            "wrapped" => {
                // Wrapped tokens start at 0 supply, 9 decimals
                meta["total_supply"] = serde_json::json!(0);
                meta["decimals"] = serde_json::json!(9);
                meta["mintable"] = serde_json::json!(true);
                meta["burnable"] = serde_json::json!(true);
                // Logo and description per wrapped asset
                let (desc, logo, logo_url) = match symbol {
                    "MUSD" | "mUSD" => (
                        "MoltChain-wrapped USD stablecoin (1:1 USD peg), used as the primary quote currency on MoltyDEX.",
                        "fas fa-dollar-sign",
                        "https://moltchain.network/assets/img/coins/128x128/musd.png",
                    ),
                    "WSOL" | "wSOL" => (
                        "Wrapped Solana (SOL) on MoltChain — bridged 1:1 from the Solana network.",
                        "fab fa-solana",
                        "https://s2.coinmarketcap.com/static/img/coins/128x128/5426.png",
                    ),
                    "WETH" | "wETH" => (
                        "Wrapped Ether (ETH) on MoltChain — bridged 1:1 from the Ethereum network.",
                        "fab fa-ethereum",
                        "https://s2.coinmarketcap.com/static/img/coins/128x128/1027.png",
                    ),
                    "WBNB" | "wBNB" => (
                        "Wrapped BNB on MoltChain — bridged 1:1 from BNB Chain.",
                        "fas fa-coins",
                        "https://s2.coinmarketcap.com/static/img/coins/128x128/1839.png",
                    ),
                    _ => ("Wrapped asset on MoltChain.", "fas fa-coins", ""),
                };
                meta["description"] = serde_json::json!(desc);
                meta["icon_class"] = serde_json::json!(logo);
                if !logo_url.is_empty() {
                    meta["logo_url"] = serde_json::json!(logo_url);
                }
            }
            _ => {}
        }
        let entry = SymbolRegistryEntry {
            symbol: symbol.to_string(),
            program: program_pubkey,
            owner: *deployer_pubkey,
            name: Some(display_name.to_string()),
            template: Some(template.to_string()),
            metadata: Some(meta),
            decimals: match template {
                "token" | "wrapped" => Some(9),
                _ => None,
            },
        };
        if let Err(e) = state.register_symbol(symbol, entry) {
            warn!("  WARN {}: register_symbol error: {}", symbol, e);
        }

        info!(
            "  OK   {} ({}) -> {}",
            symbol,
            display_name,
            program_pubkey.to_base58()
        );
        deployed += 1;
    }

    info!("──────────────────────────────────────────────────────");
    info!(
        "  Genesis deploy complete: {} deployed, {} failed",
        deployed, failed
    );
    info!("──────────────────────────────────────────────────────");
}

// ========================================================================
//  GENESIS PHASE 2 — Initialize all 29 contracts by executing their
//  initialize() function via the WASM runtime.
// ========================================================================

/// Derive a contract's deterministic address from deployer + dir_name + wasm.
/// Must match the derivation in genesis_auto_deploy().
pub fn derive_contract_address(deployer_pubkey: &Pubkey, dir_name: &str) -> Option<Pubkey> {
    let contracts_dir = PathBuf::from("contracts");
    let wasm_path = contracts_dir
        .join(dir_name)
        .join(format!("{}.wasm", dir_name));
    let wasm_bytes = fs::read(&wasm_path).ok()?;
    let mut hasher = Sha256::new();
    hasher.update(deployer_pubkey.0);
    hasher.update(dir_name.as_bytes());
    hasher.update(&wasm_bytes);
    let hash_result = hasher.finalize();
    let mut addr_bytes = [0u8; 32];
    addr_bytes.copy_from_slice(&hash_result[..32]);
    Some(Pubkey(addr_bytes))
}

/// Execute a contract function via WASM runtime and apply storage changes.
/// Returns true on success.
/// Monotonic sequence counter for genesis activity indexing.
/// Each genesis call gets a unique sequence to avoid CF key collisions.
pub static GENESIS_ACTIVITY_SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

pub fn genesis_exec_contract(
    state: &StateStore,
    program_pubkey: &Pubkey,
    deployer_pubkey: &Pubkey,
    function_name: &str,
    args: &[u8],
    label: &str,
) -> bool {
    let account = match state.get_account(program_pubkey) {
        Ok(Some(a)) => a,
        _ => {
            error!("  FAIL {}: account not found", label);
            return false;
        }
    };

    let mut contract: ContractAccount = match serde_json::from_slice(&account.data) {
        Ok(c) => c,
        Err(e) => {
            error!("  FAIL {}: deserialize ContractAccount: {}", label, e);
            return false;
        }
    };

    let ctx = ContractContext::with_args(
        *deployer_pubkey,
        *program_pubkey,
        0,
        0,
        contract.storage.clone(),
        args.to_vec(),
    );

    let mut runtime = ContractRuntime::new();
    match runtime.execute(&contract, function_name, args, ctx) {
        Ok(result) => {
            if !result.success {
                // Check for non-zero return code — indicates a real WASM error,
                // not just "already initialized". Return false so callers know.
                let rc = result.return_code.unwrap_or(1);
                if rc != 0 {
                    warn!(
                        "  FAIL {}: contract returned error code {} — {:?}",
                        label, rc, result.error
                    );
                    return false;
                }
                // return_code == 0 with success == false: treat as non-fatal
                // (e.g., "already initialized" idempotent calls)
                warn!(
                    "  WARN {}: contract returned !success with rc=0: {:?}",
                    label, result.error
                );
            }
            // Apply storage changes
            for (key, val_opt) in &result.storage_changes {
                match val_opt {
                    Some(val) => {
                        contract.set_storage(key.clone(), val.clone());
                        // Also write to CF_CONTRACT_STORAGE for fast-path RPC reads
                        if let Err(e) = state.put_contract_storage(program_pubkey, key, val) {
                            warn!("  WARN {}: put_contract_storage: {}", label, e);
                        }
                    }
                    None => {
                        contract.remove_storage(key);
                        let _ = state.delete_contract_storage(program_pubkey, key);
                    }
                }
            }
            // Re-serialize and store
            let mut updated_account = account;
            match serde_json::to_vec(&contract) {
                Ok(data) => updated_account.data = data,
                Err(e) => {
                    error!("  FAIL {}: re-serialize: {}", label, e);
                    return false;
                }
            }
            if let Err(e) = state.put_account(program_pubkey, &updated_account) {
                error!("  FAIL {}: put_account: {}", label, e);
                return false;
            }

            // ── Record genesis call in CF_PROGRAM_CALLS for explorer indexing ──
            let seq = GENESIS_ACTIVITY_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let activity = ProgramCallActivity {
                slot: 0,
                timestamp: 0,
                program: *program_pubkey,
                caller: *deployer_pubkey,
                function: function_name.to_string(),
                value: 0,
                tx_signature: Hash([0u8; 32]), // Genesis — no real tx
            };
            if let Err(e) = state.record_program_call(&activity, seq) {
                warn!("  WARN {}: failed to record genesis call: {}", label, e);
            }

            // ── Persist any events emitted during genesis WASM execution ──
            for event in &result.events {
                if let Err(e) = state.put_contract_event(program_pubkey, event) {
                    warn!("  WARN {}: failed to record genesis event: {}", label, e);
                }
            }

            true
        }
        Err(e) => {
            error!("  FAIL {}: WASM execution error: {}", label, e);
            false
        }
    }
}

pub fn genesis_initialize_contracts(state: &StateStore, deployer_pubkey: &Pubkey, label: &str) {
    info!("──────────────────────────────────────────────────────");
    info!("  {} Initializing all contracts", label);
    info!("──────────────────────────────────────────────────────");

    let admin = deployer_pubkey.0;
    let mut initialized: usize = 0;
    let mut skipped: usize = 0;

    // Build a lookup: dir_name -> Pubkey for cross-references
    let mut address_map: HashMap<String, Pubkey> = HashMap::new();
    for &(dir_name, _symbol, _display, _template) in GENESIS_CONTRACT_CATALOG {
        if let Some(addr) = derive_contract_address(deployer_pubkey, dir_name) {
            address_map.insert(dir_name.to_string(), addr);
        }
    }

    // ── Initialization in dependency order ──
    // Layer 0: Tokens (no dependencies)
    // Layer 1: Identity
    // Layer 2: DEX core (opcode dispatch)
    // Layer 3: DEX infrastructure (opcode dispatch)
    // Layer 4: DeFi protocols
    // Layer 5: Applications

    // Define initialization config for each contract:
    // (dir_name, function_name, args_builder)
    // For opcode-dispatch contracts: function="call", args=[0x00][admin 32B]
    // For named-export contracts: function="initialize" (or variant), args=[admin 32B]

    struct InitSpec {
        dir_name: &'static str,
        function: &'static str,
        /// Build arguments. We pass in admin pubkey and address map.
        args: Vec<u8>,
    }

    // Helper: build opcode-dispatch init args [0x00][admin 32B]
    fn opcode_init_args(admin: &[u8; 32]) -> Vec<u8> {
        let mut args = Vec::with_capacity(33);
        args.push(0x00); // opcode 0 = initialize
        args.extend_from_slice(admin);
        args
    }

    // Helper: build named-export init args = just [admin 32B]
    fn named_init_args(admin: &[u8; 32]) -> Vec<u8> {
        admin.to_vec()
    }

    // Resolve token contract addresses for moltswap and moltdao
    let molt_addr = address_map
        .get("moltcoin")
        .map(|p| p.0)
        .unwrap_or([0u8; 32]);
    let musd_addr = address_map
        .get("musd_token")
        .map(|p| p.0)
        .unwrap_or([0u8; 32]);

    // DAO: governance_token = MOLT address, treasury = community_treasury wallet,
    // min_proposal_threshold = 10,000 MOLT in shells (10_000 * 1e9)
    let dao_treasury = state
        .get_community_treasury_pubkey()
        .ok()
        .flatten()
        .map(|pk| pk.0)
        .unwrap_or(admin); // Fallback to deployer if community_treasury not set yet
    let dao_threshold: u64 = 10_000_000_000_000; // 10,000 MOLT
    let mut dao_args = Vec::with_capacity(72);
    dao_args.extend_from_slice(&molt_addr); // governance_token (32B)
    dao_args.extend_from_slice(&dao_treasury); // treasury (32B = community_treasury wallet)
    dao_args.extend_from_slice(&dao_threshold.to_le_bytes()); // min_proposal_threshold (8B)

    // MoltSwap: token_a = MOLT, token_b = MUSD
    let mut moltswap_args = Vec::with_capacity(64);
    moltswap_args.extend_from_slice(&molt_addr);
    moltswap_args.extend_from_slice(&musd_addr);

    // MoltMarket: owner(32B) + fee_addr(32B) = deployer for both initially
    let mut moltmarket_args = Vec::with_capacity(64);
    moltmarket_args.extend_from_slice(&admin);
    moltmarket_args.extend_from_slice(&admin); // fee recipient = deployer initially

    // MoltAuction: initialize(marketplace_addr) + initialize_ma_admin(admin)
    // marketplace_addr = moltmarket address for integration
    let moltmarket_addr = address_map.get("moltmarket").map(|p| p.0).unwrap_or(admin);

    let specs: Vec<InitSpec> = vec![
        // ── Layer 0: Tokens ──
        InitSpec {
            dir_name: "moltcoin",
            function: "initialize",
            args: named_init_args(&admin),
        },
        InitSpec {
            dir_name: "musd_token",
            function: "initialize",
            args: named_init_args(&admin),
        },
        InitSpec {
            dir_name: "wsol_token",
            function: "initialize",
            args: named_init_args(&admin),
        },
        InitSpec {
            dir_name: "weth_token",
            function: "initialize",
            args: named_init_args(&admin),
        },
        InitSpec {
            dir_name: "wbnb_token",
            function: "initialize",
            args: named_init_args(&admin),
        },
        // ── Layer 1: Identity ──
        InitSpec {
            dir_name: "moltyid",
            function: "initialize",
            args: named_init_args(&admin),
        },
        // ── Layer 2: DEX core (opcode dispatch) ──
        InitSpec {
            dir_name: "dex_core",
            function: "call",
            args: opcode_init_args(&admin),
        },
        InitSpec {
            dir_name: "dex_amm",
            function: "call",
            args: opcode_init_args(&admin),
        },
        InitSpec {
            dir_name: "dex_router",
            function: "call",
            args: opcode_init_args(&admin),
        },
        // ── Layer 3: DEX infrastructure (opcode dispatch) ──
        InitSpec {
            dir_name: "dex_margin",
            function: "call",
            args: opcode_init_args(&admin),
        },
        InitSpec {
            dir_name: "dex_rewards",
            function: "call",
            args: opcode_init_args(&admin),
        },
        InitSpec {
            dir_name: "dex_governance",
            function: "call",
            args: opcode_init_args(&admin),
        },
        InitSpec {
            dir_name: "dex_analytics",
            function: "call",
            args: opcode_init_args(&admin),
        },
        // ── Layer 4: DeFi protocols ──
        InitSpec {
            dir_name: "moltswap",
            function: "initialize",
            args: moltswap_args,
        },
        InitSpec {
            dir_name: "moltbridge",
            function: "initialize",
            args: named_init_args(&admin),
        },
        InitSpec {
            dir_name: "moltoracle",
            function: "initialize_oracle",
            args: named_init_args(&admin),
        },
        InitSpec {
            dir_name: "lobsterlend",
            function: "initialize",
            args: named_init_args(&admin),
        },
        // ── Layer 4b: Governance ──
        InitSpec {
            dir_name: "moltdao",
            function: "initialize_dao",
            args: dao_args,
        },
        // ── Layer 5: Marketplaces ──
        InitSpec {
            dir_name: "moltmarket",
            function: "initialize",
            args: moltmarket_args,
        },
        InitSpec {
            dir_name: "moltpunks",
            function: "initialize",
            args: named_init_args(&admin),
        },
        // ── Layer 5b: Infrastructure ──
        InitSpec {
            dir_name: "clawpay",
            function: "initialize_cp_admin",
            args: named_init_args(&admin),
        },
        InitSpec {
            dir_name: "clawpump",
            function: "initialize",
            args: named_init_args(&admin),
        },
        InitSpec {
            dir_name: "clawvault",
            function: "initialize",
            args: named_init_args(&admin),
        },
        InitSpec {
            dir_name: "compute_market",
            function: "initialize",
            args: named_init_args(&admin),
        },
        InitSpec {
            dir_name: "reef_storage",
            function: "initialize",
            args: named_init_args(&admin),
        },
        // ── Layer 5c: Prediction Markets ──
        InitSpec {
            dir_name: "prediction_market",
            function: "initialize",
            args: named_init_args(&admin),
        },
        // ── Layer 5d: BountyBoard ──
        // bountyboard.initialize() sets identity_admin which is required by
        // verify_identity, update_reputation, and issue_credential.
        // Without this, first-caller-wins vulnerability (see G22-02).
        InitSpec {
            dir_name: "bountyboard",
            function: "initialize",
            args: named_init_args(&admin),
        },
        // ── Layer 5e: Shielded Pool ──
        // Initializes the on-chain shielded pool WASM contract.
        // Heavy ZK proof verification runs natively in the processor;
        // this contract stores pool state and provides query endpoints.
        InitSpec {
            dir_name: "shielded_pool",
            function: "initialize",
            args: named_init_args(&admin),
        },
    ];

    for spec in &specs {
        let pubkey = match address_map.get(spec.dir_name) {
            Some(pk) => *pk,
            None => {
                warn!(
                    "  SKIP {}: address not derived (WASM missing?)",
                    spec.dir_name
                );
                skipped += 1;
                continue;
            }
        };

        if genesis_exec_contract(
            state,
            &pubkey,
            deployer_pubkey,
            spec.function,
            &spec.args,
            spec.dir_name,
        ) {
            info!("  INIT {}", spec.dir_name);
            initialized += 1;
        } else {
            skipped += 1;
        }
    }

    // MoltAuction requires TWO init calls:
    // 1. initialize(marketplace_addr) — sets escrow address
    // 2. initialize_ma_admin(admin) — sets admin
    if let Some(auction_pk) = address_map.get("moltauction") {
        let mkt_args = moltmarket_addr.to_vec();
        if genesis_exec_contract(
            state,
            auction_pk,
            deployer_pubkey,
            "initialize",
            &mkt_args,
            "moltauction(escrow)",
        ) {
            if genesis_exec_contract(
                state,
                auction_pk,
                deployer_pubkey,
                "initialize_ma_admin",
                admin.as_ref(),
                "moltauction(admin)",
            ) {
                info!("  INIT moltauction (escrow + admin)");
                initialized += 1;
            } else {
                skipped += 1;
            }
        } else {
            skipped += 1;
        }
    }

    // ── Prediction Market: wire up cross-contract addresses ──
    // Set oracle, musd, moltyid, and dex_gov addresses via opcode dispatch.
    // Opcodes: 18=set_moltyid, 19=set_oracle, 20=set_musd, 21=set_dex_gov
    // Format: [opcode][admin 32B][address 32B] = 65 bytes
    if let Some(predict_pk) = address_map.get("prediction_market") {
        let oracle_addr = address_map.get("moltoracle").map(|p| p.0).unwrap_or(admin);
        let moltyid_addr = address_map.get("moltyid").map(|p| p.0).unwrap_or(admin);
        let dex_gov_addr = address_map
            .get("dex_governance")
            .map(|p| p.0)
            .unwrap_or(admin);

        // NOTE: MoltyID address IS set here. The processor's cross-contract
        // storage injection reads the caller's MoltyID reputation from
        // CF_CONTRACT_STORAGE and injects it into the contract's execution
        // context before WASM runs. The contract's load_u64("rep:{hex}")
        // call finds the injected value in ctx.storage.
        let configs: &[(u8, &[u8; 32], &str)] = &[
            (18, &moltyid_addr, "prediction_market(moltyid)"),
            (19, &oracle_addr, "prediction_market(oracle)"),
            (20, &musd_addr, "prediction_market(musd)"),
            (21, &dex_gov_addr, "prediction_market(dex_gov)"),
        ];

        for &(opcode, addr, label) in configs {
            let mut args = Vec::with_capacity(65);
            args.push(opcode);
            args.extend_from_slice(&admin);
            args.extend_from_slice(addr);
            if genesis_exec_contract(state, predict_pk, deployer_pubkey, "call", &args, label) {
                info!("  SET {}", label);
            } else {
                warn!("  WARN: Failed to set {}", label);
            }
        }
    }

    // ── DEX Governance: wire up MoltyID address for reputation verification ──
    // Opcode 14 = set_moltyid_address. Format: [14][admin 32B][moltyid_addr 32B]
    if let Some(dex_gov_pk) = address_map.get("dex_governance") {
        let moltyid_addr = address_map.get("moltyid").map(|p| p.0).unwrap_or(admin);
        let mut args = Vec::with_capacity(65);
        args.push(14u8);
        args.extend_from_slice(&admin);
        args.extend_from_slice(&moltyid_addr);
        if genesis_exec_contract(
            state,
            dex_gov_pk,
            deployer_pubkey,
            "call",
            &args,
            "dex_governance(moltyid)",
        ) {
            info!("  SET dex_governance(moltyid)");
        } else {
            warn!("  WARN: Failed to set dex_governance(moltyid)");
        }
    }

    // ── DEX Rewards: set builder_grants wallet as rewards pool source ──
    // The dex_rewards contract pays out MOLT from its own balance (self-custody).
    // Wire builder_grants as the source, then seed the contract with 1 year of
    // rewards (1.2M MOLT = 100K/month × 12) so claims work from day one.
    if let Some(dex_rewards_pk) = address_map.get("dex_rewards") {
        let builder_grants_addr = state
            .get_builder_grants_pubkey()
            .ok()
            .flatten()
            .map(|pk| pk.0)
            .unwrap_or(admin);

        // Opcode 13 = set_rewards_pool. Format: [13][caller 32B][addr 32B]
        let mut args = Vec::with_capacity(65);
        args.push(13u8);
        args.extend_from_slice(&admin);
        args.extend_from_slice(&builder_grants_addr);
        if genesis_exec_contract(
            state,
            dex_rewards_pk,
            deployer_pubkey,
            "call",
            &args,
            "dex_rewards(builder_grants)",
        ) {
            info!("  SET dex_rewards(builder_grants)");
        } else {
            warn!("  WARN: Failed to set dex_rewards builder_grants pool");
        }

        // Seed the contract with 1 year of rewards from builder_grants.
        // Contract uses self-custody: it pays from its own address, so it needs
        // MOLT deposited into the contract's account.
        let seed_molt: u64 = 1_200_000; // 100K/month × 12 months
        let seed_shells = seed_molt * 1_000_000_000;
        let bg_pubkey = Pubkey(builder_grants_addr);
        if let Ok(Some(mut bg_acct)) = state.get_account(&bg_pubkey) {
            if bg_acct.spendable >= seed_shells {
                bg_acct.deduct_spendable(seed_shells).ok();
                state.put_account(&bg_pubkey, &bg_acct).ok();

                let mut contract_acct = state
                    .get_account(dex_rewards_pk)
                    .ok()
                    .flatten()
                    .unwrap_or_else(|| Account::new(0, *dex_rewards_pk));
                contract_acct.add_spendable(seed_shells).ok();
                state.put_account(dex_rewards_pk, &contract_acct).ok();
                info!("  💰 Seeded dex_rewards contract with {} MOLT from builder_grants (1 year of rewards)", seed_molt);
            } else {
                warn!("  WARN: builder_grants has insufficient balance to seed dex_rewards");
            }
        }
    }

    // ── DEX Router: wire dex_core, dex_amm, moltswap addresses ──
    // Opcode 1 = set_addresses. Format: [1][admin 32B][dex_core 32B][dex_amm 32B][moltswap 32B]
    if let Some(router_pk) = address_map.get("dex_router") {
        let dex_core_addr = address_map.get("dex_core").map(|p| p.0).unwrap_or(admin);
        let dex_amm_addr = address_map.get("dex_amm").map(|p| p.0).unwrap_or(admin);
        let moltswap_addr = address_map.get("moltswap").map(|p| p.0).unwrap_or(admin);
        let mut args = Vec::with_capacity(129);
        args.push(1u8); // opcode 1 = set_addresses
        args.extend_from_slice(&admin);
        args.extend_from_slice(&dex_core_addr);
        args.extend_from_slice(&dex_amm_addr);
        args.extend_from_slice(&moltswap_addr);
        if genesis_exec_contract(
            state,
            router_pk,
            deployer_pubkey,
            "call",
            &args,
            "dex_router(set_addresses)",
        ) {
            info!("  SET dex_router(set_addresses)");
        } else {
            warn!("  WARN: Failed to set dex_router addresses");
        }

        // ── DEX Router: register genesis routes (10 routes for 5 pairs) ──
        // Opcode 2 = register_route. 115 bytes:
        // [opcode 1B][caller 32B][token_in 32B][token_out 32B][route_type 1B][pool_id 8B][secondary_id 8B][split_percent 1B]
        let wsol_addr = address_map
            .get("wsol_token")
            .map(|p| p.0)
            .unwrap_or([0u8; 32]);
        let weth_addr = address_map
            .get("weth_token")
            .map(|p| p.0)
            .unwrap_or([0u8; 32]);
        let wbnb_addr = address_map
            .get("wbnb_token")
            .map(|p| p.0)
            .unwrap_or([0u8; 32]);

        // (token_in, token_out, pair_id, pool_id, label)
        type RoutePair = ([u8; 32], [u8; 32], u64, u64, &'static str);
        let route_pairs: [RoutePair; 7] = [
            (molt_addr, musd_addr, 1, 1, "MOLT/mUSD"),
            (wsol_addr, musd_addr, 2, 2, "wSOL/mUSD"),
            (weth_addr, musd_addr, 3, 3, "wETH/mUSD"),
            (wsol_addr, molt_addr, 4, 4, "wSOL/MOLT"),
            (weth_addr, molt_addr, 5, 5, "wETH/MOLT"),
            (wbnb_addr, musd_addr, 6, 6, "wBNB/mUSD"),
            (wbnb_addr, molt_addr, 7, 7, "wBNB/MOLT"),
        ];

        for (token_in, token_out, pair_id, pool_id, label) in &route_pairs {
            // CLOB route: route_type=0, id=pair_id
            let mut clob_args = Vec::with_capacity(115);
            clob_args.push(2u8); // opcode 2 = register_route
            clob_args.extend_from_slice(&admin);
            clob_args.extend_from_slice(token_in);
            clob_args.extend_from_slice(token_out);
            clob_args.push(0); // route_type: DIRECT_CLOB
            clob_args.extend_from_slice(&pair_id.to_le_bytes());
            clob_args.extend_from_slice(&0u64.to_le_bytes()); // secondary_id
            clob_args.push(0); // split_percent
            if genesis_exec_contract(
                state,
                router_pk,
                deployer_pubkey,
                "call",
                &clob_args,
                &format!("dex_router(route CLOB {})", label),
            ) {
                info!("  ROUTE CLOB {} (pair_id={})", label, pair_id);
            } else {
                warn!("  WARN: Failed to register CLOB route {}", label);
            }

            // AMM route: route_type=1, id=pool_id
            let mut amm_args = Vec::with_capacity(115);
            amm_args.push(2u8); // opcode 2 = register_route
            amm_args.extend_from_slice(&admin);
            amm_args.extend_from_slice(token_in);
            amm_args.extend_from_slice(token_out);
            amm_args.push(1); // route_type: DIRECT_AMM
            amm_args.extend_from_slice(&pool_id.to_le_bytes());
            amm_args.extend_from_slice(&0u64.to_le_bytes()); // secondary_id
            amm_args.push(0); // split_percent
            if genesis_exec_contract(
                state,
                router_pk,
                deployer_pubkey,
                "call",
                &amm_args,
                &format!("dex_router(route AMM {})", label),
            ) {
                info!("  ROUTE AMM {} (pool_id={})", label, pool_id);
            } else {
                warn!("  WARN: Failed to register AMM route {}", label);
            }
        }
        info!("  ✅ Registered 10 genesis routes (5 CLOB + 5 AMM)");
    }

    // ── MoltDAO: wire MoltyID address for identity verification ──
    // Named export: set_moltyid_address. Args: [admin 32B][moltyid_addr 32B]
    if let Some(dao_pk) = address_map.get("moltdao") {
        let moltyid_addr = address_map.get("moltyid").map(|p| p.0).unwrap_or(admin);
        let mut args = Vec::with_capacity(64);
        args.extend_from_slice(&admin);
        args.extend_from_slice(&moltyid_addr);
        if genesis_exec_contract(
            state,
            dao_pk,
            deployer_pubkey,
            "set_moltyid_address",
            &args,
            "moltdao(moltyid)",
        ) {
            info!("  SET moltdao(moltyid)");
        } else {
            warn!("  WARN: Failed to set moltdao moltyid address");
        }
    }

    // ── MoltSwap: wire MoltyID address for identity verification ──
    // Named export: set_moltyid_address. Args: [admin 32B][moltyid_addr 32B]
    if let Some(swap_pk) = address_map.get("moltswap") {
        let moltyid_addr = address_map.get("moltyid").map(|p| p.0).unwrap_or(admin);
        let mut args = Vec::with_capacity(64);
        args.extend_from_slice(&admin);
        args.extend_from_slice(&moltyid_addr);
        if genesis_exec_contract(
            state,
            swap_pk,
            deployer_pubkey,
            "set_moltyid_address",
            &args,
            "moltswap(moltyid)",
        ) {
            info!("  SET moltswap(moltyid)");
        } else {
            warn!("  WARN: Failed to set moltswap moltyid address");
        }
    }

    // ── Reef Storage: wire MOLT token address ──
    // Named export: set_molt_token. Args: [admin 32B][moltcoin_addr 32B]
    if let Some(reef_pk) = address_map.get("reef_storage") {
        let mut args = Vec::with_capacity(64);
        args.extend_from_slice(&admin);
        args.extend_from_slice(&molt_addr);
        if genesis_exec_contract(
            state,
            reef_pk,
            deployer_pubkey,
            "set_molt_token",
            &args,
            "reef_storage(moltcoin)",
        ) {
            info!("  SET reef_storage(moltcoin)");
        } else {
            warn!("  WARN: Failed to set reef_storage molt token address");
        }
    }

    // ── LobsterLend: wire MOLT token address ──
    // Named export: set_moltcoin_address. Args: [admin 32B][moltcoin_addr 32B]
    if let Some(lend_pk) = address_map.get("lobsterlend") {
        let mut args = Vec::with_capacity(64);
        args.extend_from_slice(&admin);
        args.extend_from_slice(&molt_addr);
        if genesis_exec_contract(
            state,
            lend_pk,
            deployer_pubkey,
            "set_moltcoin_address",
            &args,
            "lobsterlend(moltcoin)",
        ) {
            info!("  SET lobsterlend(moltcoin)");
        } else {
            warn!("  WARN: Failed to set lobsterlend moltcoin address");
        }
    }

    // ── MoltBridge: wire MOLT token + add first bridge validator ──
    // Named export: set_token_address. Args: [admin 32B][moltcoin_addr 32B]
    if let Some(bridge_pk) = address_map.get("moltbridge") {
        let mut args = Vec::with_capacity(64);
        args.extend_from_slice(&admin);
        args.extend_from_slice(&molt_addr);
        if genesis_exec_contract(
            state,
            bridge_pk,
            deployer_pubkey,
            "set_token_address",
            &args,
            "moltbridge(token)",
        ) {
            info!("  SET moltbridge(token)");
        } else {
            warn!("  WARN: Failed to set moltbridge token address");
        }

        // Add deployer as first bridge validator
        // Named export: add_bridge_validator. Args: [admin 32B][validator_pubkey 32B]
        let mut val_args = Vec::with_capacity(64);
        val_args.extend_from_slice(&admin);
        val_args.extend_from_slice(&admin); // deployer is first bridge validator
        if genesis_exec_contract(
            state,
            bridge_pk,
            deployer_pubkey,
            "add_bridge_validator",
            &val_args,
            "moltbridge(bridge_validator)",
        ) {
            info!("  SET moltbridge(bridge_validator)");
        } else {
            warn!("  WARN: Failed to add bridge validator to moltbridge");
        }
    }

    // ── MoltyID: Bootstrap admin reputation ──
    // The admin (deployer) needs reputation >= 1000 to create prediction markets,
    // submit governance proposals, resolve markets, etc. The initial identity
    // registration gives only 100. Write directly to MoltyID's contract storage
    // so the admin has the required reputation from genesis.
    if let Some(moltyid_pk) = address_map.get("moltyid") {
        let admin_rep: u64 = 5000; // "Elite" tier — full access to all features
        let hex_chars: &[u8; 16] = b"0123456789abcdef";
        let mut rep_key = Vec::with_capacity(68);
        rep_key.extend_from_slice(b"rep:");
        for &b in admin.iter() {
            rep_key.push(hex_chars[(b >> 4) as usize]);
            rep_key.push(hex_chars[(b & 0x0f) as usize]);
        }
        if let Err(e) = state.put_contract_storage(moltyid_pk, &rep_key, &admin_rep.to_le_bytes()) {
            warn!("  WARN: Failed to set admin reputation in MoltyID: {}", e);
        } else {
            info!(
                "  SET admin MoltyID reputation = {} (Elite tier)",
                admin_rep
            );
        }
    }

    // ── MoltyID: Register reserved .molt names at genesis ──
    // Uses admin_register_reserved_name to bypass reserved-name checks.
    // Format: admin_register_reserved_name(admin_ptr, owner_ptr, name_ptr, name_len, agent_type)
    // Since this is a named export, args = [admin 32B][owner 32B][name bytes][name_len 4B LE][agent_type 1B]
    if let Some(moltyid_pk) = address_map.get("moltyid") {
        // Genesis .molt name registrations:
        // System wallets get their canonical names
        struct GenesisName {
            label: &'static str,
            owner_key: &'static str, // address_map key or "admin" for deployer
            agent_type: u8,          // 0=system
        }

        let genesis_names: &[GenesisName] = &[
            // ── System / Admin wallets ──
            GenesisName {
                label: "moltchain",
                owner_key: "admin",
                agent_type: 0,
            },
            GenesisName {
                label: "treasury",
                owner_key: "admin",
                agent_type: 0,
            },
            GenesisName {
                label: "validator",
                owner_key: "admin",
                agent_type: 0,
            },
            GenesisName {
                label: "system",
                owner_key: "admin",
                agent_type: 0,
            },
            GenesisName {
                label: "admin",
                owner_key: "admin",
                agent_type: 0,
            },
            // ── Core token ──
            GenesisName {
                label: "moltcoin",
                owner_key: "moltcoin",
                agent_type: 0,
            },
            // ── Wrapped tokens ──
            GenesisName {
                label: "musd",
                owner_key: "musd_token",
                agent_type: 0,
            },
            GenesisName {
                label: "wsol",
                owner_key: "wsol_token",
                agent_type: 0,
            },
            GenesisName {
                label: "weth",
                owner_key: "weth_token",
                agent_type: 0,
            },
            GenesisName {
                label: "wbnb",
                owner_key: "wbnb_token",
                agent_type: 0,
            },
            // ── DEX ──
            GenesisName {
                label: "dex",
                owner_key: "dex_core",
                agent_type: 0,
            },
            GenesisName {
                label: "amm",
                owner_key: "dex_amm",
                agent_type: 0,
            },
            GenesisName {
                label: "router",
                owner_key: "dex_router",
                agent_type: 0,
            },
            GenesisName {
                label: "margin",
                owner_key: "dex_margin",
                agent_type: 0,
            },
            GenesisName {
                label: "rewards",
                owner_key: "dex_rewards",
                agent_type: 0,
            },
            GenesisName {
                label: "governance",
                owner_key: "dex_governance",
                agent_type: 0,
            },
            GenesisName {
                label: "analytics",
                owner_key: "dex_analytics",
                agent_type: 0,
            },
            // ── DeFi protocols ──
            GenesisName {
                label: "moltswap",
                owner_key: "moltswap",
                agent_type: 0,
            },
            GenesisName {
                label: "bridge",
                owner_key: "moltbridge",
                agent_type: 0,
            },
            GenesisName {
                label: "oracle",
                owner_key: "moltoracle",
                agent_type: 0,
            },
            GenesisName {
                label: "dao",
                owner_key: "moltdao",
                agent_type: 0,
            },
            GenesisName {
                label: "lending",
                owner_key: "lobsterlend",
                agent_type: 0,
            },
            // ── Marketplaces ──
            GenesisName {
                label: "marketplace",
                owner_key: "moltmarket",
                agent_type: 0,
            },
            GenesisName {
                label: "auction",
                owner_key: "moltauction",
                agent_type: 0,
            },
            GenesisName {
                label: "moltpunks",
                owner_key: "moltpunks",
                agent_type: 0,
            },
            // ── Identity ──
            GenesisName {
                label: "moltyid",
                owner_key: "moltyid",
                agent_type: 0,
            },
            // ── Infrastructure ──
            GenesisName {
                label: "clawpay",
                owner_key: "clawpay",
                agent_type: 0,
            },
            GenesisName {
                label: "clawpump",
                owner_key: "clawpump",
                agent_type: 0,
            },
            GenesisName {
                label: "clawvault",
                owner_key: "clawvault",
                agent_type: 0,
            },
            GenesisName {
                label: "bountyboard",
                owner_key: "bountyboard",
                agent_type: 0,
            },
            GenesisName {
                label: "compute",
                owner_key: "compute_market",
                agent_type: 0,
            },
            GenesisName {
                label: "reefstake",
                owner_key: "reef_storage",
                agent_type: 0,
            },
            // ── Prediction Markets ──
            GenesisName {
                label: "predict",
                owner_key: "prediction_market",
                agent_type: 0,
            },
        ];

        for gn in genesis_names {
            let owner_addr = if gn.owner_key == "admin" {
                admin
            } else {
                address_map.get(gn.owner_key).map(|p| p.0).unwrap_or(admin)
            };

            // Build args: [admin 32B][owner 32B][name bytes...][name_len 4B LE][agent_type 1B]
            let name_bytes = gn.label.as_bytes();
            let name_len = name_bytes.len() as u32;
            let mut args = Vec::with_capacity(32 + 32 + name_bytes.len() + 4 + 1);
            args.extend_from_slice(&admin);
            args.extend_from_slice(&owner_addr);
            args.extend_from_slice(name_bytes);
            args.extend_from_slice(&name_len.to_le_bytes());
            args.push(gn.agent_type);

            if genesis_exec_contract(
                state,
                moltyid_pk,
                deployer_pubkey,
                "admin_register_reserved_name",
                &args,
                &format!("moltyid(name:{})", gn.label),
            ) {
                info!(
                    "  NAME {}.molt → {}",
                    gn.label,
                    if gn.owner_key == "admin" {
                        "deployer"
                    } else {
                        gn.owner_key
                    }
                );
            } else {
                warn!("  WARN: Failed to register {}.molt", gn.label);
            }
        }

        // ── MoltyID: Genesis cross-attestations between system identities ──
        // After all reserved names are registered (each creating an identity with
        // 3 skills: Infrastructure, Consensus, Security), have system identities
        // attest each other's skills to seed the attestation system. This makes
        // the chain show real attestation data from boot instead of all-zeros.
        //
        // Key format matches the contract exactly:
        //   attestation:  "attest_{identity_hex}_{skill_hash_hex}_{attester_hex}"
        //   count:        "attest_count_{identity_hex}_{skill_hash_hex}"
        //
        // Skill hash is FNV-1a 128-bit (same as contract's skill_name_hash).

        let hex_chars: &[u8; 16] = b"0123456789abcdef";

        /// FNV-1a 128-bit hash (matches contract's skill_name_hash)
        fn fnv1a_128(data: &[u8]) -> [u8; 16] {
            const FNV_OFFSET_BASIS: u128 = 0x6c62272e07bb0142_62b821756295c58d;
            const FNV_PRIME: u128 = 0x0000000001000000_000000000000013B;
            let mut hash: u128 = FNV_OFFSET_BASIS;
            for &byte in data {
                hash ^= byte as u128;
                hash = hash.wrapping_mul(FNV_PRIME);
            }
            hash.to_le_bytes()
        }

        fn hex_encode_32(bytes: &[u8; 32], hex_chars: &[u8; 16]) -> [u8; 64] {
            let mut out = [0u8; 64];
            for i in 0..32 {
                out[i * 2] = hex_chars[(bytes[i] >> 4) as usize];
                out[i * 2 + 1] = hex_chars[(bytes[i] & 0x0f) as usize];
            }
            out
        }

        fn hex_encode_16(bytes: &[u8; 16], hex_chars: &[u8; 16]) -> [u8; 32] {
            let mut out = [0u8; 32];
            for i in 0..16 {
                out[i * 2] = hex_chars[(bytes[i] >> 4) as usize];
                out[i * 2 + 1] = hex_chars[(bytes[i] & 0x0f) as usize];
            }
            out
        }

        // Collect system identities that were registered (up to first 10 for attestation pairs)
        let attestation_skills: &[&[u8]] = &[b"Infrastructure", b"Consensus", b"Security"];
        let mut system_addrs: Vec<[u8; 32]> = Vec::new();

        // Use the same pairs from genesis_names: first 10 unique owner addresses
        let mut seen_addrs = std::collections::HashSet::new();
        for gn in genesis_names {
            let owner_addr = if gn.owner_key == "admin" {
                admin
            } else {
                address_map.get(gn.owner_key).map(|p| p.0).unwrap_or(admin)
            };
            if seen_addrs.insert(owner_addr) {
                system_addrs.push(owner_addr);
                if system_addrs.len() >= 10 {
                    break;
                }
            }
        }

        // Collect all attestation storage writes to also update embedded ContractAccount
        struct AttestEntry {
            key: Vec<u8>,
            value: Vec<u8>,
        }
        let mut attest_entries: Vec<AttestEntry> = Vec::new();

        let mut attest_count: u64 = 0;
        let now_ts: u64 = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        // Each system identity attests the NEXT identity's skills (round-robin)
        for i in 0..system_addrs.len() {
            let attester = system_addrs[i];
            let target = system_addrs[(i + 1) % system_addrs.len()];
            if attester == target {
                continue;
            }

            let attester_hex = hex_encode_32(&attester, hex_chars);
            let target_hex = hex_encode_32(&target, hex_chars);

            for skill_name in attestation_skills {
                let skill_hash = fnv1a_128(skill_name);
                let skill_hash_hex = hex_encode_16(&skill_hash, hex_chars);

                // Build attestation key: "attest_{target_hex}_{skill_hash_hex}_{attester_hex}"
                let mut att_key = Vec::with_capacity(7 + 64 + 1 + 32 + 1 + 64);
                att_key.extend_from_slice(b"attest_");
                att_key.extend_from_slice(&target_hex);
                att_key.push(b'_');
                att_key.extend_from_slice(&skill_hash_hex);
                att_key.push(b'_');
                att_key.extend_from_slice(&attester_hex);

                // Attestation data: level (1 byte) + timestamp (8 bytes)
                let mut att_data = Vec::with_capacity(9);
                att_data.push(5u8); // Level 5 (highest) for system attestations
                att_data.extend_from_slice(&now_ts.to_le_bytes());

                if state
                    .put_contract_storage(moltyid_pk, &att_key, &att_data)
                    .is_err()
                {
                    continue;
                }
                attest_entries.push(AttestEntry {
                    key: att_key,
                    value: att_data,
                });

                // Build attestation count key: "attest_count_{target_hex}_{skill_hash_hex}"
                let mut count_key = Vec::with_capacity(13 + 64 + 1 + 32);
                count_key.extend_from_slice(b"attest_count_");
                count_key.extend_from_slice(&target_hex);
                count_key.push(b'_');
                count_key.extend_from_slice(&skill_hash_hex);

                // Read existing count and increment
                let existing = state
                    .get_contract_storage(moltyid_pk, &count_key)
                    .ok()
                    .flatten()
                    .map(|d| {
                        if d.len() >= 8 {
                            u64::from_le_bytes([d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7]])
                        } else {
                            0
                        }
                    })
                    .unwrap_or(0);
                let new_count = (existing + 1).to_le_bytes().to_vec();
                let _ = state.put_contract_storage(moltyid_pk, &count_key, &new_count);
                attest_entries.push(AttestEntry {
                    key: count_key,
                    value: new_count,
                });

                attest_count += 1;
            }
        }

        // Also update embedded ContractAccount storage so RPC reads see it
        if attest_count > 0 {
            if let Ok(Some(yid_account)) = state.get_account(moltyid_pk) {
                if let Ok(mut yid_contract) =
                    serde_json::from_slice::<ContractAccount>(&yid_account.data)
                {
                    for entry in &attest_entries {
                        yid_contract.set_storage(entry.key.clone(), entry.value.clone());
                    }
                    if let Ok(data) = serde_json::to_vec(&yid_contract) {
                        let mut updated = yid_account;
                        updated.data = data;
                        let _ = state.put_account(moltyid_pk, &updated);
                    }
                }
            }

            info!(
                "  ATTEST {} genesis cross-attestations across {} system identities",
                attest_count,
                system_addrs.len()
            );
        }
    }

    info!("──────────────────────────────────────────────────────");
    info!(
        "  Genesis init complete: {} initialized, {} skipped",
        initialized, skipped
    );
    info!("──────────────────────────────────────────────────────");
}

// ========================================================================
//  GENESIS PHASE 3 — Create trading pairs and AMM pools at genesis.
//  Auto-lists MOLT/mUSD pair on dex_core and creates the corresponding
//  AMM pool on dex_amm.  WSOL/mUSD and WETH/mUSD are deferred until the
//  bridge & custody systems are live and tokens have real supply.
// ========================================================================

pub fn genesis_create_trading_pairs(state: &StateStore, deployer_pubkey: &Pubkey, label: &str) {
    info!("──────────────────────────────────────────────────────");
    info!("  {} Creating trading pairs & AMM pools", label);
    info!("──────────────────────────────────────────────────────");

    let admin = deployer_pubkey.0;

    // Resolve contract addresses
    let dex_core_pk = match derive_contract_address(deployer_pubkey, "dex_core") {
        Some(pk) => pk,
        None => {
            error!("  FAIL: Cannot derive dex_core address");
            return;
        }
    };
    let dex_amm_pk = match derive_contract_address(deployer_pubkey, "dex_amm") {
        Some(pk) => pk,
        None => {
            error!("  FAIL: Cannot derive dex_amm address");
            return;
        }
    };

    // Resolve token addresses
    let molt_addr = derive_contract_address(deployer_pubkey, "moltcoin")
        .map(|p| p.0)
        .unwrap_or([0u8; 32]);
    let musd_addr = derive_contract_address(deployer_pubkey, "musd_token")
        .map(|p| p.0)
        .unwrap_or([0u8; 32]);
    let wsol_addr = derive_contract_address(deployer_pubkey, "wsol_token")
        .map(|p| p.0)
        .unwrap_or([0u8; 32]);
    let weth_addr = derive_contract_address(deployer_pubkey, "weth_token")
        .map(|p| p.0)
        .unwrap_or([0u8; 32]);
    let wbnb_addr = derive_contract_address(deployer_pubkey, "wbnb_token")
        .map(|p| p.0)
        .unwrap_or([0u8; 32]);

    // Resolve dex_governance for allowed-quote setup
    let dex_gov_pk = derive_contract_address(deployer_pubkey, "dex_governance");

    // Genesis pair parameters (reasonable defaults for launch):
    // tick_size: 1 (minimum price increment in shells)
    // lot_size: 1_000_000 (minimum order lot = 0.001 tokens)
    // min_order: 1_000 (minimum order value in shells = MIN_ORDER_VALUE)
    let tick_size: u64 = 1;
    let lot_size: u64 = 1_000_000;
    let min_order: u64 = 1_000;

    // All genesis CLOB pairs: 4 mUSD-quoted + 3 MOLT-quoted = 7 pairs
    let pairs: [(&str, [u8; 32], [u8; 32]); 7] = [
        ("MOLT/mUSD", molt_addr, musd_addr),
        ("wSOL/mUSD", wsol_addr, musd_addr),
        ("wETH/mUSD", weth_addr, musd_addr),
        ("wSOL/MOLT", wsol_addr, molt_addr),
        ("wETH/MOLT", weth_addr, molt_addr),
        ("wBNB/mUSD", wbnb_addr, musd_addr),
        ("wBNB/MOLT", wbnb_addr, molt_addr),
    ];

    let mut created_pairs: usize = 0;
    let mut created_pools: usize = 0;
    let mut allowed_quotes_set: usize = 0;

    // ── Step 1: Set allowed quote tokens (mUSD + MOLT) on dex_core ──
    // opcode 21 = add_allowed_quote: [0x15][caller 32B][quote_addr 32B]
    for (sym, addr) in &[("mUSD", musd_addr), ("MOLT", molt_addr)] {
        let mut args = Vec::with_capacity(65);
        args.push(0x15); // opcode 21  = add_allowed_quote
        args.extend_from_slice(&admin);
        args.extend_from_slice(addr);

        if genesis_exec_contract(
            state,
            &dex_core_pk,
            deployer_pubkey,
            "call",
            &args,
            &format!("dex_core.add_allowed_quote({})", sym),
        ) {
            info!("  ALLOWED QUOTE {} (dex_core)", sym);
            allowed_quotes_set += 1;
        }
    }

    // ── Step 1b: Set allowed quote tokens on dex_governance too ──
    // opcode 15 = add_allowed_quote: [0x0F][caller 32B][quote_addr 32B]
    if let Some(ref gov_pk) = dex_gov_pk {
        for (sym, addr) in &[("mUSD", musd_addr), ("MOLT", molt_addr)] {
            let mut args = Vec::with_capacity(65);
            args.push(0x0F); // opcode 15 = add_allowed_quote
            args.extend_from_slice(&admin);
            args.extend_from_slice(addr);

            if genesis_exec_contract(
                state,
                gov_pk,
                deployer_pubkey,
                "call",
                &args,
                &format!("dex_governance.add_allowed_quote({})", sym),
            ) {
                info!("  ALLOWED QUOTE {} (dex_governance)", sym);
                allowed_quotes_set += 1;
            }
        }
    }

    // ── Step 2: Create CLOB trading pairs via dex_core opcode 1 (create_pair) ──
    // Args: [0x01][caller 32B][base 32B][quote 32B][tick_size 8B][lot_size 8B][min_order 8B]
    for (label, base, quote) in &pairs {
        let mut args = Vec::with_capacity(121);
        args.push(0x01); // opcode 1 = create_pair
        args.extend_from_slice(&admin); // caller
        args.extend_from_slice(base); // base_token
        args.extend_from_slice(quote); // quote_token
        args.extend_from_slice(&tick_size.to_le_bytes());
        args.extend_from_slice(&lot_size.to_le_bytes());
        args.extend_from_slice(&min_order.to_le_bytes());

        if genesis_exec_contract(
            state,
            &dex_core_pk,
            deployer_pubkey,
            "call",
            &args,
            &format!("dex_core.create_pair({})", label),
        ) {
            info!("  PAIR {}", label);
            created_pairs += 1;
        }
    }

    // ── Step 3: Create AMM pools via dex_amm opcode 1 (create_pool) ──
    // Args: [0x01][caller 32B][token_a 32B][token_b 32B][fee_tier 1B][initial_sqrt_price 8B]
    // fee_tier = 2 (30bps)
    // sqrt_price in Q32 fixed-point: value = (1 << 32) * sqrt(real_price)
    //
    // Prices are read from env vars at genesis time for accuracy.
    // Set GENESIS_SOL_USD, GENESIS_ETH_USD, GENESIS_BNB_USD, GENESIS_MOLT_USD
    // before first boot. Falls back to reasonable defaults if not set.
    let molt_usd: f64 = std::env::var("GENESIS_MOLT_USD")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.10);
    let sol_usd: f64 = std::env::var("GENESIS_SOL_USD")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(145.0);
    let eth_usd: f64 = std::env::var("GENESIS_ETH_USD")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2600.0);
    let bnb_usd: f64 = std::env::var("GENESIS_BNB_USD")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(620.0);

    info!(
        "  Genesis prices: MOLT=${:.4}, SOL=${:.2}, ETH=${:.2}, BNB=${:.2}",
        molt_usd, sol_usd, eth_usd, bnb_usd
    );

    // sqrt_price = floor(sqrt(price) * 2^32)
    let q32: f64 = (1u64 << 32) as f64;
    let sqrt_price = |price: f64| -> u64 { (price.sqrt() * q32) as u64 };

    let fee_tier: u8 = 2; // FEE_TIER_30BPS

    let pool_configs: [(&str, [u8; 32], [u8; 32], u64); 7] = [
        ("MOLT/mUSD", molt_addr, musd_addr, sqrt_price(molt_usd)),
        ("wSOL/mUSD", wsol_addr, musd_addr, sqrt_price(sol_usd)),
        ("wETH/mUSD", weth_addr, musd_addr, sqrt_price(eth_usd)),
        (
            "wSOL/MOLT",
            wsol_addr,
            molt_addr,
            sqrt_price(sol_usd / molt_usd),
        ),
        (
            "wETH/MOLT",
            weth_addr,
            molt_addr,
            sqrt_price(eth_usd / molt_usd),
        ),
        ("wBNB/mUSD", wbnb_addr, musd_addr, sqrt_price(bnb_usd)),
        (
            "wBNB/MOLT",
            wbnb_addr,
            molt_addr,
            sqrt_price(bnb_usd / molt_usd),
        ),
    ];

    for (label, token_a, token_b, sqrt_price) in &pool_configs {
        let mut args = Vec::with_capacity(106);
        args.push(0x01); // opcode 1 = create_pool
        args.extend_from_slice(&admin); // caller
        args.extend_from_slice(token_a); // token_a
        args.extend_from_slice(token_b); // token_b
        args.push(fee_tier);
        args.extend_from_slice(&sqrt_price.to_le_bytes());

        if genesis_exec_contract(
            state,
            &dex_amm_pk,
            deployer_pubkey,
            "call",
            &args,
            &format!("dex_amm.create_pool({})", label),
        ) {
            info!("  POOL {}", label);
            created_pools += 1;
        }
    }

    info!("──────────────────────────────────────────────────────");
    info!(
        "  Genesis DEX: {} pairs, {} pools, {} allowed quotes",
        created_pairs, created_pools, allowed_quotes_set
    );
    info!("──────────────────────────────────────────────────────");
}

// ========================================================================
//  GENESIS PHASE 4 — Seed Oracle Price Feeds
//  Authorizes the genesis admin as a MOLT price feeder on the moltoracle
//  contract, then submits the initial launch price ($0.10).
//  This ensures oracle-adjusted rewards work from the very first block.
// ========================================================================

pub fn genesis_seed_oracle(state: &StateStore, deployer_pubkey: &Pubkey, label: &str) {
    info!("──────────────────────────────────────────────────────");
    info!("  {} Seeding oracle price feeds", label);
    info!("──────────────────────────────────────────────────────");

    let admin = deployer_pubkey.0;

    // Resolve moltoracle contract address
    let oracle_pk = match derive_contract_address(deployer_pubkey, "moltoracle") {
        Some(pk) => pk,
        None => {
            warn!("  SKIP oracle seeding: moltoracle address not derived");
            return;
        }
    };

    // Step 1: Authorize genesis admin as MOLT price feeder
    // add_price_feeder(feeder_ptr: 32, asset_ptr: N, asset_len: u32) -> u32
    let asset = b"MOLT";
    let mut feeder_args = Vec::with_capacity(32 + asset.len() + 4);
    feeder_args.extend_from_slice(&admin); // feeder pubkey (32 bytes)
    feeder_args.extend_from_slice(asset); // asset name
    feeder_args.extend_from_slice(&(asset.len() as u32).to_le_bytes()); // asset_len

    if genesis_exec_contract(
        state,
        &oracle_pk,
        deployer_pubkey,
        "add_price_feeder",
        &feeder_args,
        "moltoracle.add_price_feeder(MOLT)",
    ) {
        info!("  FEEDER authorized: genesis admin → MOLT");
    } else {
        warn!("  SKIP feeder authorization failed");
        return;
    }

    // Step 2: Submit initial MOLT price ($0.10 with 8 decimals = 10_000_000)
    // submit_price(feeder_ptr: 32, asset_ptr: N, asset_len: u32, price: u64, decimals: u8) -> u32
    let launch_price: u64 = 10_000_000; // $0.10 with 8 decimals
    let decimals: u8 = 8;
    let mut price_args = Vec::with_capacity(32 + asset.len() + 4 + 8 + 1);
    price_args.extend_from_slice(&admin); // feeder pubkey
    price_args.extend_from_slice(asset); // asset name
    price_args.extend_from_slice(&(asset.len() as u32).to_le_bytes()); // asset_len
    price_args.extend_from_slice(&launch_price.to_le_bytes()); // price
    price_args.push(decimals); // decimals

    if genesis_exec_contract(
        state,
        &oracle_pk,
        deployer_pubkey,
        "submit_price",
        &price_args,
        "moltoracle.submit_price(MOLT=$0.10)",
    ) {
        info!("  PRICE submitted: MOLT = $0.10 (launch price)");
    } else {
        warn!("  SKIP initial price submission failed");
    }

    // ── Step 3: Seed external asset price feeds (wSOL, wETH, wBNB) ──
    // These provide reference prices for oracle-priced DEX pairs.
    // Prices read from env vars; the background WebSocket price feeder
    // will update them to live prices immediately after genesis.
    let sol_usd: f64 = std::env::var("GENESIS_SOL_USD")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(145.0);
    let eth_usd: f64 = std::env::var("GENESIS_ETH_USD")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2600.0);
    let bnb_usd: f64 = std::env::var("GENESIS_BNB_USD")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(620.0);
    let price_8dec = |usd: f64| -> u64 { (usd * 100_000_000.0) as u64 };

    let external_feeds: [(&[u8], u64, String); 3] = [
        (b"wSOL", price_8dec(sol_usd), format!("${:.2}", sol_usd)),
        (b"wETH", price_8dec(eth_usd), format!("${:.2}", eth_usd)),
        (b"wBNB", price_8dec(bnb_usd), format!("${:.2}", bnb_usd)),
    ];

    for (ext_asset, ext_price, display_price) in &external_feeds {
        // Authorize genesis admin as feeder for this asset
        let mut ext_feeder_args = Vec::with_capacity(32 + ext_asset.len() + 4);
        ext_feeder_args.extend_from_slice(&admin);
        ext_feeder_args.extend_from_slice(ext_asset);
        ext_feeder_args.extend_from_slice(&(ext_asset.len() as u32).to_le_bytes());

        let asset_name = core::str::from_utf8(ext_asset).unwrap_or("?");
        if genesis_exec_contract(
            state,
            &oracle_pk,
            deployer_pubkey,
            "add_price_feeder",
            &ext_feeder_args,
            &format!("moltoracle.add_price_feeder({})", asset_name),
        ) {
            info!("  FEEDER authorized: genesis admin → {}", asset_name);
        } else {
            warn!("  SKIP feeder auth for {} failed", asset_name);
            continue;
        }

        // Submit initial price
        let mut ext_price_args = Vec::with_capacity(32 + ext_asset.len() + 4 + 8 + 1);
        ext_price_args.extend_from_slice(&admin);
        ext_price_args.extend_from_slice(ext_asset);
        ext_price_args.extend_from_slice(&(ext_asset.len() as u32).to_le_bytes());
        ext_price_args.extend_from_slice(&ext_price.to_le_bytes());
        ext_price_args.push(decimals); // 8 decimals

        if genesis_exec_contract(
            state,
            &oracle_pk,
            deployer_pubkey,
            "submit_price",
            &ext_price_args,
            &format!("moltoracle.submit_price({}={})", asset_name, display_price),
        ) {
            info!(
                "  PRICE submitted: {} = {} (launch price)",
                asset_name, display_price
            );
        } else {
            warn!("  SKIP initial {} price submission failed", asset_name);
        }
    }

    // ── Step 4: Seed initial analytics prices for oracle-priced pairs ──
    // Write ana_lp_{pair_id} so the RPC /pairs endpoint shows prices from
    // the very first request, before the background price feeder starts.
    genesis_seed_analytics_prices(state, deployer_pubkey);

    info!("──────────────────────────────────────────────────────");
    info!("  Genesis oracle seeding complete (MOLT + wSOL + wETH + wBNB)");
    info!("──────────────────────────────────────────────────────");
}

// ========================================================================
//  GENESIS PHASE 4b — Seed initial analytics prices for oracle-priced pairs
//  Writes ana_lp_{pair_id} and ana_24h_{pair_id} directly to dex_analytics
//  contract storage so that RPC /pairs and /tickers endpoints return valid
//  prices immediately, before any trades occur or the live feeder starts.
// ========================================================================

pub fn genesis_seed_analytics_prices(state: &StateStore, deployer_pubkey: &Pubkey) {
    let analytics_pk = match derive_contract_address(deployer_pubkey, "dex_analytics") {
        Some(pk) => pk,
        None => {
            warn!("  SKIP analytics price seeding: dex_analytics not derived");
            return;
        }
    };

    const PRICE_SCALE: u64 = 1_000_000_000;

    // Pair IDs match genesis_create_trading_pairs order:
    //   1=MOLT/mUSD, 2=wSOL/mUSD, 3=wETH/mUSD, 4=wSOL/MOLT, 5=wETH/MOLT,
    //   6=wBNB/mUSD, 7=wBNB/MOLT
    let molt_usd: f64 = std::env::var("GENESIS_MOLT_USD")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.10);
    let wsol_usd: f64 = std::env::var("GENESIS_SOL_USD")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(145.0);
    let weth_usd: f64 = std::env::var("GENESIS_ETH_USD")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2600.0);
    let wbnb_usd: f64 = std::env::var("GENESIS_BNB_USD")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(620.0);

    let pair_prices: [(u64, f64); 7] = [
        (1, molt_usd),
        (2, wsol_usd),
        (3, weth_usd),
        (4, wsol_usd / molt_usd),
        (5, weth_usd / molt_usd),
        (6, wbnb_usd),
        (7, wbnb_usd / molt_usd),
    ];

    for (pair_id, price_f64) in &pair_prices {
        let price_scaled = (*price_f64 * PRICE_SCALE as f64) as u64;

        // Write last price: ana_lp_{pair_id}
        let lp_key = format!("ana_lp_{}", pair_id);
        let _ = state.put_contract_storage(
            &analytics_pk,
            lp_key.as_bytes(),
            &price_scaled.to_le_bytes(),
        );

        // Write 24h stats: ana_24h_{pair_id} (48 bytes)
        // Layout: volume(8) + high(8) + low(8) + open(8) + close(8) + trades(8)
        let mut stats = Vec::with_capacity(48);
        stats.extend_from_slice(&0u64.to_le_bytes()); // volume = 0
        stats.extend_from_slice(&price_scaled.to_le_bytes()); // high = price
        stats.extend_from_slice(&price_scaled.to_le_bytes()); // low = price (not u64::MAX for new pair)
        stats.extend_from_slice(&price_scaled.to_le_bytes()); // open = price
        stats.extend_from_slice(&price_scaled.to_le_bytes()); // close = price
        stats.extend_from_slice(&0u64.to_le_bytes()); // trades = 0
        let stats_key = format!("ana_24h_{}", pair_id);
        let _ = state.put_contract_storage(&analytics_pk, stats_key.as_bytes(), &stats);

        info!("  ANA seeded: pair {} → price {:.4}", pair_id, price_f64);
    }

    // Also write initial candles for each pair so TradingView has data
    // Use unix timestamp for the candle period start, matching the oracle feeder
    let genesis_ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // All 9 intervals so every TF has a seed candle
    let all_intervals: [u64; 9] = [60, 300, 900, 3600, 14400, 86400, 259200, 604800, 31536000];
    for (pair_id, price_f64) in &pair_prices {
        let price_scaled = (*price_f64 * PRICE_SCALE as f64) as u64;

        // Candle layout: open(8)+high(8)+low(8)+close(8)+volume(8)+timestamp(8) = 48 bytes
        let mut candle = Vec::with_capacity(48);
        candle.extend_from_slice(&price_scaled.to_le_bytes()); // open
        candle.extend_from_slice(&price_scaled.to_le_bytes()); // high
        candle.extend_from_slice(&price_scaled.to_le_bytes()); // low
        candle.extend_from_slice(&price_scaled.to_le_bytes()); // close
        candle.extend_from_slice(&0u64.to_le_bytes()); // volume
                                                       // timestamp placeholder — overwritten per-interval below
        candle.extend_from_slice(&0u64.to_le_bytes());

        for interval in &all_intervals {
            let candle_start = (genesis_ts / interval) * interval;
            // Store period-start time so TradingView bars align to boundaries
            candle[40..48].copy_from_slice(&candle_start.to_le_bytes());
            let candle_key = format!("ana_c_{}_{}_{}", pair_id, interval, 0);
            let _ = state.put_contract_storage(&analytics_pk, candle_key.as_bytes(), &candle);
            // Set candle count to 1
            let count_key = format!("ana_cc_{}_{}", pair_id, interval);
            let _ = state.put_contract_storage(
                &analytics_pk,
                count_key.as_bytes(),
                &1u64.to_le_bytes(),
            );
            // Set current candle start to the timestamp-based period
            let cur_key = format!("ana_cur_{}_{}", pair_id, interval);
            let _ = state.put_contract_storage(
                &analytics_pk,
                cur_key.as_bytes(),
                &candle_start.to_le_bytes(),
            );
        }
    }
}

// ========================================================================
//  GENESIS PHASE 4c — Seed dex_margin mark/index prices & enable pairs
//  Writes mrg_mark_{pair_id}, mrg_idx_{pair_id}, mrg_ena_{pair_id} directly
//  to dex_margin contract storage so margin trading works from genesis.
//  Prices match the oracle seeds (MOLT=$0.10, wSOL=$82, wETH=$1,979, wBNB=$300).
// ========================================================================

pub fn genesis_seed_margin_prices(state: &StateStore, deployer_pubkey: &Pubkey) {
    let margin_pk = match derive_contract_address(deployer_pubkey, "dex_margin") {
        Some(pk) => pk,
        None => {
            warn!("  SKIP margin price seeding: dex_margin not derived");
            return;
        }
    };

    const PRICE_SCALE: u64 = 1_000_000_000;
    let molt_usd: f64 = std::env::var("GENESIS_MOLT_USD")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.10);
    let wsol_usd: f64 = std::env::var("GENESIS_SOL_USD")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(145.0);
    let weth_usd: f64 = std::env::var("GENESIS_ETH_USD")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2600.0);
    let wbnb_usd: f64 = std::env::var("GENESIS_BNB_USD")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(620.0);

    // Pair IDs match genesis_create_trading_pairs order:
    //   1=MOLT/mUSD, 2=wSOL/mUSD, 3=wETH/mUSD, 4=wSOL/MOLT, 5=wETH/MOLT,
    //   6=wBNB/mUSD, 7=wBNB/MOLT
    let pair_prices: [(u64, f64); 7] = [
        (1, molt_usd),
        (2, wsol_usd),
        (3, weth_usd),
        (4, wsol_usd / molt_usd),
        (5, weth_usd / molt_usd),
        (6, wbnb_usd),
        (7, wbnb_usd / molt_usd),
    ];

    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    // Collect all storage writes to also update embedded ContractAccount
    let mut margin_entries: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();

    for (pair_id, price_f64) in &pair_prices {
        let price_scaled = (*price_f64 * PRICE_SCALE as f64) as u64;

        // Mark price: mrg_mark_{pair_id} → [price 8B LE][timestamp 8B LE]
        let mark_key = format!("mrg_mark_{}", pair_id);
        let mut mark_val = Vec::with_capacity(16);
        mark_val.extend_from_slice(&price_scaled.to_le_bytes());
        mark_val.extend_from_slice(&now_secs.to_le_bytes());
        let _ = state.put_contract_storage(&margin_pk, mark_key.as_bytes(), &mark_val);
        margin_entries.push((mark_key.into_bytes(), mark_val));

        // Index price: mrg_idx_{pair_id} → [price 8B LE][timestamp 8B LE]
        let idx_key = format!("mrg_idx_{}", pair_id);
        let mut idx_val = Vec::with_capacity(16);
        idx_val.extend_from_slice(&price_scaled.to_le_bytes());
        idx_val.extend_from_slice(&now_secs.to_le_bytes());
        let _ = state.put_contract_storage(&margin_pk, idx_key.as_bytes(), &idx_val);
        margin_entries.push((idx_key.into_bytes(), idx_val));

        // Enable margin trading: mrg_ena_{pair_id} → [1u64 LE]
        let ena_key = format!("mrg_ena_{}", pair_id);
        let ena_val = 1u64.to_le_bytes().to_vec();
        let _ = state.put_contract_storage(&margin_pk, ena_key.as_bytes(), &ena_val);
        margin_entries.push((ena_key.into_bytes(), ena_val));

        info!(
            "  MARGIN seeded: pair {} → price {:.4}, mark+index+enabled",
            pair_id, price_f64
        );
    }

    // Also update embedded ContractAccount storage so RPC reads see it
    if let Ok(Some(margin_account)) = state.get_account(&margin_pk) {
        if let Ok(mut margin_contract) =
            serde_json::from_slice::<ContractAccount>(&margin_account.data)
        {
            for (key, value) in &margin_entries {
                margin_contract.set_storage(key.clone(), value.clone());
            }
            if let Ok(data) = serde_json::to_vec(&margin_contract) {
                let mut updated = margin_account;
                updated.data = data;
                let _ = state.put_account(&margin_pk, &updated);
            }
        }
    }
}
