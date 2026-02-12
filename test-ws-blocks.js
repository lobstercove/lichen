// Test WebSocket Block Subscriptions
const WebSocket = require('ws');

const ws = new WebSocket('ws://localhost:8900');

ws.on('open', () => {
  console.log('\n🦞 Connected to MoltChain WebSocket\n');
  
  // Subscribe to both slots and blocks
  ws.send(JSON.stringify({
    jsonrpc: '2.0',
    id: 1,
    method: 'subscribeSlots',
    params: []
  }));
  
  ws.send(JSON.stringify({
    jsonrpc: '2.0',
    id: 2,
    method: 'subscribeBlocks',
    params: []
  }));
  
  console.log('📡 Subscribed to Slots and Blocks\n');
});

let slotCount = 0;
let blockCount = 0;

ws.on('message', (data) => {
  const msg = JSON.parse(data.toString());
  
  // Subscription confirmation
  if (msg.id) {
    console.log(`✅ Subscription ${msg.id} confirmed (ID: ${msg.result})\n`);
    return;
  }
  
  // Notification
  if (msg.method === 'subscription') {
    const { subscription, result } = msg.params;
    
    if (result.slot !== undefined && !result.hash) {
      // Slot event
      slotCount++;
      if (slotCount % 5 === 0) {
        console.log(`⏱️  Slot ${result.slot} (${slotCount} slots received)`);
      }
    } else if (result.hash) {
      // Block event
      blockCount++;
      console.log(`\n📦 BLOCK #${result.slot}`);
      console.log(`   Hash: ${result.hash.substring(0, 16)}...`);
      console.log(`   TXs: ${result.transaction_count || result.transactions || 0}`);
      console.log(`   Validator: ${result.validator.substring(0, 16)}...\n`);
    }
  }
});

ws.on('error', (error) => {
  console.error('❌ WebSocket error:', error.message);
});

ws.on('close', () => {
  console.log(`\n🛑 Connection closed`);
  console.log(`📊 Stats: ${slotCount} slots, ${blockCount} blocks received\n`);
  process.exit(0);
});

// Run for 15 seconds
setTimeout(() => {
  ws.close();
}, 15000);

console.log('🔌 Connecting to ws://localhost:8900...\n');
