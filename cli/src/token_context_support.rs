use anyhow::Result;
use lichen_core::Pubkey;

use crate::client::{ContractInfo, RpcClient};
use crate::contract_support::resolve_token_contract;
use crate::token_amount_support::token_decimals;

pub(super) struct ResolvedTokenContext {
    pub(super) contract_addr: Pubkey,
    pub(super) contract_addr_b58: String,
    pub(super) registry: Option<serde_json::Value>,
    pub(super) info: Result<ContractInfo>,
}

pub(super) async fn resolve_token_context(
    client: &RpcClient,
    token: &str,
) -> Result<ResolvedTokenContext> {
    let contract_addr = resolve_token_contract(client, token).await?;
    let contract_addr_b58 = contract_addr.to_base58();
    let registry = match client.get_symbol_by_program(&contract_addr_b58).await {
        Ok(value) if value.is_object() => Some(value),
        _ => None,
    };
    let info = client.get_contract_info(&contract_addr_b58).await;

    Ok(ResolvedTokenContext {
        contract_addr,
        contract_addr_b58,
        registry,
        info,
    })
}

pub(super) fn token_registry_field(
    registry: Option<&serde_json::Value>,
    field: &str,
) -> Option<String> {
    registry
        .and_then(|entry| entry.get(field))
        .and_then(|value| value.as_str())
        .map(|value| value.to_string())
}

pub(super) fn token_name(context: &ResolvedTokenContext) -> Option<String> {
    token_registry_field(context.registry.as_ref(), "name").or_else(|| {
        context
            .info
            .as_ref()
            .ok()
            .and_then(|info| token_metadata_field(Some(info), "token_name"))
    })
}

pub(super) fn token_symbol(context: &ResolvedTokenContext) -> Option<String> {
    token_registry_field(context.registry.as_ref(), "symbol").or_else(|| {
        context
            .info
            .as_ref()
            .ok()
            .and_then(|info| token_metadata_field(Some(info), "token_symbol"))
    })
}

pub(super) fn token_decimals_from_context(context: &ResolvedTokenContext) -> u8 {
    token_decimals(context.registry.as_ref(), context.info.as_ref().ok())
}

fn token_metadata_field(info: Option<&ContractInfo>, field: &str) -> Option<String> {
    info.and_then(|info| {
        info.token_metadata
            .as_ref()
            .and_then(|meta| meta.get(field))
            .and_then(|value| value.as_str())
            .map(|value| value.to_string())
    })
}
