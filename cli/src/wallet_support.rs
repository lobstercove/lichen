use anyhow::Result;
use lichen_core::Pubkey;

use crate::cli_args::WalletCommands;
use crate::client::RpcClient;
use crate::output_support::to_licn;
use crate::wallet::WalletManager;

pub(super) async fn handle_wallet_command(
    client: &RpcClient,
    wallet_cmd: WalletCommands,
) -> Result<()> {
    let wallet_mgr = WalletManager::new()?;

    match wallet_cmd {
        WalletCommands::Create { name } => {
            wallet_mgr.create_wallet(name)?;
        }

        WalletCommands::Import { name, keypair } => {
            wallet_mgr.import_wallet(name, keypair)?;
        }

        WalletCommands::List => {
            wallet_mgr.list_wallets()?;
        }

        WalletCommands::Show { name } => {
            wallet_mgr.show_wallet(&name)?;
        }

        WalletCommands::Remove { name } => {
            wallet_mgr.remove_wallet(&name)?;
        }

        WalletCommands::Balance { name } => {
            let wallet = wallet_mgr.get_wallet(&name)?;
            let pubkey = Pubkey::from_base58(&wallet.address)
                .map_err(|error| anyhow::anyhow!("Invalid address: {}", error))?;
            let balance = client.get_balance(&pubkey).await?;

            println!("\n🦞 Wallet: {}", wallet.name);
            println!("📍 Address: {}", wallet.address);
            println!("─────────────────────────────────────────────────────────");
            println!("💰 Total:     {:>12.4} LICN", to_licn(balance.spores));
            println!("   Spendable: {:>12.4} LICN", to_licn(balance.spendable));
            println!("   Staked:    {:>12.4} LICN", to_licn(balance.staked));
            println!("   Locked:    {:>12.4} LICN", to_licn(balance.locked));
            println!("─────────────────────────────────────────────────────────\n");
        }
    }

    Ok(())
}
