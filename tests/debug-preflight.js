#!/usr/bin/env node
'use strict';
const nacl = require('tweetnacl');
const ALPHABET = '123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz';
function bs58encode(bytes) {
    let num = BigInt(0); for (const b of bytes) num = num * 256n + BigInt(b);
    let s = ''; while (num > 0n) { s = ALPHABET[Number(num % 58n)] + s; num /= 58n; }
    for (const b of bytes) { if (b !== 0) break; s = '1' + s; } return s;
}
function bs58decode(str) {
    let num = BigInt(0); for (const c of str) num = num * 58n + BigInt(ALPHABET.indexOf(c));
    const hex = num.toString(16).padStart(64, '0');
    return new Uint8Array(hex.match(/.{2}/g).map(h => parseInt(h, 16)));
}
function bytesToHex(b) { return Array.from(b).map(x => x.toString(16).padStart(2, '0')).join(''); }

const RPC = 'http://127.0.0.1:8899';
async function rpc(method, params = []) {
    const r = await fetch(RPC, {
        method: 'POST', headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ jsonrpc: '2.0', id: 1, method, params })
    }).then(r => r.json());
    if (r.error) throw new Error(`RPC ${r.error.code}: ${r.error.message}`);
    return r.result;
}

function encodeMsg(instructions, bh, signer) {
    const parts = [Buffer.from(bh, 'hex')];
    for (const ix of instructions) {
        parts.push(Buffer.from(bs58decode(ix.program_id)));
        const abuf = Buffer.alloc(4); abuf.writeUInt32LE(ix.accounts.length); parts.push(abuf);
        for (const ac of ix.accounts) parts.push(Buffer.from(bs58decode(ac)));
        const dbuf = Buffer.alloc(4); dbuf.writeUInt32LE(ix.data.length); parts.push(dbuf);
        parts.push(Buffer.from(ix.data));
    }
    return Buffer.concat(parts);
}

async function sendTx(keypair, instructions) {
    const bhRes = await rpc('getRecentBlockhash');
    const bh = typeof bhRes === 'string' ? bhRes : bhRes.blockhash;
    const nix = instructions.map(ix => ({
        program_id: ix.program_id,
        accounts: ix.accounts || [keypair.address],
        data: typeof ix.data === 'string' ? Array.from(new TextEncoder().encode(ix.data)) : Array.from(ix.data),
    }));
    const msg = encodeMsg(nix, bh, keypair.address);
    const sig = nacl.sign.detached(msg, keypair.secretKey);
    const payload = { signatures: [bytesToHex(sig)], message: { instructions: nix, blockhash: bh } };
    const b64 = Buffer.from(JSON.stringify(payload)).toString('base64');
    return rpc('sendTransaction', [b64]);
}

const CONTRACT_PID = bs58encode(new Uint8Array(32).fill(0xFF));

(async () => {
    // Discover PREDICT contract
    const reg = await rpc('getAllSymbolRegistry', [100]);
    const entries = Array.isArray(reg) ? reg : (reg.entries || []);
    const predict = entries.find(e => e.symbol === 'PREDICT')?.program;
    console.log('PREDICT:', predict);

    // Create and fund test keypair
    const kp = nacl.sign.keyPair();
    const addr = bs58encode(kp.publicKey);
    await rpc('requestAirdrop', [addr, 10]);
    console.log('Funded:', addr);
    await new Promise(r => setTimeout(r, 2000));

    // Build buy_shares TX for non-existent market 999
    const buf = new ArrayBuffer(50);
    const a = new Uint8Array(buf);
    const v = new DataView(buf);
    a[0] = 4; // opcode buy_shares
    a.set(bs58decode(addr), 1); // trader
    v.setBigUint64(33, 999n, true); // market_id = 999
    a[41] = 0; // outcome = 0
    v.setBigUint64(42, 5000000n, true); // amount = 5 lUSD

    const data = JSON.stringify({ Call: { function: 'call', args: Array.from(a), value: 0 } });
    const ix = { program_id: CONTRACT_PID, accounts: [addr, predict], data };

    try {
        const sig = await sendTx({ address: addr, publicKey: kp.publicKey, secretKey: kp.secretKey }, [ix]);
        console.log('TX accepted (should have been REJECTED!):', sig);
    } catch (e) {
        console.log('TX correctly rejected:', e.message);
    }
})().catch(e => { console.error('FATAL:', e); process.exit(1); });
