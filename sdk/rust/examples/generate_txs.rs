//! Generate test transactions

use moltchain_sdk::{Client, Keypair, TransactionBuilder};
use moltchain_core::{Instruction, Hash, SYSTEM_PROGRAM_ID};
use base64::Engine;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🦞 Generating transactions...\n");
    
    let keypair_a = Keypair::new();
    let keypair_b = Keypair::new();
    
    println!("📍 Keypair A: {}", keypair_a.pubkey().to_base58());
    println!("📍 Keypair B: {}", keypair_b.pubkey().to_base58());
    
    // Create client
    let client = Client::new("http://localhost:8899");
    
    let blockhash_str = client.get_recent_blockhash().await?;
    let blockhash = Hash::from_hex(&blockhash_str)?;
    println!("\n🔗 Blockhash: {}...", &blockhash_str[..16]);
    
    // Create a simple instruction (memo/note)
    let instruction = Instruction {
        program_id: SYSTEM_PROGRAM_ID,
        accounts: vec![],
        data: b"test transaction".to_vec(),
    };
    
    // Build transaction
    let tx = TransactionBuilder::new()
        .add_instruction(instruction)
        .recent_blockhash(blockhash)
        .build_and_sign(&keypair_a)?;
    
    println!("✅ Built transaction with {} signature(s)", tx.signatures.len());
    println!("   Instructions: {}", tx.message.instructions.len());
    
    let tx_bytes = bincode::serialize(&tx)?;
    let tx_base64 = base64::engine::general_purpose::STANDARD.encode(&tx_bytes);
    match client.send_raw_transaction(&tx_base64).await {
        Ok(sig) => println!("📤 Submitted: {}", sig),
        Err(e) => println!("❌ Failed: {}", e),
    }
    
    println!("\n✅ Transaction generation test complete!");
    
    Ok(())
}
