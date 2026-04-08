// Keypair generation and management

use anyhow::{bail, Result};
use lichen_core::{Keypair, KeypairFile};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Get default keypair path (~/.lichen/id.json)
#[allow(dead_code)]
pub fn default_keypair_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".lichen")
        .join("id.json")
}

/// Execute keygen command
#[allow(dead_code)]
pub fn execute(outfile: Option<PathBuf>, force: bool, show_formats: bool) -> Result<()> {
    let output_path = outfile.unwrap_or_else(default_keypair_path);

    // Check if file already exists
    if output_path.exists() && !force {
        println!(
            "⚠️  Keypair file already exists at: {}",
            output_path.display()
        );
        print!("Overwrite? (y/N): ");
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;

        if !input.trim().eq_ignore_ascii_case("y") {
            println!("❌ Aborted");
            return Ok(());
        }
    }

    // Create parent directory if it doesn't exist
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)?;
    }

    // Generate new keypair
    println!("🔑 Generating new PQ signing keypair...");
    let keypair = Keypair::new();

    // Create keypair file
    let keypair_file = KeypairFile::from_keypair(&keypair);

    // Save to file
    keypair_file
        .save(&output_path)
        .map_err(anyhow::Error::msg)?;

    // Display results
    println!("\n✅ Keypair generated successfully!");
    println!("\n📍 Public Key (Base58):");
    println!("   {}", keypair_file.public_key_base58);
    println!("\n💾 Saved to: {}", output_path.display());
    println!("   Permissions: 600 (owner read/write only)");

    if show_formats {
        println!("\n🔍 Key Formats:");
        println!("   Address Hex:    {}", hex::encode(keypair.pubkey().0));
        println!("   Address Base58: {}", keypair_file.public_key_base58);
        println!("   PQ Public Key:  {} bytes", keypair_file.public_key.len());

        // Show compatibility info
        println!("\n🔗 Compatibility:");
        println!("   ✓ Lichen native PQ format");
        println!("   ✓ ML-DSA-65 signing key");
        println!("   ✗ Not compatible with legacy pre-PQ wallet imports");
    }

    println!("\n⚠️  Keep your keypair file secure!");
    println!("   Never share your private key");
    println!("   Backup this file in a safe location");

    Ok(())
}

/// Show public key from keypair file
#[allow(dead_code)]
pub fn show_pubkey(keypair_path: PathBuf, formats: bool) -> Result<()> {
    let keypair_file = KeypairFile::load(&keypair_path).map_err(anyhow::Error::msg)?;
    let keypair = keypair_file.to_keypair().map_err(anyhow::Error::msg)?;

    println!("📍 Public Key: {}", keypair_file.public_key_base58);

    if formats {
        println!("\n🔍 Formats:");
        println!("   Address Base58: {}", keypair_file.public_key_base58);
        println!("   Address Hex:    {}", hex::encode(keypair.pubkey().0));
        println!("   PQ Public Key:  {} bytes", keypair_file.public_key.len());
    }

    Ok(())
}

/// Load keypair from file path or use default
#[allow(dead_code)]
pub fn load_keypair(path: Option<&Path>) -> Result<Keypair> {
    let keypair_path = path
        .map(|p| p.to_path_buf())
        .unwrap_or_else(default_keypair_path);

    if !keypair_path.exists() {
        bail!(
            "Keypair file not found at: {}\nRun 'lichen keygen' to create one",
            keypair_path.display()
        );
    }

    let keypair_file = KeypairFile::load(&keypair_path).map_err(anyhow::Error::msg)?;
    keypair_file.to_keypair().map_err(anyhow::Error::msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_keypair_path_is_under_hidden_lichen_dir() {
        let path = default_keypair_path();
        assert!(path.ends_with(Path::new(".lichen/id.json")));
    }
}
