const fs = require('fs');

const sharedConfig = fs.readFileSync('marketplace/shared-config.js', 'utf8');
console.log('M-33.1:', sharedConfig.includes("ws: 'wss://ws.lichen.network'"));
console.log('M-33.2:', sharedConfig.includes("ws: 'wss://testnet-ws.lichen.network'"));

const contractRs = fs.readFileSync('contracts/lichenmarket/src/lib.rs', 'utf8');
console.log('M-38.7:', contractRs.includes('call_token_transfer(') && contractRs.includes('seller,') && contractRs.includes('royalty_amount,'));

const faucetSrc = fs.readFileSync('faucet-service/src/main.rs', 'utf8');
console.log('C22.5-exit:', faucetSrc.includes('std::process::exit(1)'));
console.log('C22.5-eprintln:', faucetSrc.includes('eprintln!'));
const hasPanic = faucetSrc.includes('panic!("\u274C Faucet cannot run on mainnet!")');
console.log('C22.5-panic:', hasPanic);
if (!hasPanic) {
    const lines = faucetSrc.split('\n').filter(l => l.includes('panic!'));
    console.log('  actual panic lines:', lines.map(l => l.trim()));
}
