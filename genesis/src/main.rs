//! MoltChain Genesis — standalone one-time genesis block creator.
//!
//! Usage:
//!   moltchain-genesis --network testnet --db-path /var/lib/moltchain/state-testnet
//!   moltchain-genesis --network mainnet --db-path /var/lib/moltchain/state-mainnet

use moltchain_core::{
    Account, Block, FeeConfig, GenesisConfig, GenesisWallet, Hash, Instruction, Keypair,
    Message, Pubkey, StateStore, Transaction,
    CONTRACT_DEPLOY_FEE, CONTRACT_UPGRADE_FEE, NFT_MINT_FEE, NFT_COLLECTION_FEE,
    SYSTEM_PROGRAM_ID,
};
use moltchain_core::consensus::{FOUNDING_CLIFF_SECONDS, FOUNDING_VEST_TOTAL_SECONDS};
use moltchain_core::multisig::GovernedWalletConfig;
use moltchain_genesis::{
    genesis_auto_deploy, genesis_create_trading_pairs, genesis_initialize_contracts,
    genesis_seed_analytics_prices, genesis_seed_margin_prices, genesis_seed_oracle,
};
use std::path::PathBuf;
use tracing::{error, info, warn};

const SYSTEM_ACCOUNT_OWNER: Pubkey = Pubkey([0x01; 32]);
const GENESIS_MINT_PUBKEY: Pubkey = Pubkey([0xFE; 32]);
const REWARD_POOL_MOLT: u64 = 100_000_000;

