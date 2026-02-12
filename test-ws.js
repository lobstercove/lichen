// Test WebSocket connection and subscriptions
const WebSocket = require('ws');

const ws = new WebSocket('ws://localhost:8900');

ws.on('open', () => {
  console.log('\u2705 Connected to WebSocket server');
  
  // Subscribe to slots
  const subscribeMessage = {
    jsonrpc: '2.0',
    id: 1,
    method: 'subscribeSlots',
    params: []
  };
  
  ws.send(JSON.stringify(subscribeMessage));
  console.log('Sent subscription request:', subscribeMessage);
});

ws.on('message', (data) => {
  console.log('Received:', data.toString());
});

ws.on('error', (error) => {
  console.error('WebSocket error:', error.message);
});

ws.on('close', () => {
  console.log('Connection closed');
});

// Keep alive for 10 seconds
setTimeout(() => {
  ws.close();
  process.exit(0);
}, 10000);
