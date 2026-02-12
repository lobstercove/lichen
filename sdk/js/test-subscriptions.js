// Test SDK subscriptions with auto-exit
const { Connection, PublicKey } = require('./dist/index');

async function test() {
  console.log('🦞 Testing MoltChain SDK Subscriptions\n');
  
  const connection = new Connection(
    'http://localhost:8899',
    'ws://localhost:8900'
  );

  let slotCount = 0;
  let blockCount = 0;

  // Subscribe to slots
  console.log('📡 Subscribing to slots...');
  const slotSub = await connection.onSlot((slot) => {
    slotCount++;
    if (slotCount % 5 === 0) {
      console.log(`⏱️  Slot ${slot} (${slotCount} received)`);
    }
  });
  console.log(`✅ Subscribed to slots (ID: ${slotSub})\n`);

  // Subscribe to blocks
  console.log('📡 Subscribing to blocks...');
  const blockSub = await connection.onBlock((block) => {
    blockCount++;
    console.log(`📦 Block #${block.slot}: ${block.transactions || block.transaction_count || 0} TXs`);
  });
  console.log(`✅ Subscribed to blocks (ID: ${blockSub})\n`);

  // Subscribe to account changes (using genesis account)
  console.log('📡 Subscribing to account changes...');
  const pubkey = new PublicKey('6YkFWKH9HQZFVEy4QPw82xRx5qHRk84vU1H2Hk7JLj1H');
  const accountSub = await connection.onAccountChange(pubkey, (account) => {
    console.log(`👤 Account balance updated: ${account.molt || account.balance / 1e9} MOLT`);
  });
  console.log(`✅ Subscribed to account changes (ID: ${accountSub})\n`);

  console.log('✅ All subscriptions active!\n');
  console.log('Listening for 10 seconds...\n');

  // Auto-exit after 10 seconds
  setTimeout(() => {
    console.log(`\n📊 Test complete!`);
    console.log(`   Slots received: ${slotCount}`);
    console.log(`   Blocks received: ${blockCount}`);
    connection.close();
    process.exit(0);
  }, 10000);
}

test().catch((err) => {
  console.error('Error:', err.message);
  process.exit(1);
});