fn main() {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args: Vec<String> = std::env::args().collect();

    // Parse --network
    let network = args
        .iter()
        .position(|a| a == "--network")
        .and_then(|pos| args.get(pos + 1))
        .map(|s| s.as_str());

    // Parse --db-path
    let db_path = args
        .iter()
        .position(|a| a == "--db-path")
        .and_then(|pos| args.get(pos + 1))
        .cloned();

    let network_str = match network {
        Some(n @ ("mainnet" | "testnet")) => n,
        Some(other) => {
            error!("Unknown network '{}'. Use --network mainnet or --network testnet", other);
            std::process::exit(1);
        }
        None => {
            error!("Usage: moltchain-genesis --network <mainnet|testnet> [--db-path <path>]");
            std::process::exit(1);
        }
    };

    let db_dir = db_path.unwrap_or_else(|| format!("./data/state-genesis-{}", network_str));
    let db_dir_path = PathBuf::from(&db_dir);

    // Create data directory if needed
    if let Err(e) = std::fs::create_dir_all(&db_dir_path) {
        error!("Failed to create data directory {}: {}", db_dir, e);
        std::process::exit(1);
    }

    info!("═══════════════════════════════════════════════════════");
    info!("  MoltChain Genesis — One-Time Chain Initialization");
    info!("═══════════════════════════════════════════════════════");
    info!("  Network:    {}", network_str);
    info!("  DB path:    {}", db_dir);
    info!("═══════════════════════════════════════════════════════");

    // Open state store
    let state = match StateStore::open(&db_dir) {
        Ok(s) => s,
        Err(e) => {
            error!("Failed to open state database at {}: {}", db_dir, e);
            std::process::exit(1);
        }
    };

    // Check if genesis already exists — refuse to overwrite
    if state.get_block_by_slot(0).unwrap_or(None).is_some() {
        error!("Genesis block already exists in {}. Refusing to overwrite.", db_dir);
        error!("To create a new genesis, delete or move the existing database first.");
        std::process::exit(1);
    }

    // Load genesis configuration
    let genesis_config = match network_str {
        "mainnet" => GenesisConfig::default_mainnet(),
        "testnet" | _ => GenesisConfig::default_testnet(),
    };
    info!("Chain ID: {}", genesis_config.chain_id);
    info!("Total supply: {} MOLT", genesis_config.total_supply_molt());

    // Genesis wallet + keypairs directory
    let genesis_wallet_path = db_dir_path.join("genesis-wallet.json");
    let genesis_keypairs_dir = db_dir_path.join("genesis-keys");
    std::fs::create_dir_all(&genesis_keypairs_dir).ok();

    // ════════════════════════════════════════════════════════════════════
    // GENERATE GENESIS WALLET
    // ════════════════════════════════════════════════════════════════════
    info!("🔐 Generating FRESH genesis wallet (DYNAMIC GENERATION)");

    let is_mainnet = genesis_config.chain_id.contains("mainnet");
    let (signer_count, threshold_desc) = if is_mainnet {
        (5, "3/5 production multi-sig")
    } else {
        (3, "2/3 testnet multi-sig")
    };

    info!("  🔐 Creating {} setup...", threshold_desc);

    let (wallet, keypairs, distribution_keypairs) =
        match GenesisWallet::generate(&genesis_config.chain_id, is_mainnet, signer_count) {
            Ok(result) => result,
            Err(err) => {
                error!("Failed to generate genesis wallet: {}", err);
                std::process::exit(1);
            }
        };

    let genesis_signer = match keypairs.first() {
        Some(kp) => Keypair::from_seed(&kp.to_seed()),
        None => {
            error!("No keypairs generated");
            std::process::exit(1);
        }
    };

    let genesis_pubkey = wallet.pubkey;
    info!("  ✓ Generated genesis pubkey: {}", genesis_pubkey.to_base58());

    if let Some(ref multisig) = wallet.multisig {
        info!("  ✓ Multi-sig configuration:");
        info!("    - Threshold: {}/{} signatures", multisig.threshold, multisig.signers.len());
        info!("    - Genesis treasury: {}", multisig.is_genesis);
        info!("    - Signers:");
        for (i, signer) in multisig.signers.iter().enumerate() {
            info!("      {}. {}", i + 1, signer.to_base58());
        }
    }

    // Log whitepaper distribution
    if let Some(ref dist) = wallet.distribution_wallets {
        info!("  📊 Whitepaper genesis distribution ({} wallets):", dist.len());
        for dw in dist {
            info!("    - {} ({}%): {} MOLT → {}", dw.role, dw.percentage, dw.amount_molt, dw.pubkey.to_base58());
        }
    }

    // Save wallet info
    if let Err(err) = wallet.save(&genesis_wallet_path) {
        error!("Failed to save genesis wallet: {}", err);
        std::process::exit(1);
    }
    info!("  ✓ Wallet info saved: {}", genesis_wallet_path.display());

    // Save signer keypairs
    let keypair_paths = match GenesisWallet::save_keypairs(
        &keypairs,
        &genesis_keypairs_dir,
        &genesis_config.chain_id,
    ) {
        Ok(paths) => paths,
        Err(err) => {
            error!("Failed to save keypairs: {}", err);
            std::process::exit(1);
        }
    };

    // Save distribution keypairs
    let dist_keypair_paths = match GenesisWallet::save_distribution_keypairs(
        wallet.distribution_wallets.as_deref().unwrap_or(&[]),
        &distribution_keypairs,
        &genesis_keypairs_dir,
        &genesis_config.chain_id,
    ) {
        Ok(paths) => paths,
        Err(err) => {
            error!("Failed to save distribution keypairs: {}", err);
            std::process::exit(1);
        }
    };

    // Save treasury keypair separately
    let treasury_seed_keypair = match distribution_keypairs.first() {
        Some(keypair) => keypair,
        None => {
            error!("Missing distribution keypair for treasury");
            std::process::exit(1);
        }
    };
    let treasury_keypair_path = match GenesisWallet::save_treasury_keypair(
        treasury_seed_keypair,
        &genesis_keypairs_dir,
        &genesis_config.chain_id,
    ) {
        Ok(path) => path,
        Err(err) => {
            error!("Failed to save treasury keypair: {}", err);
            std::process::exit(1);
        }
    };

    info!("  ✓ Saved {} signer keypair(s):", keypair_paths.len());
    for path in &keypair_paths {
        info!("    - {}", path);
    }
    info!("  ✓ Saved {} distribution keypair(s):", dist_keypair_paths.len());
    for path in &dist_keypair_paths {
        info!("    - {}", path);
    }
    info!("  ✓ Treasury keypair: {}", treasury_keypair_path);
    info!("  ⚠️  KEEP THESE FILES SECURE - THEY CONTROL THE GENESIS TREASURY");

    // ════════════════════════════════════════════════════════════════════
    // CREATE GENESIS STATE
    // ════════════════════════════════════════════════════════════════════
    info!("📦 Creating genesis state...");

    // Store rent params
    if let Err(e) = state.set_rent_params(
        genesis_config.features.rent_rate_shells_per_kb_month,
        genesis_config.features.rent_free_kb,
    ) {
        warn!("⚠️  Failed to store rent params: {}", e);
    }

    // Store fee configuration
    let genesis_fee_config = FeeConfig {
        base_fee: genesis_config.features.base_fee_shells,
        contract_deploy_fee: CONTRACT_DEPLOY_FEE,
        contract_upgrade_fee: CONTRACT_UPGRADE_FEE,
        nft_mint_fee: NFT_MINT_FEE,
        nft_collection_fee: NFT_COLLECTION_FEE,
        fee_burn_percent: genesis_config.features.fee_burn_percentage,
        fee_producer_percent: genesis_config.features.fee_producer_percentage,
        fee_voters_percent: genesis_config.features.fee_voters_percentage,
        fee_community_percent: genesis_config.features.fee_community_percentage,
        fee_treasury_percent: 100u64
            .saturating_sub(genesis_config.features.fee_burn_percentage)
            .saturating_sub(genesis_config.features.fee_producer_percentage)
            .saturating_sub(genesis_config.features.fee_voters_percentage)
            .saturating_sub(genesis_config.features.fee_community_percentage),
    };
    if let Err(e) = state.set_fee_config_full(&genesis_fee_config) {
        warn!("⚠️  Failed to store fee config: {}", e);
    } else {
        info!("  ✓ Fee config persisted: base={} shells, burn={}%, producer={}%, voters={}%, treasury={}%, community={}%",
            genesis_fee_config.base_fee,
            genesis_fee_config.fee_burn_percent,
            genesis_fee_config.fee_producer_percent,
            genesis_fee_config.fee_voters_percent,
            genesis_fee_config.fee_treasury_percent,
            genesis_fee_config.fee_community_percent,
        );
    }

    // Persist slot_duration_ms
    let slot_ms = genesis_config.consensus.slot_duration_ms.max(1);
    if let Err(e) = state.set_slot_duration_ms(slot_ms) {
        warn!("⚠️  Failed to store slot_duration_ms: {}", e);
    } else {
        info!("  ✓ slot_duration_ms persisted: {}ms", slot_ms);
    }

    // Create genesis treasury account with full supply
    let total_supply_molt = 1_000_000_000u64;
    let mut genesis_account = Account::new(total_supply_molt, genesis_pubkey);

    if let Some(ref multisig) = wallet.multisig {
        genesis_account.owner = genesis_pubkey;
        info!("  ✓ Flagged as genesis treasury with multi-sig");
        info!("    Threshold: {}/{} signatures", multisig.threshold, multisig.signers.len());
    }

    if let Err(e) = state.put_account(&genesis_pubkey, &genesis_account) {
        error!("Failed to store genesis account: {e}");
        std::process::exit(1);
    }
    if let Err(e) = state.set_genesis_pubkey(&genesis_pubkey) {
        error!("Failed to set genesis pubkey: {e}");
        std::process::exit(1);
    }
    info!("  ✓ Genesis mint: {} MOLT", total_supply_molt);
    info!("  ✓ Address: {}", genesis_pubkey.to_base58());

    // ════════════════════════════════════════════════════════════════════
    // WHITEPAPER GENESIS DISTRIBUTION
    // ════════════════════════════════════════════════════════════════════
    let mut genesis_txs = Vec::new();

    if let Some(ref dist_wallets) = wallet.distribution_wallets {
        info!("📊 Creating whitepaper genesis distribution:");

        let mut src_acct = match state.get_account(&genesis_pubkey).ok().flatten() {
            Some(a) => a,
            None => {
                error!("Genesis account missing after creation — cannot distribute");
                std::process::exit(1);
            }
        };

        for dw in dist_wallets {
            let amount_shells = Account::molt_to_shells(dw.amount_molt);

            let mut acct = Account::new(0, SYSTEM_ACCOUNT_OWNER);
            acct.shells = amount_shells;

            if dw.role == "founding_moltys" {
                acct.spendable = 0;
                acct.locked = amount_shells;
            } else {
                acct.spendable = amount_shells;
            }

            if let Err(e) = state.put_account(&dw.pubkey, &acct) {
                error!("Failed to create {} account: {e}", dw.role);
            }

            src_acct.shells = src_acct.shells.saturating_sub(amount_shells);
            src_acct.spendable = src_acct.spendable.saturating_sub(amount_shells);

            if dw.role == "validator_rewards" {
                if let Err(e) = state.set_treasury_pubkey(&dw.pubkey) {
                    error!("Failed to set treasury pubkey: {e}");
                }
                info!("  ✓ {} ({}%): {} MOLT → {} [TREASURY]", dw.role, dw.percentage, dw.amount_molt, dw.pubkey.to_base58());
            } else if dw.role == "founding_moltys" {
                info!("  ✓ {} ({}%): {} MOLT → {} [LOCKED — 6mo cliff + 18mo vest]", dw.role, dw.percentage, dw.amount_molt, dw.pubkey.to_base58());
            } else {
                info!("  ✓ {} ({}%): {} MOLT → {}", dw.role, dw.percentage, dw.amount_molt, dw.pubkey.to_base58());
            }
        }

        if let Err(e) = state.put_account(&genesis_pubkey, &src_acct) {
            error!("Failed to update genesis account after distribution: {e}");
        }

        // Store genesis accounts in state DB
        let ga_entries: Vec<(String, Pubkey, u64, u8)> = dist_wallets
            .iter()
            .map(|dw| (dw.role.clone(), dw.pubkey, dw.amount_molt, dw.percentage))
            .collect();
        if let Err(e) = state.set_genesis_accounts(&ga_entries) {
            error!("Failed to store genesis accounts in DB: {e}");
        } else {
            info!("  ✓ Stored {} genesis accounts in state DB", ga_entries.len());
        }

        info!("  ✓ Genesis distribution complete — 1B MOLT allocated per whitepaper");

        // Governed wallet configs for multi-sig spending
        {
            let mut all_signers: Vec<Pubkey> = dist_wallets
                .iter()
                .filter(|dw| dw.keypair_path.is_some())
                .map(|dw| dw.pubkey)
                .collect();
            if !all_signers.contains(&genesis_pubkey) {
                all_signers.push(genesis_pubkey);
            }

            for dw in dist_wallets.iter() {
                if dw.role == "ecosystem_partnerships" {
                    let config = GovernedWalletConfig::new(2, all_signers.clone(), "ecosystem_partnerships");
                    if let Err(e) = state.set_governed_wallet_config(&dw.pubkey, &config) {
                        error!("Failed to store ecosystem_partnerships governed config: {e}");
                    } else {
                        info!("  ✓ ecosystem_partnerships governed wallet: threshold={}, {} signers", config.threshold, config.signers.len());
                    }
                } else if dw.role == "reserve_pool" {
                    let config = GovernedWalletConfig::new(3, all_signers.clone(), "reserve_pool");
                    if let Err(e) = state.set_governed_wallet_config(&dw.pubkey, &config) {
                        error!("Failed to store reserve_pool governed config: {e}");
                    } else {
                        info!("  ✓ reserve_pool governed wallet: threshold={}, {} signers [SUPERMAJORITY]", config.threshold, config.signers.len());
                    }
                }
            }
        }

        // Auto-fund genesis/deployer with 10K MOLT from treasury
        let ops_fund_molt: u64 = 10_000;
        let ops_fund_shells = Account::molt_to_shells(ops_fund_molt);
        if let Some(treasury_dw) = dist_wallets.iter().find(|dw| dw.role == "validator_rewards") {
            let mut treasury_acct = state
                .get_account(&treasury_dw.pubkey)
                .ok()
                .flatten()
                .unwrap_or_else(|| Account::new(0, SYSTEM_ACCOUNT_OWNER));
            if treasury_acct.spendable >= ops_fund_shells {
                treasury_acct.deduct_spendable(ops_fund_shells).ok();
                if let Err(e) = state.put_account(&treasury_dw.pubkey, &treasury_acct) {
                    error!("Failed to debit treasury for auto-fund: {e}");
                }

                let mut genesis_acct = state
                    .get_account(&genesis_pubkey)
                    .ok()
                    .flatten()
                    .unwrap_or_else(|| Account::new(0, genesis_pubkey));
                genesis_acct.add_spendable(ops_fund_shells).ok();
                if let Err(e) = state.put_account(&genesis_pubkey, &genesis_acct) {
                    error!("Failed to credit deployer for auto-fund: {e}");
                }

                info!("  ✓ Auto-funded genesis/deployer with {} MOLT from treasury", ops_fund_molt);
            } else {
                warn!("  ⚠️  Treasury has insufficient funds for deployer auto-fund");
            }
        }

        // Auto-generate & fund faucet keypair
        let faucet_fund_molt: u64 = 100_000;
        let faucet_fund_shells = Account::molt_to_shells(faucet_fund_molt);
        let faucet_kp = Keypair::generate();
        let faucet_pubkey = faucet_kp.pubkey();
        let faucet_keypair_path = genesis_keypairs_dir.join(format!("faucet-{}.json", genesis_config.chain_id));
        let faucet_seed = faucet_kp.to_seed();
        let faucet_seed_json = serde_json::json!({
            "seed": hex::encode(faucet_seed),
            "pubkey": faucet_pubkey.to_base58(),
            "role": "faucet"
        });
        if let Err(e) = std::fs::write(
            &faucet_keypair_path,
            serde_json::to_string_pretty(&faucet_seed_json).unwrap_or_default(),
        ) {
            error!("Failed to save faucet keypair: {e}");
        }
        if let Some(treasury_dw) = dist_wallets.iter().find(|dw| dw.role == "validator_rewards") {
            let mut treasury_acct = state
                .get_account(&treasury_dw.pubkey)
                .ok()
                .flatten()
                .unwrap_or_else(|| Account::new(0, SYSTEM_ACCOUNT_OWNER));
            if treasury_acct.spendable >= faucet_fund_shells {
                treasury_acct.deduct_spendable(faucet_fund_shells).ok();
                if let Err(e) = state.put_account(&treasury_dw.pubkey, &treasury_acct) {
                    error!("Failed to debit treasury for faucet fund: {e}");
                }
                let mut faucet_acct = Account::new(0, faucet_pubkey);
                faucet_acct.add_spendable(faucet_fund_shells).ok();
                if let Err(e) = state.put_account(&faucet_pubkey, &faucet_acct) {
                    error!("Failed to credit faucet account: {e}");
                }
                info!("  ✓ Auto-funded faucet with {} MOLT → {} (keypair: {})", faucet_fund_molt, faucet_pubkey.to_base58(), faucet_keypair_path.display());
            } else {
                warn!("  ⚠️  Treasury has insufficient funds for faucet auto-fund");
            }
        }

        // Build distribution transactions for genesis block
        for dw in dist_wallets {
            let mut data = Vec::with_capacity(9);
            data.push(4); // Genesis transfer (fee-free)
            data.extend_from_slice(&Account::molt_to_shells(dw.amount_molt).to_le_bytes());

            let instruction = Instruction {
                program_id: SYSTEM_PROGRAM_ID,
                accounts: vec![genesis_pubkey, dw.pubkey],
                data,
            };

            let message = Message::new(vec![instruction], Hash::default());
            let mut tx = Transaction::new(message.clone());
            let signature = genesis_signer.sign(&message.serialize());
            tx.signatures.push(signature);
            genesis_txs.push(tx);
        }
    }
    // Legacy: single treasury (backward compat)
    else if let Some(treasury_pubkey) = wallet.treasury_pubkey {
        let reward_pool_molt = REWARD_POOL_MOLT.min(1_000_000_000);
        let treasury_account = Account::new(0, SYSTEM_ACCOUNT_OWNER);
        if let Err(e) = state.put_account(&treasury_pubkey, &treasury_account) {
            error!("Failed to store treasury account: {e}");
        }
        if let Err(e) = state.set_treasury_pubkey(&treasury_pubkey) {
            error!("Failed to set treasury pubkey: {e}");
        }
        info!("  ✓ Treasury account created: {}", treasury_pubkey.to_base58());
        info!("  ✓ Reward pool pending: {} MOLT", reward_pool_molt);

        let reward_shells = Account::molt_to_shells(reward_pool_molt);

        let mut src_acct = match state.get_account(&genesis_pubkey).ok().flatten() {
            Some(a) => a,
            None => {
                error!("Genesis account missing — cannot fund treasury");
                Account::new(0, genesis_pubkey)
            }
        };
        src_acct.shells = src_acct.shells.saturating_sub(reward_shells);
        src_acct.spendable = src_acct.spendable.saturating_sub(reward_shells);
        if let Err(e) = state.put_account(&genesis_pubkey, &src_acct) {
            error!("Failed to update genesis account balance: {e}");
        }

        let mut trs_acct = state
            .get_account(&treasury_pubkey)
            .ok()
            .flatten()
            .unwrap_or_else(|| Account::new(0, SYSTEM_ACCOUNT_OWNER));
        trs_acct.shells = trs_acct.shells.saturating_add(reward_shells);
        trs_acct.spendable = trs_acct.spendable.saturating_add(reward_shells);
        if let Err(e) = state.put_account(&treasury_pubkey, &trs_acct) {
            error!("Failed to update treasury account balance: {e}");
        }

        info!("  ✓ Reward pool funded via genesis transfer tx");

        // Legacy treasury transaction
        let mut data = Vec::with_capacity(9);
        data.push(4); // Genesis transfer (fee-free)
        data.extend_from_slice(&Account::molt_to_shells(reward_pool_molt).to_le_bytes());

        let instruction = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![genesis_pubkey, treasury_pubkey],
            data,
        };

        let message = Message::new(vec![instruction], Hash::default());
        let mut treasury_tx = Transaction::new(message.clone());
        let signature = genesis_signer.sign(&message.serialize());
        treasury_tx.signatures.push(signature);
        genesis_txs.push(treasury_tx);
    }

    // Create initial accounts from genesis config
    for account_info in &genesis_config.initial_accounts {
        let pubkey = match Pubkey::from_base58(&account_info.address) {
            Ok(pk) => pk,
            Err(e) => {
                warn!("Skipping initial account with invalid address {}: {e}", account_info.address);
                continue;
            }
        };
        let account = Account::new(account_info.balance_molt, pubkey);
        if let Err(e) = state.put_account(&pubkey, &account) {
            error!("Failed to store initial account: {e}");
        }
        info!("  ✓ Account {}: {} MOLT", &account_info.address[..20.min(account_info.address.len())], account_info.balance_molt);
    }

    // Mint transaction
    let mint_shells = Account::molt_to_shells(total_supply_molt);
    let mut mint_data = Vec::with_capacity(9);
    mint_data.push(5); // Genesis mint (synthetic, fee-free)
    mint_data.extend_from_slice(&mint_shells.to_le_bytes());

    let mint_instruction = Instruction {
        program_id: SYSTEM_PROGRAM_ID,
        accounts: vec![GENESIS_MINT_PUBKEY, genesis_pubkey],
        data: mint_data,
    };

    let mint_message = Message::new(vec![mint_instruction], Hash::default());
    let mut mint_tx = Transaction::new(mint_message);
    mint_tx.signatures.push([0u8; 64]);

    // Insert mint tx at the beginning
    genesis_txs.insert(0, mint_tx);

    // ════════════════════════════════════════════════════════════════════
    // CREATE GENESIS BLOCK
    // ════════════════════════════════════════════════════════════════════
    let state_root = state.compute_state_root();
    let genesis_timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let genesis_block = Block::genesis(state_root, genesis_timestamp, genesis_txs);
    if let Err(e) = state.put_block(&genesis_block) {
        error!("Failed to store genesis block: {e}");
        std::process::exit(1);
    }
    if let Err(e) = state.set_last_slot(0) {
        error!("Failed to set initial slot: {e}");
        std::process::exit(1);
    }
    info!("✓ Genesis block created and stored (slot 0)");
    info!("  Genesis hash: {}", genesis_block.hash());

    // Store founding moltys vesting schedule
    if let Some(fm_dw) = wallet
        .distribution_wallets
        .as_ref()
        .and_then(|ws| ws.iter().find(|dw| dw.role == "founding_moltys"))
    {
        let cliff_end = genesis_timestamp + FOUNDING_CLIFF_SECONDS;
        let vest_end = genesis_timestamp + FOUNDING_VEST_TOTAL_SECONDS;
        let total_shells = Account::molt_to_shells(fm_dw.amount_molt);
        if let Err(e) = state.set_founding_vesting_params(cliff_end, vest_end, total_shells) {
            error!("Failed to store founding vesting params: {e}");
        } else {
            info!("  ✓ Founding moltys vesting: cliff={}, vest_end={}, total={}M MOLT",
                cliff_end, vest_end, fm_dw.amount_molt / 1_000_000);
        }
    }

    // ════════════════════════════════════════════════════════════════════
    // AUTO-DEPLOY CONTRACTS
    // ════════════════════════════════════════════════════════════════════
    genesis_auto_deploy(&state, &genesis_pubkey, "GENESIS:");
    genesis_initialize_contracts(&state, &genesis_pubkey, "GENESIS:");
    genesis_create_trading_pairs(&state, &genesis_pubkey, "GENESIS:");
    genesis_seed_oracle(&state, &genesis_pubkey, "GENESIS:");
    genesis_seed_margin_prices(&state, &genesis_pubkey);
    genesis_seed_analytics_prices(&state, &genesis_pubkey);

    // Flush metrics counters to disk — contract deploy (index_program) and
    // any accounts created after the genesis block was stored need their
    // counters persisted so the validator reads correct values on startup.
    if let Err(e) = state.flush_metrics() {
        error!("Failed to flush metrics after contract deployment: {}", e);
    }

    info!("═══════════════════════════════════════════════════════");
    info!("  ✅ Genesis creation complete!");
    info!("  Database: {}", db_dir);
    info!("  Genesis pubkey: {}", genesis_pubkey.to_base58());
    info!("  Genesis hash: {}", genesis_block.hash());
    info!("═══════════════════════════════════════════════════════");
    info!("  Next: start the validator pointing at this DB:");
    info!("    moltchain-validator --network {} --db-path {}", network_str, db_dir);
    info!("═══════════════════════════════════════════════════════");
}
