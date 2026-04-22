use anyhow::Result;

use crate::client::RpcClient;
use crate::contract_readonly_support::{decode_readonly_u64, parse_json_u64};
use crate::token_context_support::{
    resolve_token_context, token_decimals_from_context, token_name, token_registry_field,
    token_symbol,
};
use crate::token_units_support::format_token_amount;

pub(super) async fn handle_token_info(client: &RpcClient, token: String) -> Result<()> {
    let context = resolve_token_context(client, &token).await?;
    let name = token_name(&context);
    let symbol = token_symbol(&context);
    let decimals = token_decimals_from_context(&context);
    let template = token_registry_field(context.registry.as_ref(), "template")
        .unwrap_or_else(|| "token".to_string());
    let crate::token_context_support::ResolvedTokenContext {
        contract_addr,
        info,
        ..
    } = context;

    println!("🪙 Token Info: {}", token);
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");

    match info {
        Ok(info) => {
            println!("📍 Address: {}", info.address);
            println!("👤 Owner:   {}", info.owner);
            println!("📏 Code size: {} bytes", info.code_size);
            let mut total_supply = info
                .token_metadata
                .as_ref()
                .and_then(|meta| meta.get("total_supply"))
                .and_then(parse_json_u64);

            if total_supply.is_none() {
                if let Ok(result) = client
                    .call_readonly_contract(&contract_addr, "total_supply", Vec::new(), None)
                    .await
                {
                    total_supply = decode_readonly_u64(&result, "total_supply").ok();
                }
            }

            if let Some(value) = &name {
                println!("🏷️  Name:    {}", value);
            }
            if let Some(value) = &symbol {
                println!("🔤 Symbol:  {}", value);
            }
            println!("🧩 Template: {}", template);
            println!("🔢 Decimals: {}", decimals);
            if let Some(supply) = total_supply {
                let symbol_suffix = symbol
                    .as_ref()
                    .map(|value| format!(" {}", value))
                    .unwrap_or_default();
                println!(
                    "💰 Supply:  {}{} ({} base units)",
                    format_token_amount(supply, decimals),
                    symbol_suffix,
                    supply
                );
            }
            if info.deployed_at > 0 {
                println!("📅 Deployed at slot: {}", info.deployed_at);
            }
        }
        Err(error) => {
            println!("⚠️  Token contract not found: {}", error);
            println!("💡 Verify the token address or symbol is registered on-chain.");
        }
    }

    Ok(())
}
