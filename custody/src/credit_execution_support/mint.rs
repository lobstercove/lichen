use super::*;
use lichen_core::keypair_file::{
    load_keypair_with_password_policy, plaintext_keypair_compat_allowed,
    require_runtime_keypair_password,
};

#[derive(Debug, Deserialize)]
struct TreasuryKeyFile {
    secret_key: String,
}

/// Build a Lichen contract Call instruction for the "mint" function.
///
/// Payload format:
///   {"Call": {"function": "mint", "args": [...], "value": 0}}
///
/// Where args is a flat byte array: [caller_32_bytes, to_32_bytes, amount_8_bytes_le]
fn build_contract_mint_instruction(
    contract_pubkey: &Pubkey,
    caller: &Pubkey,
    to: &Pubkey,
    amount: u64,
) -> Instruction {
    let mut args: Vec<u8> = Vec::with_capacity(72);
    args.extend_from_slice(caller.as_ref());
    args.extend_from_slice(to.as_ref());
    args.extend_from_slice(&amount.to_le_bytes());

    let payload = serde_json::json!({
        "Call": {
            "function": "mint",
            "args": args.iter().map(|b| *b as u64).collect::<Vec<u64>>(),
            "value": 0
        }
    });
    let data = serde_json::to_vec(&payload).expect("json encode");

    Instruction {
        program_id: Pubkey::new(LICN_CONTRACT_PROGRAM),
        accounts: vec![*caller, *contract_pubkey],
        data,
    }
}

fn load_treasury_keypair(path: &Path) -> Result<Keypair, String> {
    let password = require_runtime_keypair_password("custody treasury keypair load")?;
    match load_keypair_with_password_policy(
        path,
        password.as_deref(),
        plaintext_keypair_compat_allowed(),
    ) {
        Ok(keypair) => return Ok(keypair),
        Err(canonical_err) if !plaintext_keypair_compat_allowed() => {
            return Err(format!(
                "failed to load canonical treasury keypair {}: {}",
                path.display(),
                canonical_err
            ));
        }
        Err(_) => {}
    }

    let json = std::fs::read_to_string(path).map_err(|error| format!("read: {}", error))?;
    let parsed: TreasuryKeyFile =
        serde_json::from_str(&json).map_err(|error| format!("parse: {}", error))?;
    let bytes = hex::decode(parsed.secret_key).map_err(|error| format!("hex: {}", error))?;
    if bytes.len() != 32 {
        return Err("invalid treasury key length".to_string());
    }
    let mut seed = [0u8; 32];
    seed.copy_from_slice(&bytes);
    Ok(Keypair::from_seed(&seed))
}

pub(super) async fn submit_wrapped_credit(
    state: &CustodyState,
    job: &CreditJob,
) -> Result<String, String> {
    let rpc_url = state
        .config
        .licn_rpc_url
        .as_ref()
        .ok_or_else(|| "missing CUSTODY_LICHEN_RPC_URL".to_string())?;
    let keypair_path = state
        .config
        .treasury_keypair_path
        .as_ref()
        .ok_or_else(|| "missing CUSTODY_TREASURY_KEYPAIR".to_string())?;

    let contract_addr_str =
        resolve_token_contract(&state.config, &job.source_chain, &job.source_asset).ok_or_else(
            || {
                format!(
                    "no wrapped token contract for chain={} asset={}",
                    job.source_chain, job.source_asset
                )
            },
        )?;

    let contract_pubkey = Pubkey::from_base58(&contract_addr_str)
        .map_err(|_| format!("invalid contract address: {}", contract_addr_str))?;

    let treasury_keypair = load_treasury_keypair(Path::new(keypair_path))?;
    let to_pubkey = Pubkey::from_base58(&job.to_address)
        .map_err(|_| "invalid recipient address".to_string())?;

    let instruction = build_contract_mint_instruction(
        &contract_pubkey,
        &treasury_keypair.pubkey(),
        &to_pubkey,
        job.amount_spores,
    );

    let blockhash = licn_get_recent_blockhash(&state.http, rpc_url).await?;
    let message = Message::new(vec![instruction], blockhash);
    let signature = treasury_keypair.sign(&message.serialize());
    let mut tx = Transaction::new(message);
    tx.signatures.push(signature);

    let tx_bytes = tx.to_wire();
    let tx_base64 = base64::engine::general_purpose::STANDARD.encode(tx_bytes);

    let token_label = match job.source_asset.as_str() {
        "usdt" | "usdc" => "lUSD",
        "sol" => "wSOL",
        "eth" => "wETH",
        "bnb" => "wBNB",
        _ => "UNKNOWN",
    };
    info!(
        "minting {} {} to {} (deposit={})",
        job.amount_spores, token_label, job.to_address, job.deposit_id
    );

    licn_send_transaction(&state.http, rpc_url, &tx_base64).await
}
