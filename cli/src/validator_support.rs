use anyhow::{bail, Result};
use sha2::{Digest, Sha256};
use std::path::PathBuf;

use crate::client::RpcClient;
use crate::keypair_manager::KeypairManager;
use crate::stake_signer_support::load_staker_keypair;

pub(super) async fn handle_validator_info(client: &RpcClient, address: &str) -> Result<()> {
    println!("🦞 Validator Information");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!();

    match client.get_validator_info(address).await {
        Ok(info) => {
            println!("📍 Pubkey: {}", info.pubkey);
            println!("💰 Stake: {} LICN", info.stake as f64 / 1_000_000_000.0);
            println!("⭐ Reputation: {}", info.reputation);
            println!(
                "📊 Status: {}",
                if info.is_active { "Active" } else { "Inactive" }
            );
            println!("📦 Blocks proposed: {}", info.blocks_proposed);
            println!("📝 Transactions processed: {}", info.transactions_processed);
            println!(
                "🗳️  Votes: {}/{} correct",
                info.correct_votes, info.votes_cast
            );
            println!("⏱️  Last active slot: {}", info.last_active_slot);
        }
        Err(error) => {
            println!("⚠️  Validator not found: {}", error);
        }
    }

    Ok(())
}

pub(super) async fn handle_validator_performance(client: &RpcClient, address: &str) -> Result<()> {
    println!("🦞 Validator Performance");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!();

    match client.get_validator_performance(address).await {
        Ok(performance) => {
            println!("📍 Validator: {}", performance.pubkey);
            println!();
            println!("📊 Performance:");
            println!("   Blocks proposed: {}", performance.blocks_proposed);
            println!(
                "   Transactions processed: {}",
                performance.transactions_processed
            );
            println!("   Votes cast: {}", performance.votes_cast);
            println!("   Correct votes: {}", performance.correct_votes);
            println!("   Vote accuracy: {:.2}%", performance.vote_accuracy);
            println!("   Reputation: {:.4}", performance.reputation);
            println!();
            println!("⏰ Uptime: {:.2}%", performance.uptime);
        }
        Err(error) => {
            println!("⚠️  Could not fetch performance: {}", error);
        }
    }

    Ok(())
}

pub(super) async fn handle_validator_list(client: &RpcClient) -> Result<()> {
    let validators_info = client.get_validators().await?;
    let validators = &validators_info.validators;

    println!("🦞 Active Validators");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!();

    if validators.is_empty() {
        println!("No validators found");
    } else {
        for (index, validator) in validators.iter().enumerate() {
            println!("#{} {}", index + 1, validator.pubkey);
            println!(
                "   Stake: {} LICN",
                validator.stake as f64 / 1_000_000_000.0
            );
            println!("   Reputation: {}", validator.reputation);
            println!();
        }

        let total_stake: u64 = validators.iter().map(|validator| validator.stake).sum();
        println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        println!(
            "Total: {} validators, {} LICN staked",
            validators.len(),
            total_stake as f64 / 1_000_000_000.0
        );
    }

    Ok(())
}

pub(super) fn handle_validator_fingerprint() -> Result<()> {
    let fingerprint = collect_local_machine_fingerprint()?;
    println!("{}", hex::encode(fingerprint));
    Ok(())
}

pub(super) async fn handle_validator_register(
    client: &RpcClient,
    keypair_mgr: &KeypairManager,
    keypair: Option<PathBuf>,
    fingerprint_hex: Option<String>,
) -> Result<()> {
    let kp = load_staker_keypair(keypair_mgr, keypair)?;
    let fingerprint = match fingerprint_hex {
        Some(value) => parse_machine_fingerprint_hex(&value)?,
        None => collect_local_machine_fingerprint()?,
    };

    println!("Registering validator");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("Validator: {}", kp.pubkey().to_base58());
    println!("Mode: bootstrap grant");
    println!("Fingerprint: {}", hex::encode(fingerprint));
    println!();

    match client
        .register_validator_bootstrap_grant(&kp, fingerprint)
        .await
    {
        Ok(signature) => {
            println!("RegisterValidator transaction sent");
            println!("Signature: {}", signature);
        }
        Err(error) => {
            println!("RegisterValidator failed: {}", error);
        }
    }

    Ok(())
}

pub(super) async fn handle_validator_register_self_funded(
    client: &RpcClient,
    keypair_mgr: &KeypairManager,
    amount: u64,
    keypair: Option<PathBuf>,
    fingerprint_hex: Option<String>,
) -> Result<()> {
    let kp = load_staker_keypair(keypair_mgr, keypair)?;
    let fingerprint = match fingerprint_hex {
        Some(value) => parse_machine_fingerprint_hex(&value)?,
        None => collect_local_machine_fingerprint()?,
    };

    if amount == 0 {
        bail!("validator registration amount must be nonzero");
    }

    println!("Registering self-funded validator");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("Validator: {}", kp.pubkey().to_base58());
    println!("Amount: {} LICN", amount as f64 / 1_000_000_000.0);
    println!("Fingerprint: {}", hex::encode(fingerprint));
    println!();

    match client
        .register_validator_self_funded(&kp, fingerprint, amount)
        .await
    {
        Ok(signature) => {
            println!("RegisterValidator transaction sent");
            println!("Signature: {}", signature);
        }
        Err(error) => {
            println!("RegisterValidator failed: {}", error);
        }
    }

    Ok(())
}

