#!/usr/bin/env node
// Debug script: place a DEX order and check contract storage
const http = require('http');
const pq = require('./helpers/pq-node');
const fs = require('fs');
const path = require('path');

const RPC = 'http://127.0.0.1:8899';

function rpc(method, params) {
    return new Promise((resolve, reject) => {
        const body = JSON.stringify({ jsonrpc: '2.0', id: 1, method, params: params || [] });
        const url = new URL(RPC);
        const req = http.request({ hostname: url.hostname, port: url.port, method: 'POST', path: '/', headers: { 'Content-Type': 'application/json' } }, res => {
            let d = ''; res.on('data', c => d += c); res.on('end', () => {
                try { const r = JSON.parse(d); if (r.error) reject(new Error(JSON.stringify(r.error))); else resolve(r.result); } catch (e) { reject(e) }
            });
        });
        req.on('error', reject); req.write(body); req.end();
    });
}

function bs58chars() { return '123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz'; }
function bs58encode(bytes) {
    const ALPHABET = bs58chars();
    let num = BigInt(0);
    for (const b of bytes) num = num * 256n + BigInt(b);
    let str = '';
    while (num > 0n) { str = ALPHABET[Number(num % 58n)] + str; num /= 58n; }
    for (const b of bytes) { if (b !== 0) break; str = '1' + str; }
    return str || '1';
}
function bs58decode(s) {
    const ALPHABET = bs58chars();
    let num = BigInt(0);
    for (const c of s) { const i = ALPHABET.indexOf(c); if (i < 0) throw new Error('bad b58'); num = num * 58n + BigInt(i); }
    const hex = num.toString(16).padStart(2, '0');
    const bytes = [];
    for (let i = 0; i < hex.length; i += 2) bytes.push(parseInt(hex.substr(i, 2), 16));
    while (bytes.length < 32) bytes.unshift(0);
    for (const c of s) { if (c !== '1') break; bytes.unshift(0); }
    return new Uint8Array(bytes);
}
function hexToBytes(h) { const b = []; for (let i = 0; i < h.length; i += 2) b.push(parseInt(h.substr(i, 2), 16)); return new Uint8Array(b); }
function bytesToHex(b) { return Array.from(b).map(x => x.toString(16).padStart(2, '0')).join(''); }

function genKeypair() {
    return pq.generateKeypair();
}

async function main() {
    await pq.init();
    console.log('=== DEX Order Debug ===');

    // 1. Check symbol registry for DEX
    const dexInfo = await rpc('getSymbolRegistry', ['DEX']);
    console.log('DEX contract:', dexInfo.program);

    // 2. Load funded wallet from genesis keys
    const { loadFundedWallets } = require('./helpers/funded-wallets');
    const funded = loadFundedWallets(2);
    let wallet = funded[0];
    if (wallet) {
        console.log('Using funded wallet:', wallet.address, '(from', wallet.source, ')');
    } else {
        wallet = genKeypair();
        console.log('Generated wallet (no funded wallets found):', wallet.address);
    }

    // 3. Check balance
    const bal = await rpc('getBalance', [wallet.address]);
    console.log('Balance:', bal.spendable_licn, 'LICN');

    // 4. Try to place order via contract call
    const CONTRACT_PID = bs58encode(new Uint8Array(32).fill(0xFF));
    const PRICE_SCALE = 1000000000;

    // place_order: opcode 2, pairId=1, sell, limit, price=0.12, qty=5
    const buf = new ArrayBuffer(67);
    const v = new DataView(buf);
    const a = new Uint8Array(buf);
    a[0] = 2; // opcode place_order
    a.set(bs58decode(wallet.address).subarray(0, 32), 1); // trader
    v.setBigUint64(33, 1n, true); // pairId
    a[41] = 1; // side: sell
    a[42] = 0; // type: limit
    v.setBigUint64(43, BigInt(Math.round(0.12 * PRICE_SCALE)), true); // price
    v.setBigUint64(51, BigInt(Math.round(5 * PRICE_SCALE)), true); // qty
    v.setBigUint64(59, 0n, true); // reserved

    const callData = JSON.stringify({ Call: { function: "call", args: Array.from(a), value: 0 } });

    // Build TX
    const bhRes = await rpc('getRecentBlockhash');
    const bh = typeof bhRes === 'string' ? bhRes : bhRes.blockhash;
    console.log('Blockhash:', bh);

    const ix = {
        program_id: CONTRACT_PID,
        accounts: [wallet.address, dexInfo.program],
        data: Array.from(new TextEncoder().encode(callData)),
    };

    // Encode message
    const parts = [];
    function pushU64(n) {
        const b = new ArrayBuffer(8); const dv = new DataView(b);
        dv.setUint32(0, n & 0xFFFFFFFF, true); dv.setUint32(4, Math.floor(n / 0x100000000) & 0xFFFFFFFF, true);
        parts.push(new Uint8Array(b));
    }
    pushU64(1); // 1 instruction
    parts.push(bs58decode(ix.program_id));
    pushU64(ix.accounts.length);
    for (const acct of ix.accounts) parts.push(bs58decode(acct));
    pushU64(ix.data.length);
    parts.push(new Uint8Array(ix.data));
    parts.push(hexToBytes(bh));
    parts.push(new Uint8Array([0x00]));  // compute_budget: None
    parts.push(new Uint8Array([0x00]));  // compute_unit_price: None

    const total = parts.reduce((s, p) => s + p.length, 0);
    const msg = new Uint8Array(total);
    let off = 0;
    for (const p of parts) { msg.set(p, off); off += p.length; }

    const pqSig = pq.sign(msg, wallet);
    const payload = {
        signatures: [pqSig],
        message: { instructions: [ix], blockhash: bh }
    };
    const b64 = Buffer.from(JSON.stringify(payload)).toString('base64');

    console.log('Sending TX...');
    let txSig;
    try {
        txSig = await rpc('sendTransaction', [b64]);
        console.log('TX signature:', txSig);
    } catch (e) {
        console.log('sendTransaction error:', e.message);
        return;
    }

    // 5. Wait for TX confirmation
    console.log('Waiting for confirmation...');
    for (let i = 0; i < 30; i++) {
        await new Promise(r => setTimeout(r, 500));
        try {
            const tx = await rpc('getTransaction', [txSig]);
            if (tx) {
                console.log('TX confirmed at slot:', tx.slot);
                console.log('TX success:', tx.success);
                console.log('TX type:', tx.type);
                if (tx.logs) console.log('Logs:', tx.logs);
                if (tx.log_messages) console.log('Log messages:', tx.log_messages);
                if (tx.error) console.log('Error:', tx.error);
                console.log('Full TX:', JSON.stringify(tx, null, 2).substring(0, 500));
                break;
            }
        } catch (e) { }
    }

    // 6. Check orderbook
    await new Promise(r => setTimeout(r, 3000));
    const obResp = await new Promise((resolve, reject) => {
        http.get('http://127.0.0.1:8899/api/v1/pairs/1/orderbook', res => {
            let d = ''; res.on('data', c => d += c); res.on('end', () => resolve(JSON.parse(d)));
        }).on('error', reject);
    });
    console.log('\nOrderbook after order:', JSON.stringify(obResp.data, null, 2));
}

main().catch(e => console.error('FATAL:', e));
