#!/usr/bin/env ts-node
/**
 * Generate test transactions
 */

import { Connection, Keypair, TransactionBuilder } from './src';

async function main() {
    console.log('📘 TypeScript SDK: Generating transactions...\n');
    
    const connection = new Connection('http://localhost:8899');
    
    // Generate 5 test keypairs
    const keypairs: Keypair[] = [];
    for (let i = 0; i < 5; i++) {
        keypairs.push(Keypair.generate());
    }
    
    console.log('📝 Generated 5 test keypairs');
    console.log();
    
    // Get recent blockhash
    const blockhash = await connection.getRecentBlockhash();
    console.log(`🔗 Blockhash: ${blockhash.substring(0, 16)}...\n`);
    
    // Build and send transactions
    console.log('📤 Building transactions:');
    for (let i = 0; i < 5; i++) {
        const sender = keypairs[i];
        const recipient = keypairs[(i + 1) % 5];
        const instruction = TransactionBuilder.transfer(
            sender.pubkey(),
            recipient.pubkey(),
            (i + 1) * 100_000_000,
        );
        const tx = new TransactionBuilder()
            .add(instruction)
            .setRecentBlockhash(blockhash)
            .buildAndSign(sender);
        const sig = await connection.sendTransaction(tx);
        console.log(`   ✅ Transaction ${i + 1} sent: ${sig.substring(0, 16)}...`);
    }
    
    console.log('\n📊 Summary:');
    console.log('   Transactions built: 5');
    console.log('   Total instructions: 5');
    
    console.log('\n✅ TypeScript SDK transaction generation complete!');
}

main().catch(console.error);
