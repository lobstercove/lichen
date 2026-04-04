//! Lichen Genesis — standalone one-time genesis block creator.
//!
//! Usage:
//!   lichen-genesis --prepare-wallet --network testnet --output-dir ./artifacts/testnet
//!   lichen-genesis --network testnet --wallet-file ./artifacts/testnet/genesis-wallet.json --initial-validator <base58> --db-path /var/lib/lichen/state-testnet

use lichen_core::consensus::{
    StakePool, BOOTSTRAP_GRANT_AMOUNT, FOUNDING_CLIFF_SECONDS, FOUNDING_VEST_TOTAL_SECONDS,
};
use lichen_core::multisig::{
    bridge_committee_admin_config_for_roles, governed_wallet_config_for_role,
    incident_guardian_config_for_roles, oracle_committee_admin_config_for_roles,
    treasury_executor_config_for_roles, upgrade_proposer_config_for_roles,
    upgrade_veto_guardian_config_for_roles,
};
use lichen_core::{
    Account, Block, FeeConfig, GenesisConfig, GenesisPrices, GenesisValidator, GenesisWallet, Hash,
    Instruction, Keypair, Message, Pubkey, StateStore, Transaction, CONTRACT_DEPLOY_FEE,
    CONTRACT_UPGRADE_FEE, NFT_COLLECTION_FEE, NFT_MINT_FEE, SYSTEM_PROGRAM_ID,
};
use lichen_genesis::{
    genesis_assign_achievements, genesis_auto_deploy, genesis_create_trading_pairs,
    genesis_harden_contract_controls, genesis_initialize_contracts, genesis_seed_analytics_prices,
    genesis_seed_margin_prices, genesis_seed_oracle, genesis_set_fee_exempt_contracts,
};
use std::path::PathBuf;
use tracing::{error, info, warn};

const SYSTEM_ACCOUNT_OWNER: Pubkey = Pubkey([0x01; 32]);
const GENESIS_MINT_PUBKEY: Pubkey = Pubkey([0xFE; 32]);

#[derive(serde::Deserialize)]
struct GenesisKeypairFile {
    #[serde(rename = "privateKey")]
    private_key: Vec<u8>,
    #[serde(rename = "publicKey")]
    public_key: Vec<u8>,
    #[serde(rename = "publicKeyBase58")]
    public_key_base58: String,
}

fn flag_value<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.iter()
        .position(|arg| arg == flag)
        .and_then(|pos| args.get(pos + 1))
        .map(|value| value.as_str())
}

fn repeated_flag_values(args: &[String], flag: &str) -> Vec<String> {
    let mut values = Vec::new();
    let mut index = 0;
    while index < args.len() {
        if args[index] == flag {
            if let Some(value) = args.get(index + 1) {
                values.push(value.clone());
            }
            index += 2;
            continue;
        }
        index += 1;
    }
    values
}

fn parse_genesis_timestamp(genesis_time: &str) -> Result<u64, String> {
    chrono::DateTime::parse_from_rfc3339(genesis_time)
        .map(|dt| dt.timestamp() as u64)
        .map_err(|err| format!("Failed to parse genesis_time '{}': {}", genesis_time, err))
}

fn load_genesis_keypair(path: &std::path::Path) -> Result<Keypair, String> {
    let json = std::fs::read_to_string(path)
        .map_err(|err| format!("Failed to read keypair file {}: {}", path.display(), err))?;
    let keypair_file: GenesisKeypairFile = serde_json::from_str(&json)
        .map_err(|err| format!("Failed to parse keypair file {}: {}", path.display(), err))?;
    if keypair_file.private_key.len() != 32 {
        return Err(format!(
            "Keypair file {} has invalid seed length {} (expected 32 bytes)",
            path.display(),
            keypair_file.private_key.len()
        ));
    }
    let mut seed = [0u8; 32];
    seed.copy_from_slice(&keypair_file.private_key);

    let keypair = Keypair::from_seed(&seed);
    if keypair.public_key().bytes != keypair_file.public_key {
        return Err(format!(
            "Keypair file {} publicKey does not match the derived PQ verifying key",
            path.display()
        ));
    }
    if keypair.pubkey().to_base58() != keypair_file.public_key_base58 {
        return Err(format!(
            "Keypair file {} publicKeyBase58 does not match the derived PQ address",
            path.display()
        ));
    }

    Ok(keypair)
}

fn resolve_artifact_path(base_file: &std::path::Path, relative_or_absolute: &str) -> PathBuf {
    let candidate = PathBuf::from(relative_or_absolute);
    if candidate.is_absolute() {
        return candidate;
    }
    base_file
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join(candidate)
}

fn copy_optional_artifact(
    source_wallet_path: &std::path::Path,
    target_root: &std::path::Path,
    relative_or_absolute: Option<&str>,
) -> Result<(), String> {
    let Some(artifact_path) = relative_or_absolute else {
        return Ok(());
    };

    let source_path = resolve_artifact_path(source_wallet_path, artifact_path);
    if !source_path.exists() {
        return Ok(());
    }

    let target_path = target_root.join(artifact_path);

    // Skip copy if source and target resolve to the same file to avoid
    // truncating the file to 0 bytes (std::fs::copy opens target for
    // write-truncate before reading the source).
    if let (Ok(src_canon), Ok(tgt_canon)) = (source_path.canonicalize(), target_path.canonicalize())
    {
        if src_canon == tgt_canon {
            return Ok(());
        }
    }

    if let Some(parent) = target_path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| {
            format!(
                "Failed to create artifact directory {}: {}",
                parent.display(),
                err
            )
        })?;
    }
    std::fs::copy(&source_path, &target_path).map_err(|err| {
        format!(
            "Failed to copy artifact {} -> {}: {}",
            source_path.display(),
            target_path.display(),
            err
        )
    })?;
    Ok(())
}

