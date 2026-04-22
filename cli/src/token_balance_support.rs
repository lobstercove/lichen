use anyhow::Result;
use lichen_core::Pubkey;
use std::path::PathBuf;

use crate::client::RpcClient;
use crate::contract_args_support::encode_single_address_arg;
use crate::contract_readonly_support::decode_readonly_u64;
use crate::keypair_manager::KeypairManager;
use crate::token_context_support::{
    resolve_token_context, token_decimals_from_context, token_symbol,
};
use crate::token_units_support::format_token_amount;

pub(super) async fn handle_token_balance(
    client: &RpcClient,
    keypair_mgr: &KeypairManager,
    token: String,
    address: Option<String>,
    keypair: Option<PathBuf>,
) -> Result<()> {
    let account = if let Some(value) = address {
        value
    } else {
        let path = keypair.unwrap_or_else(|| keypair_mgr.default_keypair_path());
        let keypair = keypair_mgr.load_keypair(&path)?;
        keypair.pubkey().to_base58()
    };

    let account_pubkey = Pubkey::from_base58(&account)
        .map_err(|error| anyhow::anyhow!("Invalid account address: {}", error))?;
    let context = resolve_token_context(client, &token).await?;
    let decimals = token_decimals_from_context(&context);
    let symbol = token_symbol(&context).unwrap_or_default();
    let crate::token_context_support::ResolvedTokenContext {
        contract_addr,
        contract_addr_b58,
        ..
    } = context;

    println!("🪙 Token Balance");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("📍 Token:   {}", contract_addr_b58);
    println!("👤 Account: {}", account);
    println!();

    match client
        .call_readonly_contract(
            &contract_addr,
            "balance_of",
            encode_single_address_arg(&account_pubkey),
            None,
        )
        .await
    {
        Ok(result) => match decode_readonly_u64(&result, "balance_of") {
            Ok(balance) => {
                let suffix = if symbol.is_empty() {
                    String::new()
                } else {
                    format!(" {}", symbol)
                };
                println!(
                    "💰 Balance: {}{} ({} base units)",
                    format_token_amount(balance, decimals),
                    suffix,
                    balance
                );
            }
            Err(error) => {
                println!("⚠️  Could not decode token balance: {}", error);
            }
        },
        Err(error) => {
            println!("⚠️  Could not query token balance: {}", error);
            println!("💡 Ensure the token contract is deployed and exports balance_of(address).");
        }
    }

    Ok(())
}
