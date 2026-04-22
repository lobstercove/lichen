use lichen_core::{Keypair, Pubkey};

use crate::client::RpcClient;
use crate::contract_args_support::{encode_dual_address_amount_args, encode_single_address_arg};
use crate::contract_poll_support::wait_for_confirmation;

pub(super) async fn initialize_token_contract(
    client: &RpcClient,
    deployer: &Keypair,
    contract_addr: &Pubkey,
) {
    println!("⏳ Initializing token admin/minter state...");
    match client
        .call_contract(
            deployer,
            contract_addr,
            "initialize".to_string(),
            encode_single_address_arg(&deployer.pubkey()),
            0,
        )
        .await
    {
        Ok(init_sig) => match wait_for_confirmation(client, &init_sig, 10).await {
            Ok(true) => println!("✅ Token contract initialized (sig: {})", init_sig),
            Ok(false) => {
                println!(
                    "⚠️  Initialize transaction is still pending (sig: {}).",
                    init_sig
                );
            }
            Err(error) => {
                println!(
                    "⚠️  Token deployed, but initialize failed on-chain: {}",
                    error
                );
                println!(
                    "   Use `lichen deploy` for token contracts that require a custom init flow."
                );
            }
        },
        Err(error) => {
            println!(
                "⚠️  Token deployed, but initialize could not be submitted: {}",
                error
            );
            println!("   Use `lichen deploy` for token contracts that require a custom init flow.");
        }
    }
}

pub(super) async fn mint_initial_supply(
    client: &RpcClient,
    deployer: &Keypair,
    contract_addr: &Pubkey,
    decimals: u8,
    supply: u64,
) {
    let base_units = supply.saturating_mul(10u64.saturating_pow(decimals as u32));
    println!("⏳ Minting initial supply to creator...");
    match client
        .call_contract(
            deployer,
            contract_addr,
            "mint".to_string(),
            encode_dual_address_amount_args(&deployer.pubkey(), &deployer.pubkey(), base_units),
            0,
        )
        .await
    {
        Ok(mint_sig) => match wait_for_confirmation(client, &mint_sig, 10).await {
            Ok(true) => {
                println!(
                    "✅ Initial supply minted to {} (sig: {})",
                    deployer.pubkey().to_base58(),
                    mint_sig
                );
            }
            Ok(false) => {
                println!(
                    "⚠️  Initial mint transaction is still pending (sig: {}).",
                    mint_sig
                );
            }
            Err(error) => {
                println!(
                    "⚠️  Token deployed and initialized, but initial mint failed on-chain: {}",
                    error
                );
            }
        },
        Err(error) => {
            println!(
                "⚠️  Token deployed and initialized, but initial mint could not be submitted: {}",
                error
            );
        }
    }
}
