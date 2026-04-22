use anyhow::Result;

use crate::client::RpcClient;
use crate::deploy_output_support::{print_deploy_success, report_deploy_readiness};
use crate::deploy_post_deploy_support::{
    handle_deploy_symbol_registration, DeploySymbolRegistration,
};
use crate::deploy_prepare_support::PreparedContractDeploy;
use crate::deploy_readiness_support::wait_for_deploy_readiness;

pub(super) async fn finalize_deploy(
    client: &RpcClient,
    prepared: PreparedContractDeploy,
) -> Result<()> {
    let PreparedContractDeploy {
        contract,
        deployer,
        wasm_code,
        contract_addr,
        init_data,
        symbol,
        name,
        template,
        decimals,
        supply,
        metadata,
    } = prepared;

    println!("🦞 Deploying contract: {}", contract.display());
    println!("📦 Size: {} KB", wasm_code.len() / 1024);
    println!("📍 Contract address: {}", contract_addr.to_base58());
    println!("👤 Deployer: {}", deployer.pubkey().to_base58());
    if let Some(ref value) = symbol {
        println!("🏷️  Symbol: {}", value);
    }
    if let Some(ref value) = template {
        println!("📂 Template: {}", value);
    }
    if let Some(value) = supply {
        println!(
            "💎 Total supply: {} (decimals: {})",
            value,
            decimals.unwrap_or(9)
        );
    }
    println!("💰 Deploy fee: 25.001 LICN (25 LICN deploy + 0.001 LICN base fee)");
    println!();

    let signature = client
        .deploy_contract(&deployer, wasm_code, &contract_addr, init_data)
        .await?;

    println!("📝 Signature: {}", signature);

    println!("⏳ Waiting for transaction confirmation...");
    if !report_deploy_readiness(
        wait_for_deploy_readiness(client, &signature, &contract_addr).await,
        &signature,
        &contract_addr,
    ) {
        return Ok(());
    }

    print_deploy_success(&contract_addr);
    handle_deploy_symbol_registration(
        client,
        &deployer,
        &contract_addr,
        symbol.as_deref().map(|symbol| DeploySymbolRegistration {
            symbol,
            name: name.as_deref(),
            template: template.as_ref(),
            decimals,
            metadata: metadata.as_ref(),
        }),
    )
    .await;

    Ok(())
}