fn explicit_initial_validators(
    args: &[String],
    genesis_config: &GenesisConfig,
) -> Result<Vec<Pubkey>, String> {
    let bootstrap_grant_licn = BOOTSTRAP_GRANT_AMOUNT / 1_000_000_000;
    let mut validators = Vec::new();

    for validator in &genesis_config.initial_validators {
        if validator.stake_licn != bootstrap_grant_licn {
            return Err(format!(
                "Genesis validator {} requests {} LICN, but slot-0 registration is fixed at {} LICN",
                validator.pubkey, validator.stake_licn, bootstrap_grant_licn
            ));
        }
        let pubkey = Pubkey::from_base58(&validator.pubkey).map_err(|err| {
            format!(
                "Invalid initial validator pubkey {}: {}",
                validator.pubkey, err
            )
        })?;
        if !validators.contains(&pubkey) {
            validators.push(pubkey);
        }
    }

    for raw in repeated_flag_values(args, "--initial-validator") {
        let pubkey = Pubkey::from_base58(&raw)
            .map_err(|err| format!("Invalid --initial-validator '{}': {}", raw, err))?;
        if !validators.contains(&pubkey) {
            validators.push(pubkey);
        }
    }

    Ok(validators)
}

fn prepare_wallet_artifacts(args: &[String], genesis_config: &GenesisConfig) -> Result<(), String> {
    let output_dir = flag_value(args, "--output-dir")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(format!("./genesis-artifacts-{}", genesis_config.chain_id))
        });
    let keys_dir = output_dir.join("genesis-keys");
    std::fs::create_dir_all(&keys_dir)
        .map_err(|err| format!("Failed to create {}: {}", keys_dir.display(), err))?;

    let is_mainnet = genesis_config.chain_id.contains("mainnet");
    let default_signers = if is_mainnet { 5usize } else { 3usize };
    let signer_count = flag_value(args, "--signers")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(default_signers);

    let (mut wallet, keypairs, distribution_keypairs) =
        GenesisWallet::generate(&genesis_config.chain_id, is_mainnet, signer_count)?;

    // Save keypair files first
    let keypair_paths =
        GenesisWallet::save_keypairs(&keypairs, &keys_dir, &genesis_config.chain_id)?;
    let distribution_paths = GenesisWallet::save_distribution_keypairs(
        wallet.distribution_wallets.as_deref().unwrap_or(&[]),
        &distribution_keypairs,
        &keys_dir,
        &genesis_config.chain_id,
    )?;
    if let Some(treasury_keypair) = distribution_keypairs.first() {
        GenesisWallet::save_treasury_keypair(
            treasury_keypair,
            &keys_dir,
            &genesis_config.chain_id,
        )?;
    }

    // Fill keypair_path on each distribution wallet so the wallet JSON records them
    if let Some(ref mut dist) = wallet.distribution_wallets {
        for dw in dist.iter_mut() {
            dw.keypair_path = Some(format!(
                "genesis-keys/{}-{}.json",
                dw.role, genesis_config.chain_id
            ));
        }
    }

    // Save wallet AFTER filling keypair paths
    let wallet_path = output_dir.join("genesis-wallet.json");
    wallet.save(&wallet_path)?;

    info!("═══════════════════════════════════════════════════════");
    info!("  Prepared deterministic genesis artifacts");
    info!("═══════════════════════════════════════════════════════");
    info!("  Wallet: {}", wallet_path.display());
    info!("  Signers: {}", keypair_paths.len());
    info!("  Distribution wallets: {}", distribution_paths.len());
    info!("  Output dir: {}", output_dir.display());
    info!("═══════════════════════════════════════════════════════");
    Ok(())
}

/// Fetch live market prices from Binance REST API for genesis seeding.
/// Falls back to compiled defaults if the fetch fails (e.g. no internet).
/// This runs ONCE during genesis creation — the returned prices are embedded
/// in the genesis block and reused by all joining validators.
fn fetch_genesis_prices() -> GenesisPrices {
    let defaults = GenesisPrices::default();

    #[derive(serde::Deserialize)]
    struct Ticker {
        symbol: String,
        price: String,
    }

    let url = "https://api.binance.com/api/v3/ticker/price?symbols=[%22SOLUSDT%22,%22ETHUSDT%22,%22BNBUSDT%22]";
    let resp = match ureq::get(url).call() {
        Ok(r) => r,
        Err(e) => {
            warn!(
                "Binance price fetch failed ({}), using compiled defaults",
                e
            );
            return defaults;
        }
    };

    let body = match resp.into_string() {
        Ok(b) => b,
        Err(e) => {
            warn!(
                "Failed to read Binance response ({}), using compiled defaults",
                e
            );
            return defaults;
        }
    };

    let tickers: Vec<Ticker> = match serde_json::from_str(&body) {
        Ok(t) => t,
        Err(e) => {
            warn!(
                "Failed to parse Binance JSON ({}), using compiled defaults",
                e
            );
            return defaults;
        }
    };

    let mut prices = defaults.clone();
    for t in &tickers {
        let usd: f64 = t.price.parse().unwrap_or(0.0);
        if usd <= 0.0 {
            continue;
        }
        let price_8dec = (usd * 100_000_000.0) as u64;
        match t.symbol.as_str() {
            "SOLUSDT" => prices.wsol_usd_8dec = price_8dec,
            "ETHUSDT" => prices.weth_usd_8dec = price_8dec,
            "BNBUSDT" => prices.wbnb_usd_8dec = price_8dec,
            _ => {}
        }
    }

    info!(
        "  💰 Live prices: SOL=${:.2}, ETH=${:.2}, BNB=${:.2}",
        prices.wsol_usd_8dec as f64 / 100_000_000.0,
        prices.weth_usd_8dec as f64 / 100_000_000.0,
        prices.wbnb_usd_8dec as f64 / 100_000_000.0,
    );

    prices
}

