//! Lichen Genesis — standalone one-time genesis block creator.
//!
//! Usage:
//!   lichen-genesis --prepare-wallet --network testnet --output-dir ./artifacts/testnet
//!   lichen-genesis --network testnet --wallet-file ./artifacts/testnet/genesis-wallet.json --initial-validator <base58> --db-path /var/lib/lichen/state-testnet

use flate2::{write::GzEncoder, Compression};
use lichen_core::consensus::{
    StakePool, BOOTSTRAP_GRANT_AMOUNT, FOUNDING_CLIFF_SECONDS, FOUNDING_VEST_TOTAL_SECONDS,
};
use lichen_core::keypair_file::{
    copy_secure_file, load_keypair_with_password_policy, plaintext_keypair_compat_allowed,
    require_runtime_keypair_password,
};
use lichen_core::multisig::{
    bridge_committee_admin_config_for_roles, governed_wallet_config_for_role,
    incident_guardian_config_for_roles, oracle_committee_admin_config_for_roles,
    treasury_executor_config_for_roles, upgrade_proposer_config_for_roles,
    upgrade_veto_guardian_config_for_roles,
};
use lichen_core::{
    Account, Block, FeeConfig, GenesisConfig, GenesisPrices, GenesisStateBundle,
    GenesisStateCategory, GenesisStateChunk, GenesisValidator, GenesisWallet, Hash, Instruction,
    Keypair, Message, Pubkey, StateStore, Transaction, CONTRACT_DEPLOY_FEE, CONTRACT_UPGRADE_FEE,
    GENESIS_STATE_BUNDLE_VERSION, GENESIS_STATE_CHUNK_OPCODE, NFT_COLLECTION_FEE, NFT_MINT_FEE,
    SYSTEM_PROGRAM_ID,
};
use lichen_genesis::{
    genesis_assign_achievements, genesis_auto_deploy, genesis_bootstrap_bridge_committee,
    genesis_create_trading_pairs, genesis_harden_contract_controls, genesis_initialize_contracts,
    genesis_seed_analytics_prices, genesis_seed_consensus_oracle_prices,
    genesis_seed_margin_prices, genesis_seed_oracle, genesis_set_fee_exempt_contracts,
};
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tracing::{error, info, warn};

const SYSTEM_ACCOUNT_OWNER: Pubkey = Pubkey([0x01; 32]);
const GENESIS_MINT_PUBKEY: Pubkey = Pubkey([0xFE; 32]);
const GENESIS_STATE_CHUNK_BYTES: usize = 180 * 1024;
const GENESIS_STATE_KV_CATEGORIES: &[&str] = &[
    "accounts",
    "contract_storage",
    "programs",
    "symbol_registry",
    "symbol_by_program",
    "restrictions",
    "restriction_index_target",
    "restriction_index_code_hash",
    "stats",
];

type GenesisStateEntry = (Vec<u8>, Vec<u8>);
type GenesisStateEntries = Vec<GenesisStateEntry>;

fn export_all_category_entries(
    state: &StateStore,
    category: &str,
) -> Result<GenesisStateEntries, String> {
    let mut entries = Vec::new();
    let mut cursor: Option<Vec<u8>> = None;

    loop {
        let page =
            state.export_snapshot_category_cursor_untracked(category, cursor.as_deref(), 1000)?;
        entries.extend(page.entries);
        if !page.has_more {
            break;
        }
        cursor = page.next_cursor;
    }

    Ok(entries)
}

