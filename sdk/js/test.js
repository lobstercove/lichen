// Simple JS test of SDK
const { Connection } = require('./dist/index');

async function test() {
  console.log('Testing Lichen SDK...');
  
  const connection = new Connection('http://localhost:8899');
  console.log('Connection created');
  
  try {
    const info = await connection.getNetworkInfo();
    console.log('Network Info:', info);
  } catch (error) {
    console.error('Error:', error.message);
  }
}

test().catch(console.error);
