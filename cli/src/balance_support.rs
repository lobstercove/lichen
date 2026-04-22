use anyhow::Result;
use lichen_core::Pubkey;
use std::path::PathBuf;

use crate::client::RpcClient;
use crate::keypair_manager::KeypairManager;
use crate::output_support::to_licn;

pub(super) async fn handle_balance(
    client: &RpcClient,
    keypair_mgr: &KeypairManager,
    address: Option<String>,
    keypair: Option<PathBuf>,
) -> Result<()> {
    let pubkey = if let Some(addr) = address {
        if addr.starts_with("0x") {
            anyhow::bail!(
                "EVM addresses not yet supported for balance queries. Use Base58 format."
            );
        } else {
            Pubkey::from_base58(&addr)
                .map_err(|error| anyhow::anyhow!("Invalid Base58 address: {}", error))?
        }
    } else {
        let path = keypair.unwrap_or_else(|| keypair_mgr.default_keypair_path());
        let kp = keypair_mgr.load_keypair(&path)?;
        kp.pubkey()
    };

    let balance = client.get_balance(&pubkey).await?;

    println!("\n🦞 Balance for {}", pubkey.to_base58());
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!(
        "💰 Total:     {:>12.4} LICN ({} spores)",
        to_licn(balance.spores),
        balance.spores
    );
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!(
        "   Spendable: {:>12.4} LICN (available for transfers)",
        to_licn(balance.spendable)
    );
    println!(
        "   Staked:    {:>12.4} LICN (locked in validation)",
        to_licn(balance.staked)
    );
    println!(
        "   Locked:    {:>12.4} LICN (locked in contracts)",
        to_licn(balance.locked)
    );
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    Ok(())
}
