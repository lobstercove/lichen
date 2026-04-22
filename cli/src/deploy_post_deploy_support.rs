use lichen_core::{Keypair, Pubkey};

use crate::cli_args::ContractTemplate;
use crate::client::{RpcClient, SymbolRegistration};
use crate::deploy_output_support::print_deploy_symbol_registration_status;
use crate::symbol_registration_support::ensure_symbol_registration;

pub(super) struct DeploySymbolRegistration<'a> {
    pub(super) symbol: &'a str,
    pub(super) name: Option<&'a str>,
    pub(super) template: Option<&'a ContractTemplate>,
    pub(super) decimals: Option<u8>,
    pub(super) metadata: Option<&'a serde_json::Value>,
}

pub(super) async fn handle_deploy_symbol_registration(
    client: &RpcClient,
    deployer: &Keypair,
    contract_addr: &Pubkey,
    registration: Option<DeploySymbolRegistration<'_>>,
) {
    let Some(DeploySymbolRegistration {
        symbol,
        name,
        template,
        decimals,
        metadata,
    }) = registration
    else {
        return;
    };

    let template_str = template.map(|entry| entry.to_string());
    let status = ensure_symbol_registration(
        client,
        deployer,
        contract_addr,
        SymbolRegistration {
            symbol,
            name,
            template: template_str.as_deref(),
            decimals,
            metadata: metadata.cloned(),
        },
        3,
        10,
    )
    .await;

    print_deploy_symbol_registration_status(
        status,
        symbol,
        contract_addr,
        name,
        template,
        decimals,
    );
}