fn build_genesis_state_bundle(
    state: &StateStore,
    state_root: Hash,
) -> Result<GenesisStateBundle, String> {
    let mut categories = Vec::new();

    for category in GENESIS_STATE_KV_CATEGORIES {
        categories.push(GenesisStateCategory {
            name: (*category).to_string(),
            entries: export_all_category_entries(state, category)?,
        });
    }

    let stake_pool = state.get_stake_pool()?;
    categories.push(GenesisStateCategory {
        name: "stake_pool".to_string(),
        entries: vec![(
            b"pool".to_vec(),
            bincode::serialize(&stake_pool)
                .map_err(|e| format!("Failed to serialize stake pool: {}", e))?,
        )],
    });

    let mossstake_pool = state.get_mossstake_pool()?;
    categories.push(GenesisStateCategory {
        name: "mossstake_pool".to_string(),
        entries: vec![(
            b"pool".to_vec(),
            bincode::serialize(&mossstake_pool)
                .map_err(|e| format!("Failed to serialize MossStake pool: {}", e))?,
        )],
    });

    Ok(GenesisStateBundle {
        version: GENESIS_STATE_BUNDLE_VERSION,
        state_root: state_root.0,
        categories,
    })
}

fn encode_genesis_state_chunks(
    state_root: Hash,
    bundle: &GenesisStateBundle,
) -> Result<Vec<GenesisStateChunk>, String> {
    let raw = bincode::serialize(bundle)
        .map_err(|e| format!("Failed to serialize genesis state bundle: {}", e))?;
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(&raw)
        .map_err(|e| format!("Failed to compress genesis state bundle: {}", e))?;
    let compressed = encoder
        .finish()
        .map_err(|e| format!("Failed to finish genesis state compression: {}", e))?;

    let mut hasher = Sha256::new();
    hasher.update(&compressed);
    let digest = hasher.finalize();
    let mut compressed_sha256 = [0u8; 32];
    compressed_sha256.copy_from_slice(&digest[..32]);

    let total_chunks = compressed.len().div_ceil(GENESIS_STATE_CHUNK_BYTES).max(1);
    if total_chunks > u32::MAX as usize {
        return Err("Genesis state bundle produced too many chunks".to_string());
    }

    Ok(compressed
        .chunks(GENESIS_STATE_CHUNK_BYTES)
        .enumerate()
        .map(|(index, data)| GenesisStateChunk {
            version: GENESIS_STATE_BUNDLE_VERSION,
            state_root: state_root.0,
            compression: "gzip".to_string(),
            compressed_len: compressed.len() as u64,
            uncompressed_len: raw.len() as u64,
            compressed_sha256,
            chunk_index: index as u32,
            total_chunks: total_chunks as u32,
            data: data.to_vec(),
        })
        .collect())
}

fn append_genesis_state_bundle_txs(
    genesis_txs: &mut Vec<Transaction>,
    genesis_pubkey: Pubkey,
    chunks: Vec<GenesisStateChunk>,
) -> Result<(), String> {
    for chunk in chunks {
        let chunk_bytes = bincode::serialize(&chunk)
            .map_err(|e| format!("Failed to serialize genesis state chunk: {}", e))?;
        let mut ix_data = Vec::with_capacity(1 + chunk_bytes.len());
        ix_data.push(GENESIS_STATE_CHUNK_OPCODE);
        ix_data.extend_from_slice(&chunk_bytes);

        let instruction = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![genesis_pubkey],
            data: ix_data,
        };
        let message = Message::new(vec![instruction], Hash::default());
        genesis_txs.push(Transaction::new(message));
    }

    Ok(())
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

fn load_genesis_keypair_with_policy(
    path: &std::path::Path,
    password: Option<&str>,
    allow_plaintext: bool,
) -> Result<Keypair, String> {
    load_keypair_with_password_policy(path, password, allow_plaintext)
        .map_err(|err| format!("Failed to load keypair file {}: {}", path.display(), err))
}

