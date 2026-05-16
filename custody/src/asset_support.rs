use super::*;

pub(super) fn is_solana_stablecoin(asset: &str) -> bool {
    matches!(asset, "usdc" | "usdt")
}

pub(super) fn ensure_solana_config(config: &CustodyConfig) -> Result<(), String> {
    if config.solana_rpc_url.is_none() {
        return Err("missing CUSTODY_SOLANA_RPC_URL".to_string());
    }
    Ok(())
}

pub(super) fn solana_mint_for_asset(config: &CustodyConfig, asset: &str) -> Result<String, String> {
    match asset {
        "usdc" => Ok(config.solana_usdc_mint.clone()),
        "usdt" => Ok(config.solana_usdt_mint.clone()),
        _ => Err("unsupported solana token".to_string()),
    }
}

pub(super) fn evm_contract_for_asset(
    config: &CustodyConfig,
    asset: &str,
) -> Result<String, String> {
    match asset {
        "usdc" => Ok(config.evm_usdc_contract.clone()),
        "usdt" => Ok(config.evm_usdt_contract.clone()),
        _ => Err("unsupported evm token".to_string()),
    }
}

/// Returns the native decimal precision for a given (chain, asset) pair.
///
/// Used by deposit → credit conversion AND withdrawal → outbound conversion.
///
/// Native tokens:
///   ETH on Ethereum:             18 decimals (wei)
///   BNB on BSC:                  18 decimals (wei)
///   GAS on Neo X:                18 decimals (read-only WGAS10 verification)
///   SOL on Solana:               9 decimals (lamports)
///
/// ERC-20 / SPL tokens:
///   USDT/USDC on Ethereum:       6 decimals
///   USDT/USDC on BSC (BEP-20):  18 decimals
///   USDT/USDC on Solana (SPL):   6 decimals
pub(super) fn source_chain_decimals(chain: &str, asset: &str) -> Result<u32, String> {
    let chain = chain.trim().to_ascii_lowercase();
    let asset = asset.trim().to_ascii_lowercase();
    match (chain.as_str(), asset.as_str()) {
        ("eth" | "ethereum", "eth") => Ok(18),
        ("bsc" | "bnb", "bnb") => Ok(18),
        ("neox" | "neo-x" | "neo_x", "gas") => Ok(18),
        ("eth" | "ethereum", "usdt" | "usdc") => Ok(6),
        ("bsc" | "bnb", "usdt" | "usdc") => Ok(18),
        ("sol" | "solana", "sol") => Ok(9),
        ("sol" | "solana", "usdt" | "usdc") => Ok(6),
        _ => Err(format!(
            "unsupported source decimals for chain={} asset={}",
            chain, asset
        )),
    }
}

/// Convert Lichen spores (9 decimals) to the target chain's native amount.
///
/// Inverse of the deposit conversion in `build_credit_job`.
pub(super) fn spores_to_chain_amount(
    spores: u64,
    chain: &str,
    asset: &str,
) -> Result<u128, String> {
    let target_decimals = source_chain_decimals(chain, asset)?;
    if target_decimals > 9 {
        Ok((spores as u128).saturating_mul(10u128.pow(target_decimals - 9)))
    } else if target_decimals < 9 {
        let divisor = 10u128.pow(9 - target_decimals);
        let spores = spores as u128;
        if spores % divisor != 0 {
            return Err(format!(
                "non-exact withdrawal decimal conversion rejected (spores={spores}, div={divisor}, chain={chain}, asset={asset})"
            ));
        }
        Ok(spores / divisor)
    } else {
        Ok(spores as u128)
    }
}

/// Resolve deposited asset → Lichen wrapped token contract address.
///
/// Mapping:
///   sol (any chain)          → wSOL contract
///   eth (any chain)          → wETH contract
///   bnb (any chain)          → wBNB contract
///   gas on Neo X             → wGAS contract
///   neo on Neo X             → wNEO contract only when NEO source route is configured
///   usdt, usdc (any chain)   → lUSD contract (unified stablecoin)
pub(super) fn resolve_token_contract(
    config: &CustodyConfig,
    chain: &str,
    asset: &str,
) -> Option<String> {
    let canonical_chain = canonical_evm_chain(chain);
    match asset {
        "sol" if matches!(chain, "sol" | "solana") => config.wsol_contract_addr.clone(),
        "eth" if canonical_chain == Some("ethereum") => config.weth_contract_addr.clone(),
        "bnb" if canonical_chain == Some("bsc") => config.wbnb_contract_addr.clone(),
        "gas" if canonical_chain == Some("neox") => config.wgas_contract_addr.clone(),
        "neo" if canonical_chain == Some("neox") && config.neox_neo_token_contract.is_some() => {
            config.wneo_contract_addr.clone()
        }
        "usdt" | "usdc" => config.musd_contract_addr.clone(),
        _ => None,
    }
}

