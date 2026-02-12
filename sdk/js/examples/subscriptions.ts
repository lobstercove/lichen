// Subscription example for MoltChain SDK

import { Connection, PublicKey } from '../dist/index';

async function main() {
  const connection = new Connection(
    'http://localhost:8899',
    'ws://localhost:8900'
  );

  console.log('🦞 MoltChain Subscription Example\n');
  console.log('Press Ctrl+C to exit\n');

  // Subscribe to all events
  const subscriptions: number[] = [];

  // 1. Subscribe to slots
  console.log('📡 Subscribing to slots...');
  const slotSub = await connection.onSlot((slot) => {
    console.log(`⏱️  Slot ${slot}`);
  });
  subscriptions.push(slotSub);

  // 2. Subscribe to blocks
  console.log('📡 Subscribing to blocks...');
  const blockSub = await connection.onBlock((block) => {
    console.log(`📦 Block #${block.slot}: ${block.transactions} TXs, Hash: ${block.hash.substring(0, 16)}...`);
  });
  subscriptions.push(blockSub);

  // 3. Subscribe to transactions
  console.log('📡 Subscribing to transactions...');
  const txSub = await connection.onTransaction((tx) => {
    console.log(`💸 Transaction: ${tx.signatures[0]?.substring(0, 16)}...`);
  });
  subscriptions.push(txSub);

  // 4. Subscribe to account changes
  console.log('📡 Subscribing to account changes...');
  const pubkey = new PublicKey('YourPublicKeyHere...');
  const accountSub = await connection.onAccountChange(pubkey, (account) => {
    console.log(`👤 Account ${account.pubkey.substring(0, 12)}... balance: ${account.molt} MOLT`);
  });
  subscriptions.push(accountSub);

  // 5. Subscribe to all contract logs
  console.log('📡 Subscribing to contract logs...');
  const logsSub = await connection.onLogs((log) => {
    console.log(`📝 Log from ${log.contract.substring(0, 12)}...: ${log.message}`);
  });
  subscriptions.push(logsSub);

  console.log('\n✅ All subscriptions active!\n');
  console.log('Listening for events...\n');

  // Handle graceful shutdown
  process.on('SIGINT', async () => {
    console.log('\n\n🛑 Shutting down...');
    
    // Unsubscribe from all
    for (const sub of subscriptions) {
      await connection.offSlot(sub);
    }
    
    connection.close();
    console.log('✅ Disconnected');
    process.exit(0);
  });

  // Keep running
  await new Promise(() => {});
}

main().catch(console.error);