fn load_genesis_keypair(path: &std::path::Path) -> Result<Keypair, String> {
    let password = require_runtime_keypair_password("genesis keypair load")?;
    load_genesis_keypair_with_policy(
        path,
        password.as_deref(),
        plaintext_keypair_compat_allowed(),
    )
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
    if artifact_path.contains("genesis-keys/")
        || target_path
            .file_name()
            .and_then(|name| name.to_str())
            .map(|name| name.ends_with("-keypair.json"))
            .unwrap_or(false)
    {
        copy_secure_file(&source_path, &target_path).map_err(|err| {
            format!(
                "Failed to securely copy artifact {} -> {}: {}",
                source_path.display(),
                target_path.display(),
                err
            )
        })?;
    } else {
        std::fs::copy(&source_path, &target_path).map_err(|err| {
            format!(
                "Failed to copy artifact {} -> {}: {}",
                source_path.display(),
                target_path.display(),
                err
            )
        })?;
    }
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

fn explicit_pubkey_list(
    args: &[String],
    config_values: &[String],
    flag: &str,
    label: &str,
) -> Result<Vec<Pubkey>, String> {
    let mut pubkeys = Vec::new();

    for raw in config_values {
        let pubkey = Pubkey::from_base58(raw)
            .map_err(|err| format!("Invalid {} pubkey '{}': {}", label, raw, err))?;
        if !pubkeys.contains(&pubkey) {
            pubkeys.push(pubkey);
        }
    }

    for raw in repeated_flag_values(args, flag) {
        let pubkey = Pubkey::from_base58(&raw)
            .map_err(|err| format!("Invalid {} '{}': {}", flag, raw, err))?;
        if !pubkeys.contains(&pubkey) {
            pubkeys.push(pubkey);
        }
    }

    Ok(pubkeys)
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

fn price_from_usd_8dec(asset: &str, usd: f64) -> Result<u64, String> {
    if !usd.is_finite() || usd <= 0.0 {
        return Err(format!("{asset} price must be a positive finite USD value"));
    }
    let raw = (usd * 100_000_000.0).round();
    if raw > u64::MAX as f64 {
        return Err(format!("{asset} price is too large"));
    }
    Ok(raw as u64)
}

fn validate_genesis_prices(prices: &GenesisPrices, source: &str) -> Result<(), String> {
    let required = [
        ("LICN", prices.licn_usd_8dec),
        ("wSOL", prices.wsol_usd_8dec),
        ("wETH", prices.weth_usd_8dec),
        ("wBNB", prices.wbnb_usd_8dec),
        ("wNEO", prices.wneo_usd_8dec),
        ("wGAS", prices.wgas_usd_8dec),
    ];
    let missing: Vec<&str> = required
        .iter()
        .filter_map(|(asset, price)| if *price == 0 { Some(*asset) } else { None })
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "genesis price source {source} has zero prices for: {}",
            missing.join(", ")
        ))
    }
}

fn load_genesis_prices_file(path: &Path) -> Result<GenesisPrices, String> {
    let contents = std::fs::read_to_string(path).map_err(|err| {
        format!(
            "Failed to read genesis prices file {}: {}",
            path.display(),
            err
        )
    })?;
    let value: serde_json::Value = serde_json::from_str(&contents).map_err(|err| {
        format!(
            "Failed to parse genesis prices file {}: {}",
            path.display(),
            err
        )
    })?;
    for key in [
        "licn_usd_8dec",
        "wsol_usd_8dec",
        "weth_usd_8dec",
        "wbnb_usd_8dec",
        "wneo_usd_8dec",
        "wgas_usd_8dec",
    ] {
        if value.get(key).is_none() {
            return Err(format!(
                "genesis prices file {} is missing required field {key}",
                path.display()
            ));
        }
    }
    let prices: GenesisPrices = serde_json::from_str(&contents).map_err(|err| {
        format!(
            "Failed to parse genesis prices file {}: {}",
            path.display(),
            err
        )
    })?;
    validate_genesis_prices(&prices, &path.display().to_string())?;
    Ok(prices)
}

