use anyhow::Result;

use crate::client::RpcClient;
use crate::deploy_readiness_support::wait_for_deploy_readiness;
use crate::token_create_post_deploy_support::{handle_token_post_deploy, TokenPostDeploy};
use crate::token_create_prepare_support::PreparedTokenCreate;
use crate::token_output_support::{
    print_token_deploy_preamble, report_token_deploy_readiness, TokenDeployPreamble,
};

pub(super) async fn finalize_token_create(
    client: &RpcClient,
    prepared: PreparedTokenCreate,
) -> Result<()> {
    let PreparedTokenCreate {
        name,
        symbol,
        wasm,
        decimals,
        initial_supply,
        deployer,
        registry_metadata,
        wasm_code,
        contract_addr,
        init_data_bytes,
    } = prepared;

    print_token_deploy_preamble(TokenDeployPreamble {
        name: &name,
        symbol: &symbol,
        wasm: &wasm,
        wasm_len: wasm_code.len(),
        contract_addr: &contract_addr,
        creator: &deployer.pubkey(),
        decimals,
        initial_supply,
    });

    let signature = client
        .deploy_contract(&deployer, wasm_code, &contract_addr, init_data_bytes)
        .await?;

    println!("📝 Signature: {}", signature);

    if !report_token_deploy_readiness(
        wait_for_deploy_readiness(client, &signature, &contract_addr).await,
        &contract_addr,
    ) {
        return Ok(());
    }

    handle_token_post_deploy(
        client,
        &deployer,
        &contract_addr,
        TokenPostDeploy {
            name: &name,
            symbol: &symbol,
            decimals,
            initial_supply,
            registry_metadata,
        },
    )
    .await;

    Ok(())
}
