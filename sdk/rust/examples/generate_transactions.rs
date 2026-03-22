//! Generate test transactions

use moltchain_client_sdk::{Client, Keypair, TransactionBuilder};
use moltchain_core::{Instruction, Hash, SYSTEM_PROGRAM_ID};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🦀 Rust SDK: Generating transactions...\n");
    
    let client = Client::new("http://localhost:8899");
    
    // Generate 5 test keypairs
    let keypairs: Vec<Keypair> = (0..5).map(|_| Keypair::new()).collect();
    
    println!("📝 Generated 5 test keypairs:");
    for (i, kp) in keypairs.iter().enumerate() {
        println!("   {}: {}", i + 1, kp.pubkey().to_base58());
    }
    println!();
    
    // Get recent blockhash
    let blockhash_str = client.get_recent_blockhash().await?;
    let blockhash = Hash::from_hex(&blockhash_str)?;
    println!("🔗 Blockhash: {}...\n", &blockhash_str[..16]);
    
    // Generate transactions
    println!("📤 Building transactions:");
    let mut transactions = Vec::new();
    
    for i in 0..5 {
        let sender = &keypairs[i];
        let recipient = &keypairs[(i + 1) % 5];
        
        // Create transfer instruction
        let mut data = vec![0u8]; // Transfer instruction
        data.extend_from_slice(&(100 * (i + 1) as u64).to_le_bytes());
        
        let instruction = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![sender.pubkey(), recipient.pubkey()],
            data,
        };
        
        let tx = TransactionBuilder::new()
            .add_instruction(instruction)
            .recent_blockhash(blockhash)
            .build_and_sign(sender)?;
        
        transactions.push(tx);
        println!("   ✅ Transaction {} built", i + 1);
    }
    
    println!("\n📊 Summary:");
    println!("   Transactions built: {}", transactions.len());
    println!("   Total signatures: {}", transactions.iter().map(|t| t.signatures.len()).sum::<usize>());
    println!("   Total instructions: {}", transactions.iter().map(|t| t.message.instructions.len()).sum::<usize>());
    
    println!("\n✅ Rust SDK transaction generation complete!");
    
    Ok(())
}