fn genesis_prices_from_env() -> Result<Option<GenesisPrices>, String> {
    let sol = std::env::var("GENESIS_SOL_USD").ok();
    let eth = std::env::var("GENESIS_ETH_USD").ok();
    let bnb = std::env::var("GENESIS_BNB_USD").ok();
    let neo = std::env::var("GENESIS_NEO_USD").ok();
    let gas = std::env::var("GENESIS_GAS_USD").ok();
    if sol.is_none() && eth.is_none() && bnb.is_none() && neo.is_none() && gas.is_none() {
        return Ok(None);
    }

    let parse_env_price = |name: &str, value: Option<String>| -> Result<u64, String> {
        let value = value.ok_or_else(|| {
            format!(
                "partial genesis price environment override: {name} is missing; set GENESIS_SOL_USD, GENESIS_ETH_USD, GENESIS_BNB_USD, GENESIS_NEO_USD, and GENESIS_GAS_USD together"
            )
        })?;
        let usd = value
            .parse::<f64>()
            .map_err(|err| format!("Invalid {name} value '{value}': {err}"))?;
        price_from_usd_8dec(name, usd)
    };

    let prices = GenesisPrices {
        licn_usd_8dec: GenesisPrices::default().licn_usd_8dec,
        wsol_usd_8dec: parse_env_price("GENESIS_SOL_USD", sol)?,
        weth_usd_8dec: parse_env_price("GENESIS_ETH_USD", eth)?,
        wbnb_usd_8dec: parse_env_price("GENESIS_BNB_USD", bnb)?,
        wneo_usd_8dec: parse_env_price("GENESIS_NEO_USD", neo)?,
        wgas_usd_8dec: parse_env_price("GENESIS_GAS_USD", gas)?,
    };
    validate_genesis_prices(&prices, "environment")?;
    Ok(Some(prices))
}

fn fetch_url(url: &str) -> Result<String, String> {
    let response = ureq::get(url)
        .timeout(Duration::from_secs(10))
        .call()
        .map_err(|err| err.to_string())?;
    response.into_string().map_err(|err| err.to_string())
}

/// Fetch live market prices from Binance REST API for genesis seeding.
/// This runs ONCE during genesis creation — the returned prices are embedded
/// in the genesis block and reused by all joining validators.
fn fetch_binance_genesis_prices() -> Result<GenesisPrices, String> {
    #[derive(serde::Deserialize)]
    struct Ticker {
        symbol: String,
        price: String,
    }

    let url = "https://api.binance.com/api/v3/ticker/price?symbols=[%22SOLUSDT%22,%22ETHUSDT%22,%22BNBUSDT%22,%22NEOUSDT%22,%22GASUSDT%22]";
    let body = fetch_url(url).map_err(|err| format!("Binance price fetch failed: {err}"))?;
    let tickers: Vec<Ticker> = serde_json::from_str(&body)
        .map_err(|err| format!("Failed to parse Binance price JSON: {err}"))?;

    let mut wsol = None;
    let mut weth = None;
    let mut wbnb = None;
    let mut wneo = None;
    let mut wgas = None;
    for t in &tickers {
        let usd: f64 = t
            .price
            .parse()
            .map_err(|err| format!("Invalid Binance {} price '{}': {}", t.symbol, t.price, err))?;
        let price_8dec = price_from_usd_8dec(&t.symbol, usd)?;
        match t.symbol.as_str() {
            "SOLUSDT" => wsol = Some(price_8dec),
            "ETHUSDT" => weth = Some(price_8dec),
            "BNBUSDT" => wbnb = Some(price_8dec),
            "NEOUSDT" => wneo = Some(price_8dec),
            "GASUSDT" => wgas = Some(price_8dec),
            _ => {}
        }
    }
    let prices = GenesisPrices {
        licn_usd_8dec: GenesisPrices::default().licn_usd_8dec,
        wsol_usd_8dec: wsol.ok_or_else(|| "Binance response missing SOLUSDT".to_string())?,
        weth_usd_8dec: weth.ok_or_else(|| "Binance response missing ETHUSDT".to_string())?,
        wbnb_usd_8dec: wbnb.ok_or_else(|| "Binance response missing BNBUSDT".to_string())?,
        wneo_usd_8dec: wneo.ok_or_else(|| "Binance response missing NEOUSDT".to_string())?,
        wgas_usd_8dec: wgas.ok_or_else(|| "Binance response missing GASUSDT".to_string())?,
    };
    validate_genesis_prices(&prices, "Binance")?;
    Ok(prices)
}

