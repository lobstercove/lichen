use lichen_core::{Keypair, Pubkey};

use crate::client::RpcClient;
use crate::token_contract_setup_support::{initialize_token_contract, mint_initial_supply};
use crate::token_output_support::print_token_deploy_success;
use crate::token_post_registration_support::{
    handle_token_symbol_registration, TokenSymbolRegistration,
};

pub(super) struct TokenPostDeploy<'a> {
    pub(super) name: &'a str,
    pub(super) symbol: &'a str,
    pub(super) decimals: u8,
    pub(super) initial_supply: Option<u64>,
    pub(super) registry_metadata: Option<serde_json::Value>,
}

pub(super) async fn handle_token_post_deploy(
    client: &RpcClient,
    deployer: &Keypair,
    contract_addr: &Pubkey,
    deployment: TokenPostDeploy<'_>,
) {
    let TokenPostDeploy {
        name,
        symbol,
        decimals,
        initial_supply,
        registry_metadata,
    } = deployment;

    let symbol_registered = handle_token_symbol_registration(
        client,
        deployer,
        contract_addr,
        TokenSymbolRegistration {
            name,
            symbol,
            decimals,
            registry_metadata,
        },
    )
    .await;

    initialize_token_contract(client, deployer, contract_addr).await;

    if let Some(supply) = initial_supply.filter(|value| *value > 0) {
        mint_initial_supply(client, deployer, contract_addr, decimals, supply).await;
    }

    print_token_deploy_success(contract_addr, symbol, initial_supply, symbol_registered);
}
