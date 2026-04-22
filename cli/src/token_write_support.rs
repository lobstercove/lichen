use anyhow::Result;
use lichen_core::Pubkey;
use std::path::PathBuf;

use crate::client::RpcClient;
use crate::contract_args_support::encode_dual_address_amount_args;
use crate::contract_poll_support::wait_for_confirmation;
use crate::keypair_manager::KeypairManager;
use crate::token_context_support::{resolve_token_context, token_decimals_from_context};
use crate::token_units_support::scale_whole_token_amount;

pub(super) async fn handle_token_mint(
    client: &RpcClient,
    keypair_mgr: &KeypairManager,
    token: String,
    amount: u64,
    to: Option<String>,
    keypair: Option<PathBuf>,
) -> Result<()> {
    let path = keypair.unwrap_or_else(|| keypair_mgr.default_keypair_path());
    let minter = keypair_mgr.load_keypair(&path)?;
    let recipient = to.unwrap_or_else(|| minter.pubkey().to_base58());
    let recipient_pubkey = Pubkey::from_base58(&recipient)
        .map_err(|error| anyhow::anyhow!("Invalid recipient address: {}", error))?;
    let context = resolve_token_context(client, &token).await?;
    let decimals = token_decimals_from_context(&context);
    let contract_addr = context.contract_addr;
    let amount_base_units = scale_whole_token_amount(amount, decimals)?;

    println!("🪙 Minting {} whole tokens to {}", amount, recipient);

    let signature = client
        .call_contract(
            &minter,
            &contract_addr,
            "mint".to_string(),
            encode_dual_address_amount_args(&minter.pubkey(), &recipient_pubkey, amount_base_units),
            0,
        )
        .await?;

    match wait_for_confirmation(client, &signature, 10).await {
        Ok(true) => println!("✅ Tokens minted! Sig: {}", signature),
        Ok(false) => {
            println!("📝 Mint submitted (pending confirmation): {}", signature)
        }
        Err(error) => println!(
            "⚠️  Mint submitted but failed on-chain: {} (sig: {})",
            error, signature
        ),
    }

    Ok(())
}

pub(super) async fn handle_token_send(
    client: &RpcClient,
    keypair_mgr: &KeypairManager,
    token: String,
    to: String,
    amount: u64,
    keypair: Option<PathBuf>,
) -> Result<()> {
    let path = keypair.unwrap_or_else(|| keypair_mgr.default_keypair_path());
    let sender = keypair_mgr.load_keypair(&path)?;
    let recipient_pubkey = Pubkey::from_base58(&to)
        .map_err(|error| anyhow::anyhow!("Invalid recipient address: {}", error))?;
    let context = resolve_token_context(client, &token).await?;
    let decimals = token_decimals_from_context(&context);
    let contract_addr = context.contract_addr;
    let amount_base_units = scale_whole_token_amount(amount, decimals)?;

    println!("📤 Sending {} whole tokens to {}", amount, to);

    let signature = client
        .call_contract(
            &sender,
            &contract_addr,
            "transfer".to_string(),
            encode_dual_address_amount_args(&sender.pubkey(), &recipient_pubkey, amount_base_units),
            0,
        )
        .await?;

    match wait_for_confirmation(client, &signature, 10).await {
        Ok(true) => println!("✅ Tokens sent! Sig: {}", signature),
        Ok(false) => {
            println!(
                "📝 Transfer submitted (pending confirmation): {}",
                signature
            )
        }
        Err(error) => println!(
            "⚠️  Transfer submitted but failed on-chain: {} (sig: {})",
            error, signature
        ),
    }

    Ok(())
}
