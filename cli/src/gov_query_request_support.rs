use anyhow::Result;
use lichen_core::Pubkey;

use crate::client::RpcClient;
use crate::gov_common_support::load_query_keypair;
use crate::keypair_manager::KeypairManager;

pub(super) struct GovQueryRequest<'a> {
    pub(super) function: &'a str,
    pub(super) data: Vec<u8>,
    pub(super) error_subject: &'a str,
    pub(super) success_hint: &'a str,
}

pub(super) async fn submit_gov_query(
    client: &RpcClient,
    keypair_mgr: &KeypairManager,
    dao_addr: &Pubkey,
    request: GovQueryRequest<'_>,
) -> Result<()> {
    let query_keypair = load_query_keypair(keypair_mgr)?;

    match client
        .call_contract(
            &query_keypair,
            dao_addr,
            request.function.to_string(),
            request.data,
            0,
        )
        .await
    {
        Ok(signature) => {
            println!("📝 Query submitted (sig: {})", signature);
            println!("💡 {}", request.success_hint);
        }
        Err(error) => {
            println!("⚠️  Could not query {}: {}", request.error_subject, error);
            println!("💡 Ensure the LichenDAO contract is deployed at the well-known address");
        }
    }

    Ok(())
}
