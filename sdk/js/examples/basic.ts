// Basic usage example for Lichen SDK

import { Connection, PublicKey } from '../dist/index';

async function main() {
  // Connect to Lichen
  const connection = new Connection('http://localhost:8899', 'ws://localhost:8900');

  console.log('🦞 Lichen SDK Example\n');

  // 1. Get network info
  console.log('📡 Network Information:');
  const networkInfo = await connection.getNetworkInfo();
  console.log(`  Chain ID: ${networkInfo.chainId}`);
  console.log(`  Version: ${networkInfo.version}`);
  console.log(`  Current Slot: ${networkInfo.currentSlot}`);
  console.log(`  Validators: ${networkInfo.validatorCount}\n`);

  // 2. Get account balance
  console.log('💰 Account Balance:');
  const pubkey = new PublicKey('YourPublicKeyHere...');
  try {
    const balance = await connection.getBalance(pubkey);
    console.log(`  Balance: ${balance.licn} LICN (${balance.shells} shells)\n`);
  } catch (error) {
    console.log(`  Error: ${error}\n`);
  }

  // 3. Get chain status
  console.log('⛓️  Chain Status:');
  const status = await connection.getChainStatus();
  console.log(`  TPS: ${status.tps}`);
  console.log(`  Total Blocks: ${status.totalBlocks}`);
  console.log(`  Total Transactions: ${status.totalTransactions}`);
  console.log(`  Healthy: ${status.isHealthy}\n`);

  // 4. Get all validators
  console.log('🔒 Validators:');
  const validators = await connection.getValidators();
  console.log(`  Total: ${validators.length}`);
  validators.forEach((v: any, i: number) => {
    console.log(`  ${i + 1}. ${v.pubkey.substring(0, 12)}... (Stake: ${v.stake / 1e9} LICN)`);
  });
  console.log();

  // 5. Subscribe to real-time events
  console.log('🔔 Subscribing to real-time events...\n');

  // Subscribe to blocks
  const blockSub = await connection.onBlock((block: any) => {
    console.log(`📦 New Block #${block.slot}: ${block.transactions} transactions`);
  });

  // Subscribe to slots
  const slotSub = await connection.onSlot((slot: number) => {
    console.log(`⏱️  Slot: ${slot}`);
  });

  // Wait for events for 30 seconds
  console.log('Listening for 30 seconds...\n');
  await new Promise(resolve => setTimeout(resolve, 30000));

  // Cleanup
  await connection.offBlock(blockSub);
  await connection.offSlot(slotSub);
  connection.close();

  console.log('\n✅ Example complete!');
}

main().catch(console.error);
