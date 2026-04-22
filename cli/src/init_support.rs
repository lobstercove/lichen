use anyhow::Result;
use lichen_core::{Keypair, KeypairFile};
use std::path::PathBuf;

pub(super) fn handle_init_command(output: Option<PathBuf>) -> Result<()> {
    eprintln!("⚠️  'lichen init' is deprecated. Use 'lichen identity new' instead.");

    let keypair = Keypair::new();
    let pubkey = keypair.pubkey();

    let path = match output {
        Some(path) => path,
        None => {
            eprintln!("Error: --output is required for init command");
            std::process::exit(1);
        }
    };

    let password = lichen_core::require_runtime_keypair_password("validator keypair generation")
        .map_err(anyhow::Error::msg)?;
    KeypairFile::from_keypair(&keypair)
        .save_with_password(&path, password.as_deref(), password.is_some())
        .map_err(anyhow::Error::msg)?;

    println!("🦞 Validator keypair initialized!");
    println!("📍 Pubkey: {}", pubkey.to_base58());
    println!("💾 Saved to: {}", path.display());

    Ok(())
}