fn main() {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args: Vec<String> = std::env::args().collect();

    let network = flag_value(&args, "--network");
    let db_path = flag_value(&args, "--db-path").map(str::to_string);
    let wallet_file = flag_value(&args, "--wallet-file").map(PathBuf::from);
    let genesis_keypair_file = flag_value(&args, "--genesis-keypair").map(PathBuf::from);
    let prepare_wallet = args.iter().any(|arg| arg == "--prepare-wallet");
    let config_path = flag_value(&args, "--config").map(PathBuf::from);

    let network_str = match network {
        Some(n @ ("mainnet" | "testnet")) => n,
        Some(other) => {
            error!(
                "Unknown network '{}'. Use --network mainnet or --network testnet",
                other
            );
            std::process::exit(1);
        }
        None => {
            error!("Usage: lichen-genesis --network <mainnet|testnet> [--prepare-wallet --output-dir <path>] [--wallet-file <path>] [--initial-validator <base58>] [--db-path <path>] [--config <path>]");
            std::process::exit(1);
        }
    };

    let mut genesis_config = if let Some(ref path) = config_path {
        match GenesisConfig::from_file(path) {
            Ok(config) => config,
            Err(err) => {
                error!("Failed to load genesis config {}: {}", path.display(), err);
                std::process::exit(1);
            }
        }
    } else {
        match network_str {
            "mainnet" => GenesisConfig::default_mainnet(),
            _ => GenesisConfig::default_testnet(),
        }
    };

    if prepare_wallet {
        if let Err(err) = prepare_wallet_artifacts(&args, &genesis_config) {
            error!("{}", err);
            std::process::exit(1);
        }
        return;
    }

    let wallet_file = match wallet_file {
        Some(path) => path,
        None => {
            error!("Genesis creation now requires --wallet-file <path>. Use --prepare-wallet to generate artifacts explicitly.");
            std::process::exit(1);
        }
    };

    let genesis_timestamp = match parse_genesis_timestamp(&genesis_config.genesis_time) {
        Ok(timestamp) => timestamp,
        Err(err) => {
            error!("{}", err);
            std::process::exit(1);
        }
    };

    let wallet = match GenesisWallet::load(&wallet_file) {
        Ok(wallet) => wallet,
        Err(err) => {
            error!("Failed to load wallet {}: {}", wallet_file.display(), err);
            std::process::exit(1);
        }
    };
    if wallet.chain_id != genesis_config.chain_id {
        error!(
            "Wallet chain_id {} does not match genesis chain_id {}",
            wallet.chain_id, genesis_config.chain_id
        );
        std::process::exit(1);
    }

    let genesis_signer_path = genesis_keypair_file
        .unwrap_or_else(|| resolve_artifact_path(&wallet_file, &wallet.keypair_path));
    let genesis_signer = match load_genesis_keypair(&genesis_signer_path) {
        Ok(keypair) => keypair,
        Err(err) => {
            error!("{}", err);
            std::process::exit(1);
        }
    };

    let initial_validators = match explicit_initial_validators(&args, &genesis_config) {
        Ok(validators) if !validators.is_empty() => validators,
        Ok(_) => {
            error!("Genesis creation requires at least one explicit validator. Pass --initial-validator <base58> or provide initial_validators in --config.");
            std::process::exit(1);
        }
        Err(err) => {
            error!("{}", err);
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
    info!("  Lichen Genesis — One-Time Chain Initialization");
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
        error!(
            "Genesis block already exists in {}. Refusing to overwrite.",
            db_dir
        );
        error!("To create a new genesis, delete or move the existing database first.");
        std::process::exit(1);
    }

    info!("Chain ID: {}", genesis_config.chain_id);
    info!("Total supply: {} LICN", genesis_config.total_supply_licn());
    info!("Genesis time: {}", genesis_config.genesis_time);

    let genesis_wallet_path = db_dir_path.join("genesis-wallet.json");
    let genesis_keypairs_dir = db_dir_path.join("genesis-keys");
    std::fs::create_dir_all(&genesis_keypairs_dir).ok();

    let genesis_pubkey = wallet.pubkey;
    info!("  ✓ Loaded genesis pubkey: {}", genesis_pubkey.to_base58());

    if let Some(ref multisig) = wallet.multisig {
        info!("  ✓ Multi-sig configuration:");
        info!(
            "    - Threshold: {}/{} signatures",
            multisig.threshold,
            multisig.signers.len()
        );
        info!("    - Genesis treasury: {}", multisig.is_genesis);
        info!("    - Signers:");
        for (i, signer) in multisig.signers.iter().enumerate() {
            info!("      {}. {}", i + 1, signer.to_base58());
        }
    }

    // Log whitepaper distribution
    if let Some(ref dist) = wallet.distribution_wallets {
        info!(
            "  📊 Whitepaper genesis distribution ({} wallets):",
            dist.len()
        );
        for dw in dist {
            info!(
                "    - {} ({}%): {} LICN → {}",
                dw.role,
                dw.percentage,
                dw.amount_licn,
                dw.pubkey.to_base58()
            );
        }
    }

    if let Err(err) = wallet.save(&genesis_wallet_path) {
        error!("Failed to save genesis wallet: {}", err);
        std::process::exit(1);
    }
    info!("  ✓ Wallet info saved: {}", genesis_wallet_path.display());
    if let Err(err) = copy_optional_artifact(&wallet_file, &db_dir_path, Some(&wallet.keypair_path))
    {
        error!("{}", err);
        std::process::exit(1);
    }
    if let Err(err) = copy_optional_artifact(
        &wallet_file,
        &db_dir_path,
        wallet.treasury_keypair_path.as_deref(),
    ) {
        error!("{}", err);
        std::process::exit(1);
    }

    // Copy ALL distribution keypairs to data dir and validate pubkey consistency
    if let Some(ref dist) = wallet.distribution_wallets {
        for dw in dist {
            if let Some(ref kp_path) = dw.keypair_path {
                if let Err(err) = copy_optional_artifact(&wallet_file, &db_dir_path, Some(kp_path))
                {
                    error!("{}", err);
                    std::process::exit(1);
                }
                // Validate keypair pubkey matches wallet pubkey
                let resolved = db_dir_path.join(kp_path);
                if resolved.exists() {
                    match std::fs::read_to_string(&resolved) {
                        Ok(contents) => {
                            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&contents)
                            {
                                if let Some(file_pk) = parsed.get("pubkey").and_then(|v| v.as_str())
                                {
                                    let wallet_pk = dw.pubkey.to_base58();
                                    if file_pk != wallet_pk {
                                        error!(
                                            "KEYPAIR MISMATCH for {}: wallet has {} but keypair file has {}. \
                                             Re-run --prepare-wallet to regenerate matching artifacts.",
                                            dw.role, wallet_pk, file_pk
                                        );
                                        std::process::exit(1);
                                    }
                                    info!(
                                        "  ✓ {} keypair copied and validated: {}",
                                        dw.role, wallet_pk
                                    );
                                }
                            }
                        }
                        Err(e) => warn!(
                            "  ⚠️  Could not read {} keypair for validation: {}",
                            dw.role, e
                        ),
                    }
                }
            }
        }
    }

    // Sync CLI-provided validators into genesis_config so genesis.json is accurate
    let bootstrap_grant_licn = BOOTSTRAP_GRANT_AMOUNT / 1_000_000_000;
    for v in &initial_validators {
        let pubkey_str = v.to_base58();
        if !genesis_config
            .initial_validators
            .iter()
            .any(|gv| gv.pubkey == pubkey_str)
        {
            genesis_config.initial_validators.push(GenesisValidator {
                pubkey: pubkey_str,
                stake_licn: bootstrap_grant_licn,
                reputation: 100,
                comment: Some("CLI --initial-validator".to_string()),
            });
        }
    }

    // ════════════════════════════════════════════════════════════════════
    // FETCH LIVE MARKET PRICES — frozen into genesis config forever
    // ════════════════════════════════════════════════════════════════════
    genesis_config.genesis_prices = fetch_genesis_prices();
    info!(
        "  ✓ Genesis prices frozen: LICN=${:.4}, SOL=${:.2}, ETH=${:.2}, BNB=${:.2}",
        genesis_config.genesis_prices.licn_usd_8dec as f64 / 100_000_000.0,
        genesis_config.genesis_prices.wsol_usd_8dec as f64 / 100_000_000.0,
        genesis_config.genesis_prices.weth_usd_8dec as f64 / 100_000_000.0,
        genesis_config.genesis_prices.wbnb_usd_8dec as f64 / 100_000_000.0,
    );

    let effective_genesis_config_path = db_dir_path.join("genesis.json");
    // Always serialize the effective config (includes live prices + CLI validators)
    {
        let json = match serde_json::to_string_pretty(&genesis_config) {
            Ok(json) => json,
            Err(err) => {
                error!("Failed to serialize effective genesis config: {}", err);
                std::process::exit(1);
            }
        };
        if let Err(err) = std::fs::write(&effective_genesis_config_path, json) {
            error!(
                "Failed to write effective genesis config {}: {}",
                effective_genesis_config_path.display(),
                err
            );
            std::process::exit(1);
        }
    }
    info!(
        "  ✓ Genesis config saved: {}",
        effective_genesis_config_path.display()
    );

    // ════════════════════════════════════════════════════════════════════
    // CREATE GENESIS STATE
    // ════════════════════════════════════════════════════════════════════
    info!("📦 Creating genesis state...");

    // Store rent params
    if let Err(e) = state.set_rent_params(
        genesis_config.features.rent_rate_spores_per_kb_month,
        genesis_config.features.rent_free_kb,
    ) {
        warn!("⚠️  Failed to store rent params: {}", e);
    }

    // Store fee configuration
    let genesis_fee_config = FeeConfig {
        base_fee: genesis_config.features.base_fee_spores,
        contract_deploy_fee: CONTRACT_DEPLOY_FEE,
        contract_upgrade_fee: CONTRACT_UPGRADE_FEE,
        nft_mint_fee: NFT_MINT_FEE,
        nft_collection_fee: NFT_COLLECTION_FEE,
        fee_burn_percent: genesis_config.features.fee_burn_percentage,
        fee_producer_percent: genesis_config.features.fee_producer_percentage,
        fee_voters_percent: genesis_config.features.fee_voters_percentage,
        fee_community_percent: genesis_config.features.fee_community_percentage,
        fee_treasury_percent: genesis_config.features.fee_treasury_percentage,
        fee_exempt_contracts: Vec::new(),
    };
    if let Err(e) = state.set_fee_config_full(&genesis_fee_config) {
        warn!("⚠️  Failed to store fee config: {}", e);
    } else {
        info!("  ✓ Fee config persisted: base={} spores, burn={}%, producer={}%, voters={}%, treasury={}%, community={}%",
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
    let total_supply_licn = 500_000_000u64;
    let mut genesis_account = Account::new(total_supply_licn, genesis_pubkey);

    if let Some(ref multisig) = wallet.multisig {
        genesis_account.owner = genesis_pubkey;
        info!("  ✓ Flagged as genesis treasury with multi-sig");
        info!(
            "    Threshold: {}/{} signatures",
            multisig.threshold,
            multisig.signers.len()
        );
    }

    if let Err(e) = state.put_account(&genesis_pubkey, &genesis_account) {
        error!("Failed to store genesis account: {e}");
        std::process::exit(1);
    }
    if let Err(e) = state.set_genesis_pubkey(&genesis_pubkey) {
        error!("Failed to set genesis pubkey: {e}");
        std::process::exit(1);
    }
    info!("  ✓ Genesis mint: {} LICN", total_supply_licn);
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
            let amount_spores = Account::licn_to_spores(dw.amount_licn);

            let mut acct = Account::new(0, SYSTEM_ACCOUNT_OWNER);
            acct.spores = amount_spores;

            if dw.role == "founding_symbionts" {
                acct.spendable = 0;
                acct.locked = amount_spores;
            } else {
                acct.spendable = amount_spores;
            }

            if let Err(e) = state.put_account(&dw.pubkey, &acct) {
                error!("Failed to create {} account: {e}", dw.role);
            }

            src_acct.spores = src_acct.spores.saturating_sub(amount_spores);
            src_acct.spendable = src_acct.spendable.saturating_sub(amount_spores);

            if dw.role == "validator_rewards" {
                if let Err(e) = state.set_treasury_pubkey(&dw.pubkey) {
                    error!("Failed to set treasury pubkey: {e}");
                }
                info!(
                    "  ✓ {} ({}%): {} LICN → {} [TREASURY]",
                    dw.role,
                    dw.percentage,
                    dw.amount_licn,
                    dw.pubkey.to_base58()
                );
            } else if dw.role == "founding_symbionts" {
                info!(
                    "  ✓ {} ({}%): {} LICN → {} [LOCKED — 6mo cliff + 18mo vest]",
                    dw.role,
                    dw.percentage,
                    dw.amount_licn,
                    dw.pubkey.to_base58()
                );
            } else {
                info!(
                    "  ✓ {} ({}%): {} LICN → {}",
                    dw.role,
                    dw.percentage,
                    dw.amount_licn,
                    dw.pubkey.to_base58()
                );
            }
        }

        if let Err(e) = state.put_account(&genesis_pubkey, &src_acct) {
            error!("Failed to update genesis account after distribution: {e}");
        }

        // Store genesis accounts in state DB
        let ga_entries: Vec<(String, Pubkey, u64, u8)> = dist_wallets
            .iter()
            .map(|dw| (dw.role.clone(), dw.pubkey, dw.amount_licn, dw.percentage))
            .collect();
        if let Err(e) = state.set_genesis_accounts(&ga_entries) {
            error!("Failed to store genesis accounts in DB: {e}");
        } else {
            info!(
                "  ✓ Stored {} genesis accounts in state DB",
                ga_entries.len()
            );
        }

        info!("  ✓ Genesis distribution complete — 500M LICN allocated per whitepaper");

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
                if let Some(config) = governed_wallet_config_for_role(&dw.role, &all_signers) {
                    if let Err(e) = state.set_governed_wallet_config(&dw.pubkey, &config) {
                        error!("Failed to store {} governed config: {e}", dw.role);
                    } else {
                        info!(
                            "  ✓ {} governed wallet: threshold={}, {} signers, timelock={} epoch(s)",
                            dw.role,
                            config.threshold,
                            config.signers.len(),
                            config.timelock_epochs
                        );
                    }
                }
            }

            let committee_roles: Vec<(String, Pubkey)> = dist_wallets
                .iter()
                .map(|dw| (dw.role.clone(), dw.pubkey))
                .collect();
            match state.get_community_treasury_pubkey() {
                Ok(Some(governance_authority)) => {
                    match incident_guardian_config_for_roles(
                        &committee_roles,
                        &governance_authority,
                    ) {
                        Ok((guardian_authority, guardian_config)) => {
                            if let Err(e) =
                                state.set_incident_guardian_authority(&guardian_authority)
                            {
                                error!("Failed to store incident guardian authority: {e}");
                            } else if let Err(e) = state
                                .set_governed_wallet_config(&guardian_authority, &guardian_config)
                            {
                                error!("Failed to store incident guardian config: {e}");
                            } else {
                                info!(
                                    "  ✓ incident_guardian governed authority: threshold={}, {} signers, authority={}",
                                    guardian_config.threshold,
                                    guardian_config.signers.len(),
                                    guardian_authority.to_base58()
                                );
                            }
                        }
                        Err(e) => error!("Failed to derive incident guardian config: {e}"),
                    }

                    match bridge_committee_admin_config_for_roles(
                        &committee_roles,
                        &governance_authority,
                    ) {
                        Ok((bridge_authority, bridge_config)) => {
                            if let Err(e) =
                                state.set_bridge_committee_admin_authority(&bridge_authority)
                            {
                                error!("Failed to store bridge committee admin authority: {e}");
                            } else if let Err(e) =
                                state.set_governed_wallet_config(&bridge_authority, &bridge_config)
                            {
                                error!("Failed to store bridge committee admin config: {e}");
                            } else {
                                info!(
                                    "  ✓ bridge_committee_admin governed authority: threshold={}, {} signers, authority={}",
                                    bridge_config.threshold,
                                    bridge_config.signers.len(),
                                    bridge_authority.to_base58()
                                );
                            }
                        }
                        Err(e) => error!("Failed to derive bridge committee admin config: {e}"),
                    }

                    match oracle_committee_admin_config_for_roles(
                        &committee_roles,
                        &governance_authority,
                    ) {
                        Ok((oracle_authority, oracle_config)) => {
                            if let Err(e) =
                                state.set_oracle_committee_admin_authority(&oracle_authority)
                            {
                                error!("Failed to store oracle committee admin authority: {e}");
                            } else if let Err(e) =
                                state.set_governed_wallet_config(&oracle_authority, &oracle_config)
                            {
                                error!("Failed to store oracle committee admin config: {e}");
                            } else {
                                info!(
                                    "  ✓ oracle_committee_admin governed authority: threshold={}, {} signers, authority={}",
                                    oracle_config.threshold,
                                    oracle_config.signers.len(),
                                    oracle_authority.to_base58()
                                );
                            }
                        }
                        Err(e) => error!("Failed to derive oracle committee admin config: {e}"),
                    }

                    match treasury_executor_config_for_roles(
                        &committee_roles,
                        &governance_authority,
                    ) {
                        Ok((treasury_authority, treasury_config)) => {
                            if let Err(e) =
                                state.set_treasury_executor_authority(&treasury_authority)
                            {
                                error!("Failed to store treasury executor authority: {e}");
                            } else if let Err(e) = state
                                .set_governed_wallet_config(&treasury_authority, &treasury_config)
                            {
                                error!("Failed to store treasury executor config: {e}");
                            } else {
                                info!(
                                    "  ✓ treasury_executor governed authority: threshold={}, {} signers, authority={}",
                                    treasury_config.threshold,
                                    treasury_config.signers.len(),
                                    treasury_authority.to_base58()
                                );
                            }
                        }
                        Err(e) => error!("Failed to derive treasury executor config: {e}"),
                    }

                    match upgrade_proposer_config_for_roles(&committee_roles, &governance_authority)
                    {
                        Ok((upgrade_authority, upgrade_config)) => {
                            if let Err(e) = state.set_upgrade_proposer_authority(&upgrade_authority)
                            {
                                error!("Failed to store upgrade proposer authority: {e}");
                            } else if let Err(e) = state
                                .set_governed_wallet_config(&upgrade_authority, &upgrade_config)
                            {
                                error!("Failed to store upgrade proposer config: {e}");
                            } else {
                                info!(
                                    "  ✓ upgrade_proposer governed authority: threshold={}, {} signers, authority={}",
                                    upgrade_config.threshold,
                                    upgrade_config.signers.len(),
                                    upgrade_authority.to_base58()
                                );
                            }
                        }
                        Err(e) => error!("Failed to derive upgrade proposer config: {e}"),
                    }

                    match upgrade_veto_guardian_config_for_roles(
                        &committee_roles,
                        &governance_authority,
                    ) {
                        Ok((veto_authority, veto_config)) => {
                            if let Err(e) =
                                state.set_upgrade_veto_guardian_authority(&veto_authority)
                            {
                                error!("Failed to store upgrade veto guardian authority: {e}");
                            } else if let Err(e) =
                                state.set_governed_wallet_config(&veto_authority, &veto_config)
                            {
                                error!("Failed to store upgrade veto guardian config: {e}");
                            } else {
                                info!(
                                    "  ✓ upgrade_veto_guardian governed authority: threshold={}, {} signers, authority={}",
                                    veto_config.threshold,
                                    veto_config.signers.len(),
                                    veto_authority.to_base58()
                                );
                            }
                        }
                        Err(e) => error!("Failed to derive upgrade veto guardian config: {e}"),
                    }
                }
                Ok(None) => {
                    error!(
                        "Failed to derive incident guardian config: community_treasury not found"
                    )
                }
                Err(e) => error!("Failed to load community_treasury for incident guardian: {e}"),
            }
        }

        // Build distribution transactions for genesis block
        for dw in dist_wallets {
            let mut data = Vec::with_capacity(9);
            data.push(4); // Genesis transfer (fee-free)
            data.extend_from_slice(&Account::licn_to_spores(dw.amount_licn).to_le_bytes());

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
    } else {
        error!("Genesis wallet missing required distribution_wallets configuration");
        std::process::exit(1);
    }

    // Create initial accounts from genesis config
    for account_info in &genesis_config.initial_accounts {
        let pubkey = match Pubkey::from_base58(&account_info.address) {
            Ok(pk) => pk,
            Err(e) => {
                warn!(
                    "Skipping initial account with invalid address {}: {e}",
                    account_info.address
                );
                continue;
            }
        };
        let account = Account::new(account_info.balance_licn, pubkey);
        if let Err(e) = state.put_account(&pubkey, &account) {
            error!("Failed to store initial account: {e}");
        }
        info!(
            "  ✓ Account {}: {} LICN",
            &account_info.address[..20.min(account_info.address.len())],
            account_info.balance_licn
        );
    }

    // Mint transaction
    let mint_spores = Account::licn_to_spores(total_supply_licn);
    let mut mint_data = Vec::with_capacity(9);
    mint_data.push(5); // Genesis mint (synthetic, fee-free)
    mint_data.extend_from_slice(&mint_spores.to_le_bytes());

    let mint_instruction = Instruction {
        program_id: SYSTEM_PROGRAM_ID,
        accounts: vec![GENESIS_MINT_PUBKEY, genesis_pubkey],
        data: mint_data,
    };

    let mint_message = Message::new(vec![mint_instruction], Hash::default());
    let mint_tx = Transaction::new(mint_message);

    // Insert mint tx at the beginning
    genesis_txs.insert(0, mint_tx);

    // Explicit slot-0 validator registrations.
    let treasury_pubkey = match state.get_treasury_pubkey().ok().flatten() {
        Some(pubkey) => pubkey,
        None => {
            error!("Treasury pubkey missing before validator bootstrap");
            std::process::exit(1);
        }
    };
    let mut treasury_account = match state.get_account(&treasury_pubkey).ok().flatten() {
        Some(account) => account,
        None => {
            error!("Treasury account missing before validator bootstrap");
            std::process::exit(1);
        }
    };
    let mut stake_pool = state.get_stake_pool().unwrap_or_else(|_| StakePool::new());
    for validator_pubkey in &initial_validators {
        if let Err(err) = treasury_account.deduct_spendable(BOOTSTRAP_GRANT_AMOUNT) {
            error!(
                "Treasury cannot fund explicit validator {}: {}",
                validator_pubkey.to_base58(),
                err
            );
            std::process::exit(1);
        }

        let mut validator_account = state
            .get_account(validator_pubkey)
            .ok()
            .flatten()
            .unwrap_or_else(|| Account::new(0, SYSTEM_ACCOUNT_OWNER));
        validator_account.spores = validator_account
            .spores
            .saturating_add(BOOTSTRAP_GRANT_AMOUNT);
        validator_account.staked = validator_account
            .staked
            .saturating_add(BOOTSTRAP_GRANT_AMOUNT);
        validator_account.spendable = validator_account
            .spendable
            .saturating_sub(validator_account.spendable);
        if let Err(err) = state.put_account(validator_pubkey, &validator_account) {
            error!(
                "Failed to store initial validator account {}: {}",
                validator_pubkey.to_base58(),
                err
            );
            std::process::exit(1);
        }
        if let Err(err) = stake_pool.try_bootstrap_with_fingerprint(
            *validator_pubkey,
            BOOTSTRAP_GRANT_AMOUNT,
            0,
            [0u8; 32],
        ) {
            error!(
                "Failed to bootstrap initial validator {}: {}",
                validator_pubkey.to_base58(),
                err
            );
            std::process::exit(1);
        }

        let mut ix_data = vec![26u8];
        ix_data.extend_from_slice(&[0u8; 32]);
        let instruction = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![*validator_pubkey],
            data: ix_data,
        };
        let message = Message::new(vec![instruction], Hash::default());
        let mut tx = Transaction::new(message.clone());
        tx.signatures
            .push(genesis_signer.sign(&message.serialize()));
        genesis_txs.push(tx);
        info!(
            "  ✓ Initial validator registered at genesis: {} ({} LICN)",
            validator_pubkey.to_base58(),
            BOOTSTRAP_GRANT_AMOUNT / 1_000_000_000
        );
    }
    if let Err(err) = state.put_account(&treasury_pubkey, &treasury_account) {
        error!(
            "Failed to update treasury after validator bootstrap: {}",
            err
        );
        std::process::exit(1);
    }
    if let Err(err) = state.put_stake_pool(&stake_pool) {
        error!("Failed to persist initial stake pool: {}", err);
        std::process::exit(1);
    }

    // Store founding symbionts vesting schedule (CF_STATS, not in state root)
    if let Some(fm_dw) = wallet
        .distribution_wallets
        .as_ref()
        .and_then(|ws| ws.iter().find(|dw| dw.role == "founding_symbionts"))
    {
        let cliff_end = genesis_timestamp + FOUNDING_CLIFF_SECONDS;
        let vest_end = genesis_timestamp + FOUNDING_VEST_TOTAL_SECONDS;
        let total_spores = Account::licn_to_spores(fm_dw.amount_licn);
        if let Err(e) = state.set_founding_vesting_params(cliff_end, vest_end, total_spores) {
            error!("Failed to store founding vesting params: {e}");
        } else {
            info!(
                "  ✓ Founding symbionts vesting: cliff={}, vest_end={}, total={}M LICN",
                cliff_end,
                vest_end,
                fm_dw.amount_licn / 1_000_000
            );
        }
    }

    // ════════════════════════════════════════════════════════════════════
    // AUTO-DEPLOY CONTRACTS (before genesis block so state_root is complete)
    // ════════════════════════════════════════════════════════════════════
    let gp = &genesis_config.genesis_prices;
    genesis_auto_deploy(&state, &genesis_pubkey, "GENESIS:");
    if let Err(err) = genesis_harden_contract_controls(&state, &genesis_pubkey, "GENESIS:") {
        error!("Failed to install genesis governance/timelocks: {}", err);
        std::process::exit(1);
    };
    if let Err(err) =
        genesis_initialize_contracts(&state, &genesis_pubkey, "GENESIS:", genesis_timestamp)
    {
        error!("Failed to initialize genesis contracts: {}", err);
        std::process::exit(1);
    };
    genesis_create_trading_pairs(&state, &genesis_pubkey, "GENESIS:", gp);
    genesis_set_fee_exempt_contracts(&state, &genesis_pubkey, "GENESIS:");
    genesis_seed_oracle(&state, &genesis_pubkey, "GENESIS:", genesis_timestamp, gp);
    genesis_seed_margin_prices(&state, &genesis_pubkey, genesis_timestamp, gp);
    genesis_seed_analytics_prices(&state, &genesis_pubkey, genesis_timestamp, gp);

    // ════════════════════════════════════════════════════════════════════
    // GENESIS IDENTITIES & ACHIEVEMENTS
    // ════════════════════════════════════════════════════════════════════
    {
        let dist_pairs: Vec<(String, Pubkey)> = wallet
            .distribution_wallets
            .as_deref()
            .unwrap_or(&[])
            .iter()
            .map(|dw| (dw.role.clone(), dw.pubkey))
            .collect();
        genesis_assign_achievements(&state, &genesis_pubkey, &dist_pairs, genesis_timestamp);
    }

    // ════════════════════════════════════════════════════════════════════
    // EMBED GENESIS CONFIG (opcode 40) — self-contained genesis block
    // Joining validators extract this to get the frozen GenesisPrices.
    // ════════════════════════════════════════════════════════════════════
    {
        let config_json = serde_json::to_vec(&genesis_config).expect("serialize GenesisConfig");
        let mut ix_data = Vec::with_capacity(1 + config_json.len());
        ix_data.push(40u8); // opcode 40 = GENESIS_CONFIG
        ix_data.extend_from_slice(&config_json);

        let instruction = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![genesis_pubkey],
            data: ix_data,
        };
        let message = Message::new(vec![instruction], Hash::default());
        let config_tx = Transaction::new(message);
        genesis_txs.push(config_tx);
        info!(
            "  ✓ GenesisConfig embedded in genesis block (opcode 40, {} bytes)",
            config_json.len()
        );
    }

    // ════════════════════════════════════════════════════════════════════
    // CREATE GENESIS BLOCK — state_root captures FULL state (accounts +
    // contracts + oracle + analytics + margin) per Cosmos/Substrate standard.
    // ════════════════════════════════════════════════════════════════════
    let state_root = state.compute_state_root();
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
    info!("  State root: {}", hex::encode(state_root.0));

    // Flush metrics counters to disk — contract deploy (index_program) and
    // any accounts created need their counters persisted so the validator
    // reads correct values on startup.
    if let Err(e) = state.save_metrics_counters() {
        error!("Failed to flush metrics after contract deployment: {}", e);
    }

    info!("═══════════════════════════════════════════════════════");
    info!("  ✅ Genesis creation complete!");
    info!("  Database: {}", db_dir);
    info!("  Genesis pubkey: {}", genesis_pubkey.to_base58());
    info!("  Genesis hash: {}", genesis_block.hash());
    info!("═══════════════════════════════════════════════════════");
    info!("  Next: start the validator pointing at this DB:");
    info!(
        "    lichen-validator --network {} --db-path {}",
        network_str, db_dir
    );
    info!("═══════════════════════════════════════════════════════");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_genesis_keypair_from_canonical_file() {
        let keypair = Keypair::generate();
        let public_key = keypair.public_key();
        let path = std::env::temp_dir().join(format!(
            "lichen-genesis-keypair-{}-{}.json",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let json = serde_json::json!({
            "privateKey": keypair.to_seed(),
            "publicKey": public_key.bytes,
            "publicKeyBase58": keypair.pubkey().to_base58(),
        });

        std::fs::write(&path, serde_json::to_string_pretty(&json).unwrap()).unwrap();

        let loaded = load_genesis_keypair(&path).unwrap();
        assert_eq!(loaded.to_seed(), keypair.to_seed());

        let _ = std::fs::remove_file(path);
    }
}