fn fetch_coingecko_genesis_prices() -> Result<GenesisPrices, String> {
    let url = "https://api.coingecko.com/api/v3/simple/price?ids=solana,ethereum,binancecoin,neo,gas&vs_currencies=usd";
    let body = fetch_url(url).map_err(|err| format!("CoinGecko price fetch failed: {err}"))?;
    let value: serde_json::Value = serde_json::from_str(&body)
        .map_err(|err| format!("Failed to parse CoinGecko price JSON: {err}"))?;

    let read_price = |id: &str, asset: &str| -> Result<u64, String> {
        let usd = value
            .get(id)
            .and_then(|entry| entry.get("usd"))
            .and_then(|price| price.as_f64())
            .ok_or_else(|| format!("CoinGecko response missing {id}.usd"))?;
        price_from_usd_8dec(asset, usd)
    };

    let prices = GenesisPrices {
        licn_usd_8dec: GenesisPrices::default().licn_usd_8dec,
        wsol_usd_8dec: read_price("solana", "wSOL")?,
        weth_usd_8dec: read_price("ethereum", "wETH")?,
        wbnb_usd_8dec: read_price("binancecoin", "wBNB")?,
        wneo_usd_8dec: read_price("neo", "wNEO")?,
        wgas_usd_8dec: read_price("gas", "wGAS")?,
    };
    validate_genesis_prices(&prices, "CoinGecko")?;
    Ok(prices)
}

fn fetch_live_genesis_prices() -> Result<(GenesisPrices, &'static str), String> {
    let mut errors = Vec::new();
    match fetch_binance_genesis_prices() {
        Ok(prices) => return Ok((prices, "Binance")),
        Err(err) => errors.push(err),
    }
    match fetch_coingecko_genesis_prices() {
        Ok(prices) => return Ok((prices, "CoinGecko")),
        Err(err) => errors.push(err),
    }
    Err(errors.join("; "))
}

fn resolve_genesis_prices(
    network: &str,
    genesis_prices_file: Option<&Path>,
) -> Result<GenesisPrices, String> {
    if let Some(path) = genesis_prices_file {
        let prices = load_genesis_prices_file(path)?;
        info!("  ✓ Genesis prices loaded from {}", path.display());
        return Ok(prices);
    }

    if let Some(prices) = genesis_prices_from_env()? {
        info!("  ✓ Genesis prices loaded from GENESIS_*_USD environment");
        return Ok(prices);
    }

    match fetch_live_genesis_prices() {
        Ok((prices, source)) => {
            info!("  ✓ Genesis prices fetched live from {}", source);
            Ok(prices)
        }
        Err(err) if network == "mainnet" => Err(format!(
            "Mainnet genesis requires explicit or live market prices. Provide --genesis-prices-file or GENESIS_SOL_USD/GENESIS_ETH_USD/GENESIS_BNB_USD/GENESIS_NEO_USD/GENESIS_GAS_USD, or fix live price access. Live fetch errors: {err}"
        )),
        Err(err) => {
            warn!(
                "Live genesis price fetch failed ({}), using compiled defaults for {} only",
                err, network
            );
            Ok(GenesisPrices::default())
        }
    }
}

