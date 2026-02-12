// Transaction example for MoltChain SDK

import { Connection, PublicKey, TransactionBuilder } from '../dist/index';

async function main() {
  const connection = new Connection('http://localhost:8899');

  console.log('🦞 MoltChain Transaction Example\n');

  // Create a transfer transaction
  const from = new PublicKey('FromPublicKeyHere...');
  const to = new PublicKey('ToPublicKeyHere...');
  const amount = 1_000_000_000; // 1 MOLT

  console.log('Building transaction...');
  
  // Get recent blockhash
  const latestBlock = await connection.getLatestBlock();
  
  // Build transaction
  const transaction = new TransactionBuilder()
    .add(TransactionBuilder.transfer(from, to, amount))
    .setRecentBlockhash(latestBlock.hash)
    .build();

  console.log(`Transfer: ${amount / 1e9} MOLT`);
  console.log(`From: ${from.toBase58()}`);
  console.log(`To: ${to.toBase58()}`);
  console.log(`Blockhash: ${latestBlock.hash.substring(0, 16)}...`);

  // Note: In a real application, you would sign the transaction here
  // For example: transaction.sign(keypair);

  console.log('\n⚠️  Transaction built but not signed or sent');
  console.log('In a real app, you would sign with a keypair and send with connection.sendTransaction()');
}

main().catch(console.error);