pub(super) fn derive_associated_token_address(owner: &str, mint: &str) -> Result<String, String> {
    let owner_key = decode_solana_pubkey(owner)?;
    let mint_key = decode_solana_pubkey(mint)?;
    let token_program = decode_solana_pubkey(SOLANA_TOKEN_PROGRAM)?;
    let associated_program = decode_solana_pubkey(SOLANA_ASSOCIATED_TOKEN_PROGRAM)?;
    let seeds: [&[u8]; 3] = [&owner_key, &token_program, &mint_key];
    let address = find_program_address(&seeds, &associated_program)?;
    Ok(encode_solana_pubkey(&address))
}

pub(super) fn derive_associated_token_address_from_str(
    owner: &str,
    mint: &str,
) -> Result<String, String> {
    derive_associated_token_address(owner, mint)
}

pub(super) async fn ensure_associated_token_account(
    state: &CustodyState,
    owner: &str,
    mint: &str,
    ata: &str,
) -> Result<(), String> {
    ensure_associated_token_account_for_str(state, owner, mint, ata).await
}

pub(super) async fn ensure_associated_token_account_for_str(
    state: &CustodyState,
    owner: &str,
    mint: &str,
    ata: &str,
) -> Result<(), String> {
    let url = state
        .config
        .solana_rpc_url
        .as_ref()
        .ok_or_else(|| "missing CUSTODY_SOLANA_RPC_URL".to_string())?;

    if solana_get_account_exists(&state.http, url, ata).await? {
        return Ok(());
    }

    let owner_key = decode_solana_pubkey(owner)?;
    let mint_key = decode_solana_pubkey(mint)?;
    let ata_key = decode_solana_pubkey(ata)?;

    let fee_payer = if let Some(ref fee_payer_path) = state.config.solana_fee_payer_keypair_path {
        load_solana_keypair(fee_payer_path)?
    } else {
        derive_solana_keypair("custody/fee-payer/solana", &state.config.master_seed)?
    };

    let system_program = decode_solana_pubkey(SOLANA_SYSTEM_PROGRAM)?;
    let token_program = decode_solana_pubkey(SOLANA_TOKEN_PROGRAM)?;
    let rent_sysvar = decode_solana_pubkey(SOLANA_RENT_SYSVAR)?;
    let associated_program = decode_solana_pubkey(SOLANA_ASSOCIATED_TOKEN_PROGRAM)?;

    let account_keys = vec![
        fee_payer.pubkey,
        ata_key,
        owner_key,
        mint_key,
        system_program,
        token_program,
        rent_sysvar,
        associated_program,
    ];

    let header = SolanaMessageHeader {
        num_required_signatures: 1,
        num_readonly_signed: 0,
        num_readonly_unsigned: 6,
    };

    let instruction = SolanaInstruction {
        program_id_index: 7,
        account_indices: vec![0, 1, 2, 3, 4, 5, 6],
        data: Vec::new(),
    };

    let recent_blockhash = solana_get_latest_blockhash(&state.http, url).await?;
    let message = build_solana_message_with_instructions(
        header,
        &account_keys,
        &recent_blockhash,
        &[instruction],
    );
    let signature = fee_payer.sign(&message);
    let tx = build_solana_transaction(&[signature], &message);
    solana_send_transaction(&state.http, url, &tx).await?;
    Ok(())
}

pub(super) fn load_solana_keypair(path: &str) -> Result<SimpleSolanaKeypair, String> {
    let json = std::fs::read_to_string(path).map_err(|e| format!("read: {}", e))?;
    let bytes: Vec<u8> = serde_json::from_str(&json).map_err(|e| format!("parse: {}", e))?;
    if bytes.len() != 64 {
        return Err("invalid keypair length".to_string());
    }
    let mut seed = [0u8; 32];
    seed.copy_from_slice(&bytes[..32]);
    let signing_key = ed25519_dalek::SigningKey::from_bytes(&seed);
    let pubkey = signing_key.verifying_key().to_bytes();
    Ok(SimpleSolanaKeypair {
        signing_key,
        pubkey,
    })
}