fn log_genesis_prices(prices: &GenesisPrices) {
    info!(
        "  ✓ Genesis prices frozen: LICN=${:.4}, SOL=${:.2}, ETH=${:.2}, BNB=${:.2}, NEO=${:.2}, GAS=${:.2}",
        prices.licn_usd_8dec as f64 / 100_000_000.0,
        prices.wsol_usd_8dec as f64 / 100_000_000.0,
        prices.weth_usd_8dec as f64 / 100_000_000.0,
        prices.wbnb_usd_8dec as f64 / 100_000_000.0,
        prices.wneo_usd_8dec as f64 / 100_000_000.0,
        prices.wgas_usd_8dec as f64 / 100_000_000.0,
    );
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
    let genesis_prices_file = flag_value(&args, "--genesis-prices-file").map(PathBuf::from);

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
            error!("Usage: lichen-genesis --network <mainnet|testnet> [--prepare-wallet --output-dir <path>] [--wallet-file <path>] [--initial-validator <base58>] [--bridge-validator <base58>] [--oracle-operator <base58>] [--db-path <path>] [--config <path>] [--genesis-prices-file <path>]");
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

    let bridge_validators = match explicit_pubkey_list(
        &args,
        &genesis_config.bridge_validators,
        "--bridge-validator",
        "bridge validator",
    ) {
        Ok(validators) if validators.len() >= 2 => validators,
        Ok(validators) => {
            error!(
                "Genesis creation requires at least 2 bridge validators, got {}. Pass --bridge-validator <base58> for each bridge operator.",
                validators.len()
            );
            std::process::exit(1);
        }
        Err(err) => {
            error!("{}", err);
            std::process::exit(1);
        }
    };

    let oracle_operators = match explicit_pubkey_list(
        &args,
        &genesis_config.oracle_operators,
        "--oracle-operator",
        "oracle operator",
    ) {
        Ok(operators) if operators.len() >= 2 => operators,
        Ok(operators) => {
            error!(
                "Genesis creation requires at least 2 oracle operators, got {}. Pass --oracle-operator <base58> for each oracle operator.",
                operators.len()
            );
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
    genesis_config.bridge_validators = bridge_validators.iter().map(|pk| pk.to_base58()).collect();
    genesis_config.oracle_operators = oracle_operators.iter().map(|pk| pk.to_base58()).collect();

    // ════════════════════════════════════════════════════════════════════
    // RESOLVE MARKET PRICES — frozen into genesis config forever
    // ════════════════════════════════════════════════════════════════════
    genesis_config.genesis_prices =
        match resolve_genesis_prices(network_str, genesis_prices_file.as_deref()) {
            Ok(prices) => prices,
            Err(err) => {
                error!("{}", err);
                std::process::exit(1);
            }
        };
    log_genesis_prices(&genesis_config.genesis_prices);

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
    if let Err(err) =
        genesis_bootstrap_bridge_committee(&state, &genesis_pubkey, "GENESIS:", &bridge_validators)
    {
        error!("Failed to bootstrap bridge committee: {}", err);
        std::process::exit(1);
    };
    genesis_create_trading_pairs(&state, &genesis_pubkey, "GENESIS:", gp);
    genesis_set_fee_exempt_contracts(&state, &genesis_pubkey, "GENESIS:");
    if let Err(err) = genesis_seed_oracle(
        &state,
        &genesis_pubkey,
        "GENESIS:",
        genesis_timestamp,
        gp,
        &oracle_operators,
    ) {
        error!("Failed to seed oracle: {}", err);
        std::process::exit(1);
    };
    genesis_seed_margin_prices(&state, &genesis_pubkey, genesis_timestamp, gp);
    genesis_seed_analytics_prices(&state, &genesis_pubkey, genesis_timestamp, gp);
    genesis_seed_consensus_oracle_prices(&state, 0, gp);

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

    match genesis_config.seed_initial_restrictions(&state, genesis_pubkey) {
        Ok(0) => {}
        Ok(count) => info!("  ✓ Seeded {} initial genesis restriction(s)", count),
        Err(err) => {
            error!("Failed to seed initial genesis restrictions: {}", err);
            std::process::exit(1);
        }
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

    // Persist metrics before exporting canonical genesis state. Metrics live in
    // CF_STATS outside the state root, but imported validators need the same
    // operational counters and protocol parameters immediately after slot 0.
    if let Err(e) = state.save_metrics_counters() {
        error!("Failed to flush metrics before genesis state export: {}", e);
        std::process::exit(1);
    }

    // ════════════════════════════════════════════════════════════════════
    // EMBED CANONICAL GENESIS STATE (opcode 41)
    // ════════════════════════════════════════════════════════════════════
    let state_root = state.compute_state_root();
    match build_genesis_state_bundle(&state, state_root)
        .and_then(|bundle| encode_genesis_state_chunks(state_root, &bundle))
        .and_then(|chunks| {
            let chunk_count = chunks.len();
            append_genesis_state_bundle_txs(&mut genesis_txs, genesis_pubkey, chunks)?;
            Ok(chunk_count)
        }) {
        Ok(chunk_count) => info!(
            "  ✓ Canonical genesis state embedded in {} opcode-41 chunks",
            chunk_count
        ),
        Err(err) => {
            error!("Failed to embed canonical genesis state: {}", err);
            std::process::exit(1);
        }
    }

    // ════════════════════════════════════════════════════════════════════
    // CREATE GENESIS BLOCK — state_root captures FULL state (accounts +
    // contracts + oracle + analytics + margin) per Cosmos/Substrate standard.
    // ════════════════════════════════════════════════════════════════════
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
    fn price_from_usd_8dec_rounds_to_integer_units() {
        assert_eq!(
            price_from_usd_8dec("wSOL", 86.789_123_456).unwrap(),
            8_678_912_346
        );
        assert!(price_from_usd_8dec("wSOL", 0.0).is_err());
        assert!(price_from_usd_8dec("wSOL", f64::NAN).is_err());
    }

    #[test]
    fn load_genesis_prices_file_accepts_raw_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("genesis-prices.json");
        std::fs::write(
            &path,
            serde_json::json!({
                "licn_usd_8dec": 10_000_000u64,
                "wsol_usd_8dec": 8_678_000_000u64,
                "weth_usd_8dec": 199_934_000_000u64,
                "wbnb_usd_8dec": 60_978_000_000u64,
                "wneo_usd_8dec": 307_500_000u64,
                "wgas_usd_8dec": 165_000_000u64,
                "source": "operator-snapshot"
            })
            .to_string(),
        )
        .unwrap();

        let prices = load_genesis_prices_file(&path).unwrap();
        assert_eq!(prices.wsol_usd_8dec, 8_678_000_000);
    }

    #[test]
    fn load_genesis_prices_file_rejects_zero_price() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("genesis-prices.json");
        std::fs::write(
            &path,
            serde_json::json!({
                "licn_usd_8dec": 10_000_000u64,
                "wsol_usd_8dec": 0u64,
                "weth_usd_8dec": 199_934_000_000u64,
                "wbnb_usd_8dec": 60_978_000_000u64,
                "wneo_usd_8dec": 307_500_000u64,
                "wgas_usd_8dec": 165_000_000u64
            })
            .to_string(),
        )
        .unwrap();

        let err = load_genesis_prices_file(&path).unwrap_err();
        assert!(err.contains("wSOL"));
    }

    #[test]
    fn load_genesis_prices_file_rejects_missing_neo_price() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("genesis-prices.json");
        std::fs::write(
            &path,
            serde_json::json!({
                "licn_usd_8dec": 10_000_000u64,
                "wsol_usd_8dec": 8_678_000_000u64,
                "weth_usd_8dec": 199_934_000_000u64,
                "wbnb_usd_8dec": 60_978_000_000u64,
                "wgas_usd_8dec": 165_000_000u64
            })
            .to_string(),
        )
        .unwrap();

        let err = load_genesis_prices_file(&path).unwrap_err();
        assert!(err.contains("wneo_usd_8dec"));
    }

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

        let loaded = load_genesis_keypair_with_policy(&path, None, true).unwrap();
        assert_eq!(loaded.to_seed(), keypair.to_seed());

        let _ = std::fs::remove_file(path);
    }
}
