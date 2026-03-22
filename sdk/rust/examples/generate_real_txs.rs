//! Generate real test transactions for explorer

use moltchain_client_sdk::{Client, Keypair};
use moltchain_core::{Instruction, Hash, SYSTEM_PROGRAM_ID};
use base64::Engine;
use std::error::Error;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    println!("🦞 Generating Real Test Transactions\n");
    println!("=====================================\n");
    
    let client = Client::new("http://localhost:8899");
    
    // Check connection
    println!("🔍 Checking validator connection...");
    let slot = client.get_slot().await?;
    println!("✅ Connected! Current slot: {}\n", slot);
    
    // Generate 3 test keypairs
    println!("🔑 Generating test keypairs...");
    let keypairs: Vec<Keypair> = (0..3).map(|_| Keypair::new()).collect();
    
    for (i, kp) in keypairs.iter().enumerate() {
        println!("   Wallet {}: {}", i + 1, kp.pubkey().to_base58());
    }
    println!();
    
    // Get recent blockhash
    println!("🔗 Getting recent blockhash...");
    let blockhash_str = client.get_recent_blockhash().await?;
    let blockhash = Hash::from_hex(&blockhash_str)?;
    println!("   Blockhash: {}...\n", &blockhash_str[..16]);
    
    println!("📝 Creating transactions...\n");
    
    // Create circular transactions (each sends to next)
    for i in 0..3 {
        let sender = &keypairs[i];
        let recipient = &keypairs[(i + 1) % 3];
        let amount = (i + 1) as u64 * 100_000_000; // 0.1, 0.2, 0.3 MOLT
        
        println!("Transaction {}:", i + 1);
        println!("  From: {}...", &sender.pubkey().to_base58()[..16]);
        println!("  To:   {}...", &recipient.pubkey().to_base58()[..16]);
        println!("  Amount: {} shells ({} MOLT)", amount, amount as f64 / 1_000_000_000.0);
        
        // Build transfer instruction
        let mut data = vec![0u8]; // Transfer instruction type
        data.extend_from_slice(&amount.to_le_bytes());
        
        let instruction = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![sender.pubkey(), recipient.pubkey()],
            data,
        };
        
        // Build transaction
        use moltchain_core::{Message, Transaction};
        let message = Message::new(vec![instruction], blockhash);
        
        // Sign transaction
        let message_bytes = message.serialize();
        let signature = sender.sign(&message_bytes);
        
        let tx = Transaction {
            signatures: vec![signature],
            message,
            tx_type: Default::default(),
        };
        
        // Serialize with bincode
        let tx_bytes = bincode::serialize(&tx)?;
        let tx_base64 = base64::engine::general_purpose::STANDARD.encode(&tx_bytes);
        
        // Send transaction
        match client.send_raw_transaction(&tx_base64).await {
            Ok(sig) => {
                println!("  ✅ Submitted! Signature: {}...", &sig[..16]);
            }
            Err(e) => {
                println!("  ⚠️  Failed: {} (expected - accounts have no balance)", e);
            }
        }
        
        println!();
    }
    
    println!("=====================================");
    println!("📊 Summary:");
    println!("   Transactions attempted: 3");
    println!("   Check explorer at: http://localhost:8080");
    println!("\n✅ Transaction generation complete!\n");
    
    Ok(())
}