pub(super) async fn handle_validator_reclassify_bootstrap(
    client: &RpcClient,
    keypair_mgr: &KeypairManager,
    keypair: Option<PathBuf>,
) -> Result<()> {
    let kp = load_staker_keypair(keypair_mgr, keypair)?;

    println!("Reclassifying validator bootstrap recovery");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("Validator: {}", kp.pubkey().to_base58());
    println!();

    match client.reclassify_validator_bootstrap(&kp).await {
        Ok(signature) => {
            println!("ReclassifyValidatorBootstrap transaction sent");
            println!("Signature: {}", signature);
        }
        Err(error) => {
            println!("ReclassifyValidatorBootstrap failed: {}", error);
        }
    }

    Ok(())
}

fn parse_machine_fingerprint_hex(value: &str) -> Result<[u8; 32]> {
    let trimmed = value.trim().strip_prefix("0x").unwrap_or(value.trim());
    let bytes = hex::decode(trimmed)?;
    if bytes.len() != 32 {
        bail!(
            "machine fingerprint must be exactly 32 bytes / 64 hex chars, got {} bytes",
            bytes.len()
        );
    }
    let mut fingerprint = [0u8; 32];
    fingerprint.copy_from_slice(&bytes);
    if fingerprint == [0u8; 32] {
        bail!("machine fingerprint must not be all zeroes");
    }
    Ok(fingerprint)
}

fn collect_local_machine_fingerprint() -> Result<[u8; 32]> {
    let mut hasher = Sha256::new();
    let mut got_uuid = false;
    let mut got_mac = false;

    #[cfg(target_os = "macos")]
    {
        if let Ok(output) = std::process::Command::new("ioreg")
            .args(["-rd1", "-c", "IOPlatformExpertDevice"])
            .output()
        {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                if line.contains("IOPlatformUUID") {
                    if let Some(uuid) = line.split('"').nth(3) {
                        hasher.update(uuid.as_bytes());
                        got_uuid = true;
                        break;
                    }
                }
            }
        }
    }

    #[cfg(target_os = "linux")]
    {
        if let Ok(uuid) = std::fs::read_to_string("/sys/class/dmi/id/product_uuid") {
            hasher.update(uuid.trim().as_bytes());
            got_uuid = true;
        } else if let Ok(machine_id) = std::fs::read_to_string("/etc/machine-id") {
            hasher.update(machine_id.trim().as_bytes());
            got_uuid = true;
        }
    }

    #[cfg(target_os = "macos")]
    {
        if let Ok(output) = std::process::Command::new("ifconfig").arg("en0").output() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                let trimmed = line.trim();
                if trimmed.starts_with("ether ") {
                    let mac = trimmed.trim_start_matches("ether ").trim();
                    hasher.update(mac.as_bytes());
                    got_mac = true;
                    break;
                }
            }
        }
    }

    #[cfg(target_os = "linux")]
    {
        if let Ok(entries) = std::fs::read_dir("/sys/class/net") {
            let mut macs = Vec::new();
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name == "lo" || name.starts_with("veth") || name.starts_with("docker") {
                    continue;
                }
                let addr_path = entry.path().join("address");
                if let Ok(mac) = std::fs::read_to_string(&addr_path) {
                    let mac = mac.trim().to_string();
                    if mac != "00:00:00:00:00:00" {
                        macs.push(mac);
                    }
                }
            }
            macs.sort();
            if let Some(mac) = macs.first() {
                hasher.update(mac.as_bytes());
                got_mac = true;
            }
        }
    }

    if !got_uuid && !got_mac {
        bail!(
            "could not collect platform UUID or MAC address; pass --fingerprint-hex explicitly"
        );
    }

    let result = hasher.finalize();
    let mut fingerprint = [0u8; 32];
    fingerprint.copy_from_slice(&result);
    if fingerprint == [0u8; 32] {
        bail!("machine fingerprint resolved to all zeroes");
    }
    Ok(fingerprint)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_machine_fingerprint_hex() {
        let parsed = parse_machine_fingerprint_hex(
            "0x0102030405060708090001020304050607080900010203040506070809000102",
        )
        .unwrap();

        assert_eq!(parsed[0], 1);
        assert_eq!(parsed[31], 2);
    }

    #[test]
    fn rejects_bad_machine_fingerprint_hex_length() {
        let err = parse_machine_fingerprint_hex("abcd").unwrap_err();
        assert!(err.to_string().contains("exactly 32 bytes"));
    }

    #[test]
    fn rejects_zero_machine_fingerprint() {
        let err = parse_machine_fingerprint_hex(
            "0000000000000000000000000000000000000000000000000000000000000000",
        )
        .unwrap_err();
        assert!(err.to_string().contains("must not be all zeroes"));
    }
}
