// Transaction creation and signing

use anyhow::{Context, Result, bail};
use lichen_core::{Transaction, Message, Instruction, Pubkey, Hash, SYSTEM_PROGRAM_ID};
use std::path::PathBuf;
use std::io::{self, Write};

use crate::config::CliConfig;
use crate::keygen;

/// Convert LICN (f64) to spores (u64) with precise integer arithmetic.
fn licn_to_spores(lichen: f64) -> u64 {
    if lichen <= 0.0 {
        return 0;
    }
    if lichen >= (u64::MAX / 1_000_000_000) as f64 {
        return u64::MAX;
    }
    let whole = lichen.trunc() as u64;
    let frac = ((lichen.fract() * 1_000_000_000.0).round()) as u64;
    whole.saturating_mul(1_000_000_000).saturating_add(frac)
}

/// Transfer tokens between accounts
pub async fn transfer(
    config: &CliConfig,
    from_path: PathBuf,
    to_address: String,
    amount: f64,
    skip_confirm: bool,
) -> Result<()> {
    // Load sender keypair
    let sender_keypair = keygen::load_keypair(Some(&from_path))?;
    let sender_pubkey = sender_keypair.pubkey();
    
    // Parse recipient
    let recipient = Pubkey::from_base58(&to_address)
        .context("Invalid recipient address")?;
    
    // Convert LICN to spores using precise integer arithmetic
    let spores = licn_to_spores(amount);
    
    // Display transaction details
    println!("📤 Transfer Transaction");
    println!("\n   From:   {}", sender_pubkey.to_base58());
    println!("   To:     {}", to_address);
    println!("   Amount: {} LICN ({} spores)", amount, spores);
    
    // Get confirmation
    if !skip_confirm {
        print!("\nConfirm transaction? (y/N): ");
        io::stdout().flush()?;
        
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        
        if !input.trim().eq_ignore_ascii_case("y") {
            println!("❌ Transaction cancelled");
            return Ok(());
        }
    }
    
    println!("\n🔨 Building transaction...");
    
    // Get recent blockhash from chain
    let client = reqwest::Client::new();
    let rpc_request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getLatestBlock",
        "params": []
    });
    
    let response = client
        .post(&config.rpc_url)
        .json(&rpc_request)
        .send()
        .await?
        .json::<serde_json::Value>()
        .await?;
    
    let blockhash_str = response["result"]["hash"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Failed to get recent blockhash"))?;
    
    let blockhash = Hash::from_hex(blockhash_str)?;
    
    println!("   Recent blockhash: {}...", &blockhash_str[..16]);
    
    // Create transfer instruction
    let mut transfer_data = vec![0u8];
    transfer_data.extend_from_slice(&spores.to_le_bytes());
    
    let instruction = Instruction {
        program_id: SYSTEM_PROGRAM_ID,
        accounts: vec![sender_pubkey, recipient],
        data: transfer_data,
    };
    
    // Create message
    let message = Message::new(vec![instruction], blockhash);
    
    // Sign transaction
    println!("🔏 Signing transaction...");
    let message_bytes = message.serialize();
    let signature = sender_keypair.sign(&message_bytes);
    
    // Create signed transaction
    let transaction = Transaction {
        signatures: vec![signature],
        message,
            tx_type: Default::default(),
};
    
    // Serialize with wire envelope (M-6)
    let tx_bytes = transaction.to_wire();
    let tx_base64 = base64::engine::general_purpose::STANDARD.encode(&tx_bytes);
    
    println!("   Transaction size: {} bytes", tx_bytes.len());
    println!("   Signature: {}...", hex::encode(&signature[..16]));
    
    // Send transaction
    println!("\n📡 Sending transaction...");
    
    let send_request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "sendTransaction",
        "params": [tx_base64]
    });
    
    let send_response = client
        .post(&config.rpc_url)
        .json(&send_request)
        .send()
        .await?;
    
    let status = send_response.status();
    let response_json = send_response.json::<serde_json::Value>().await?;
    
    if !status.is_success() || response_json.get("error").is_some() {
        if let Some(error) = response_json.get("error") {
            bail!("Transaction failed: {}", error);
        } else {
            bail!("Transaction failed with status: {}", status);
        }
    }
    
    // Success!
    if let Some(tx_hash) = response_json["result"].as_str() {
        println!("\n✅ Transaction sent successfully!");
        println!("   Transaction hash: {}", tx_hash);
        println!("\n💡 View in explorer: http://localhost:3000/transaction.html?sig={}", tx_hash);
    } else {
        println!("\n✅ Transaction submitted to mempool");
    }
    
    Ok(())
}
