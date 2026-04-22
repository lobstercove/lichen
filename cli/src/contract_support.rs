use anyhow::Result;
use lichen_core::Pubkey;

use crate::client::RpcClient;

pub(super) async fn resolve_token_contract(client: &RpcClient, token: &str) -> Result<Pubkey> {
    if let Ok(address) = Pubkey::from_base58(token) {
        return Ok(address);
    }

    match client.resolve_symbol(token).await? {
        Some(address) => Ok(address),
        None => anyhow::bail!(
            "'{}' is neither a valid contract address nor a registered token symbol",
            token
        ),
    }
}
