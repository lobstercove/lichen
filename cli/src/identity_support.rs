use anyhow::{Context, Result};
use lichen_core::Keypair;

use crate::cli_args::IdentityCommands;
use crate::keypair_manager::KeypairManager;
use crate::output_support::print_json;

pub(super) async fn handle_identity_command(
    keypair_mgr: &KeypairManager,
    id_cmd: IdentityCommands,
    json_output: bool,
) -> Result<()> {
    match id_cmd {
        IdentityCommands::New { output } => {
            let keypair = Keypair::new();
            let pubkey = keypair.pubkey();

            let path = output.unwrap_or_else(|| keypair_mgr.default_keypair_path());
            keypair_mgr.save_keypair(&keypair, &path)?;

            println!("🦞 Generated new identity!");
            println!("📍 Pubkey: {}", pubkey.to_base58());
            println!("🔐 EVM Address: {}", pubkey.to_evm());
            println!("💾 Saved to: {}", path.display());
            println!();
            println!("💡 Get test tokens: lichen airdrop 100");
        }

        IdentityCommands::Show { keypair } => {
            let path = keypair.unwrap_or_else(|| keypair_mgr.default_keypair_path());
            let kp = keypair_mgr.load_keypair(&path)?;
            let pubkey = kp.pubkey();

            println!("🦞 Your Lichen Identity");
            println!("📍 Pubkey: {}", pubkey.to_base58());
            println!("🔐 EVM Address: {}", pubkey.to_evm());
            println!("📄 Keypair: {}", path.display());
        }

        IdentityCommands::Export {
            keypair,
            reveal_seed,
        } => {
            let path = keypair.unwrap_or_else(|| keypair_mgr.default_keypair_path());

            let kp = keypair_mgr.load_keypair(&path).with_context(|| {
                format!(
                    "Failed to decrypt {}. Is LICHEN_KEYPAIR_PASSWORD set correctly?",
                    path.display()
                )
            })?;
            let pubkey = kp.pubkey();

            if json_output {
                let mut obj = serde_json::json!({
                    "pubkey": pubkey.to_base58(),
                    "evm_address": pubkey.to_evm(),
                    "file": path.display().to_string(),
                    "encrypted": std::env::var("LICHEN_KEYPAIR_PASSWORD")
                        .map(|value| !value.is_empty())
                        .unwrap_or(false),
                });
                if reveal_seed {
                    obj["seed_hex"] = serde_json::Value::String(hex::encode(kp.to_seed()));
                }
                print_json(&obj);
            } else {
                println!("🦞 Keypair Export");
                println!("📄 File:    {}", path.display());
                println!("📍 Pubkey:  {}", pubkey.to_base58());
                println!("🔐 EVM:     {}", pubkey.to_evm());
                if reveal_seed {
                    println!("🔑 Seed:    {}", hex::encode(kp.to_seed()));
                    println!();
                    println!("⚠️  SECURITY: The seed above is your private key material.");
                    println!("   Anyone with this seed controls your account. Do not share.");
                }
            }
        }
    }

    Ok(())
}
