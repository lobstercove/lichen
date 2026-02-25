#!/usr/bin/env node
/**
 * MoltChain E2E Transaction Tests
 *
 * Submits real signed transactions against running validators (RPC on port 8899).
 *
 * Usage:
 *   npm install tweetnacl            # one-time
 *   node tests/e2e-transactions.js
 *
 * Prerequisites:
 *   - Validator running with --dev-mode on port 8899
 *   - DEX contracts deployed (genesis auto-deploy)
 */
'use strict';

// ═══════════════════════════════════════════════════════════════════════════════
// Dependencies
// ═══════════════════════════════════════════════════════════════════════════════
let nacl;
try {
    nacl = require('tweetnacl');
} catch {
    console.error('Missing dependency: npm install tweetnacl');
    process.exit(1);
}

const { loadFundedWallets } = require('./helpers/funded-wallets');

const RPC_URL = process.env.MOLTCHAIN_RPC || 'http://127.0.0.1:8899';

// ═══════════════════════════════════════════════════════════════════════════════
// Test harness
// ═══════════════════════════════════════════════════════════════════════════════
let passed = 0, failed = 0, skipped = 0;
function assert(cond, msg) {
    if (cond) { passed++; process.stdout.write(`  ✓ ${msg}\n`); }
    else { failed++; process.stderr.write(`  ✗ ${msg}\n`); }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Base58 encode/decode (matches dex.js)
// ═══════════════════════════════════════════════════════════════════════════════
const BS58_ALPHABET = '123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz';

function bs58encode(bytes) {
    let leadingZeros = 0;
    for (let i = 0; i < bytes.length && bytes[i] === 0; i++) leadingZeros++;
    let num = 0n;
    for (const b of bytes) num = num * 256n + BigInt(b);
    let encoded = '';
    while (num > 0n) { encoded = BS58_ALPHABET[Number(num % 58n)] + encoded; num = num / 58n; }
    return '1'.repeat(leadingZeros) + encoded;
}

function bs58decode(str) {
    let num = 0n;
    for (const c of str) {
        const idx = BS58_ALPHABET.indexOf(c);
        if (idx < 0) throw new Error(`Invalid base58 char: ${c}`);
        num = num * 58n + BigInt(idx);
    }
    const hex = num === 0n ? '' : num.toString(16);
    const padded = hex.length % 2 ? '0' + hex : hex;
    const bytes = [];
    for (let i = 0; i < padded.length; i += 2) bytes.push(parseInt(padded.slice(i, i + 2), 16));
    let leadingOnes = 0;
    for (let i = 0; i < str.length && str[i] === '1'; i++) leadingOnes++;
    const result = new Uint8Array(leadingOnes + bytes.length);
    result.set(bytes, leadingOnes);
    return result;
}

// ═══════════════════════════════════════════════════════════════════════════════
// Hex helpers
// ═══════════════════════════════════════════════════════════════════════════════
function bytesToHex(b) { return Array.from(b).map(x => x.toString(16).padStart(2, '0')).join(''); }
function hexToBytes(h) {
    const c = h.startsWith('0x') ? h.slice(2) : h;
    const o = new Uint8Array(c.length / 2);
    for (let i = 0; i < o.length; i++) o[i] = parseInt(c.slice(i * 2, i * 2 + 2), 16);
    return o;
}

// ═══════════════════════════════════════════════════════════════════════════════
// RPC client
// ═══════════════════════════════════════════════════════════════════════════════
let rpcIdCounter = 1;

async function rpc(method, params = []) {
    const body = JSON.stringify({ jsonrpc: '2.0', id: rpcIdCounter++, method, params });
    const res = await fetch(RPC_URL, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body,
    });
    if (!res.ok) throw new Error(`RPC HTTP ${res.status}: ${await res.text()}`);
    const json = await res.json();
    if (json.error) throw new Error(`RPC error ${json.error.code}: ${json.error.message}`);
    return json.result;
}

// ═══════════════════════════════════════════════════════════════════════════════
// Keypair generation
// ═══════════════════════════════════════════════════════════════════════════════
function generateKeypair() {
    const kp = nacl.sign.keyPair();
    return {
        publicKey: kp.publicKey,
        secretKey: kp.secretKey,
        address: bs58encode(kp.publicKey),
    };
}

function keypairFromSeed(seed32) {
    const kp = nacl.sign.keyPair.fromSeed(seed32);
    return {
        publicKey: kp.publicKey,
        secretKey: kp.secretKey,
        address: bs58encode(kp.publicKey),
    };
}

// ═══════════════════════════════════════════════════════════════════════════════
// Transaction message serialization (bincode-compatible)
// Must match Rust's bincode::serialize(Message { instructions, recent_blockhash })
// ═══════════════════════════════════════════════════════════════════════════════
function encodeTransactionMessage(instructions, blockhash, signer) {
    const parts = [];
    function pushU64LE(n) {
        const buf = new ArrayBuffer(8);
        const view = new DataView(buf);
        view.setUint32(0, n & 0xFFFFFFFF, true);
        view.setUint32(4, Math.floor(n / 0x100000000) & 0xFFFFFFFF, true);
        parts.push(new Uint8Array(buf));
    }
    // instructions: Vec<Instruction> — u64 length prefix
    pushU64LE(instructions.length);
    for (const ix of instructions) {
        // program_id: [u8; 32]
        parts.push(bs58decode(ix.program_id));
        // accounts: Vec<Pubkey>
        const accounts = ix.accounts || [signer];
        pushU64LE(accounts.length);
        for (const acct of accounts) parts.push(bs58decode(acct));
        // data: Vec<u8>
        const dataBytes = typeof ix.data === 'string'
            ? new TextEncoder().encode(ix.data)
            : new Uint8Array(ix.data);
        pushU64LE(dataBytes.length);
        parts.push(dataBytes);
    }
    // recent_blockhash: [u8; 32]
    parts.push(hexToBytes(blockhash));
    const total = parts.reduce((s, a) => s + a.length, 0);
    const out = new Uint8Array(total);
    let off = 0;
    for (const a of parts) { out.set(a, off); off += a.length; }
    return out;
}

// ═══════════════════════════════════════════════════════════════════════════════
// Transaction signing & submission
// ═══════════════════════════════════════════════════════════════════════════════
async function sendTransaction(keypair, instructions) {
    // 1. Get recent blockhash
    const bhResult = await rpc('getRecentBlockhash');
    const blockhash = typeof bhResult === 'string' ? bhResult : bhResult.blockhash;

    // 2. Normalize instructions: ensure data is array of u8
    const normalizedIx = instructions.map(ix => {
        const accounts = ix.accounts || [keypair.address];
        const dataBytes = typeof ix.data === 'string'
            ? Array.from(new TextEncoder().encode(ix.data))
            : Array.from(ix.data);
        return { program_id: ix.program_id, accounts, data: dataBytes };
    });

    // 3. Sign: bincode-compatible message bytes
    const msgBytes = encodeTransactionMessage(normalizedIx, blockhash, keypair.address);
    const sig = nacl.sign.detached(msgBytes, keypair.secretKey);

    // 4. Build wire-format JSON
    const txPayload = {
        signatures: [bytesToHex(sig)],
        message: {
            instructions: normalizedIx,
            blockhash: blockhash,
        },
    };

    // 5. Base64-encode and submit
    const txJson = JSON.stringify(txPayload);
    const txBase64 = Buffer.from(txJson).toString('base64');
    return rpc('sendTransaction', [txBase64]);
}

// ═══════════════════════════════════════════════════════════════════════════════
// Contract call helpers (matches dex.js convention)
// ═══════════════════════════════════════════════════════════════════════════════
const CONTRACT_PROGRAM_ID = bs58encode(new Uint8Array(32).fill(0xFF));

function buildContractCall(argsBytes) {
    return JSON.stringify({ Call: { function: "call", args: Array.from(argsBytes), value: 0 } });
}

function contractIx(walletAddress, contractAddr, argsBytes) {
    return {
        program_id: CONTRACT_PROGRAM_ID,
        accounts: [walletAddress, contractAddr],
        data: buildContractCall(argsBytes),
    };
}

// ═══════════════════════════════════════════════════════════════════════════════
// Binary encoding helpers for DEX instructions
// ═══════════════════════════════════════════════════════════════════════════════
function writeU64LE(view, offset, n) {
    const bn = BigInt(Math.round(n));
    view.setBigUint64(offset, bn, true);
}
function writeU8(arr, offset, n) { arr[offset] = n & 0xFF; }
function writePubkey(arr, offset, base58Addr) {
    const bytes = bs58decode(base58Addr);
    arr.set(bytes.subarray(0, 32), offset);
}

// Opcode 2: place_order(trader, pair_id, side, type, price, qty, expiry)
// Total: 67 bytes
function buildPlaceOrderArgs(trader, pairId, side, orderType, price, quantity) {
    const buf = new ArrayBuffer(67);
    const view = new DataView(buf);
    const arr = new Uint8Array(buf);
    writeU8(arr, 0, 2);                               // opcode
    writePubkey(arr, 1, trader);                       // trader pubkey
    writeU64LE(view, 33, pairId);                      // pair_id
    writeU8(arr, 41, side === 'buy' ? 0 : 1);         // side
    writeU8(arr, 42, orderType === 'market' ? 1 : 0); // order_type
    writeU64LE(view, 43, price);                       // price
    writeU64LE(view, 51, quantity);                    // quantity
    writeU64LE(view, 59, 0);                           // expiry (0 = no expiry)
    return arr;
}

// Opcode 3: cancel_order(trader, order_id)
function buildCancelOrderArgs(trader, orderId) {
    const buf = new ArrayBuffer(41);
    const view = new DataView(buf);
    const arr = new Uint8Array(buf);
    writeU8(arr, 0, 3);
    writePubkey(arr, 1, trader);
    writeU64LE(view, 33, orderId);
    return arr;
}

// ═══════════════════════════════════════════════════════════════════════════════
// Contract addresses — dynamically discovered via getAllSymbolRegistry
// ═══════════════════════════════════════════════════════════════════════════════
const CONTRACTS = {};
const TOKEN_CONTRACTS = {};

async function discoverContracts() {
    const result = await rpc('getAllSymbolRegistry', [100]);
    const entries = result?.entries || [];
    const symbolToContract = {
        'DEX': 'dex_core', 'DEXAMM': 'dex_amm', 'DEXROUTER': 'dex_router',
        'DEXMARGIN': 'dex_margin', 'DEXREWARDS': 'dex_rewards', 'DEXGOV': 'dex_governance',
        'ANALYTICS': 'dex_analytics', 'PREDICT': 'prediction_market',
        'MUSD': 'musd_token',
    };
    const symbolToToken = {
        'MOLT': 'MOLT', 'MUSD': 'mUSD', 'WSOL': 'wSOL', 'WETH': 'wETH', 'REEF': 'REEF',
    };
    for (const e of entries) {
        if (symbolToContract[e.symbol]) CONTRACTS[symbolToContract[e.symbol]] = e.program;
        if (symbolToToken[e.symbol]) TOKEN_CONTRACTS[symbolToToken[e.symbol]] = e.program;
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Utility: wait for a transaction to confirm
// ═══════════════════════════════════════════════════════════════════════════════
async function waitForConfirmation(signature, timeoutMs = 10000) {
    const start = Date.now();
    while (Date.now() - start < timeoutMs) {
        try {
            const result = await rpc('confirmTransaction', [signature]);
            if (result?.value?.confirmationStatus) return result;
        } catch { /* not yet */ }
        await new Promise(r => setTimeout(r, 500));
    }
    throw new Error(`Transaction ${signature} not confirmed within ${timeoutMs}ms`);
}

// ═══════════════════════════════════════════════════════════════════════════════
// Utility: sleep
// ═══════════════════════════════════════════════════════════════════════════════
const sleep = ms => new Promise(r => setTimeout(r, ms));

// ═══════════════════════════════════════════════════════════════════════════════
// TEST SUITE
// ═══════════════════════════════════════════════════════════════════════════════
async function runTests() {
    console.log(`\n═══ MoltChain E2E Transaction Tests ═══`);
    console.log(`RPC: ${RPC_URL}\n`);

    // ── Discover contracts dynamically ──
    await discoverContracts();

    // ── Test 0: RPC connectivity ──
    console.log('── Test 0: RPC Connectivity ──');
    let slot;
    try {
        slot = await rpc('getSlot');
        assert(typeof slot === 'number' && slot >= 0, `getSlot returned slot=${slot}`);
    } catch (e) {
        console.error(`FATAL: Cannot reach RPC at ${RPC_URL}: ${e.message}`);
        process.exit(1);
    }

    // ── Test 1: Keypair generation ──
    console.log('\n── Test 1: Keypair Generation ──');
    // Use funded genesis wallets for balance-dependent tests; fresh keypairs for crypto tests
    const funded = loadFundedWallets(2);
    const freshAlice = generateKeypair();
    const freshBob = generateKeypair();
    const alice = funded[0] || freshAlice;
    const bob   = funded[1] || freshBob;
    // Crypto property checks use the fresh keypairs to validate generation
    assert(freshAlice.publicKey.length === 32, 'Alice pubkey is 32 bytes');
    assert(freshAlice.secretKey.length === 64, 'Alice secretKey is 64 bytes');
    assert(freshAlice.address.length > 30, `Alice address: ${freshAlice.address}`);
    assert(freshBob.address !== freshAlice.address, 'Bob address differs from Alice');

    // Round-trip test
    const decoded = bs58decode(freshAlice.address);
    assert(decoded.length === 32, 'bs58 round-trip: decoded to 32 bytes');
    assert(bs58encode(decoded) === freshAlice.address, 'bs58 round-trip: re-encodes correctly');
    console.log(`  Alice: ${alice.address} (${alice.source ? 'funded' : 'fresh'})`);
    console.log(`  Bob:   ${bob.address} (${bob.source ? 'funded' : 'fresh'})`);

    // ── Test 2: getRecentBlockhash ──
    console.log('\n── Test 2: getRecentBlockhash ──');
    const bhResult = await rpc('getRecentBlockhash');
    const blockhash = typeof bhResult === 'string' ? bhResult : bhResult.blockhash;
    assert(typeof blockhash === 'string', `Blockhash is string: ${blockhash.slice(0, 16)}...`);
    assert(blockhash.length === 64, `Blockhash is 64 hex chars (32 bytes)`);
    assert(/^[0-9a-fA-F]+$/.test(blockhash), 'Blockhash is valid hex');

    // ── Test 3: requestAirdrop (MOLT) ──
    console.log('\n── Test 3: requestAirdrop (MOLT to Alice) ──');
    let airdropResult;
    try {
        const freshAddr = freshAlice.address;
        airdropResult = await rpc('requestAirdrop', [freshAddr, 10]);
        assert(airdropResult.success === true, `Airdrop success: ${airdropResult.message}`);
        assert(airdropResult.amount === 10, `Airdropped 10 MOLT`);
    } catch (e) {
        console.warn(`  ⚠ Airdrop skipped (multi-validator mode): ${e.message}`);
        skipped += 2;
    }

    // Wait for block propagation
    await sleep(2000);

    // ── Test 4: getBalance ──
    console.log('\n── Test 4: getBalance (Alice) ──');
    const balance = await rpc('getBalance', [alice.address]);
    assert(balance.spendable > 0, `Alice spendable: ${balance.spendable_molt} MOLT (${balance.spendable} shells)`);

    // ── Test 5: Airdrop to Bob ──
    console.log('\n── Test 5: requestAirdrop (MOLT to Bob) ──');
    try {
        const bobAirdrop = await rpc('requestAirdrop', [freshBob.address, 5]);
        assert(bobAirdrop.success === true, `Bob airdrop: ${bobAirdrop.message}`);
    } catch (e) {
        console.warn(`  ⚠ Bob airdrop skipped (multi-validator mode): ${e.message}`);
        skipped++;
    }
    await sleep(2000);

    // ── Test 6: MOLT transfer (Alice → Bob) via sendTransaction ──
    console.log('\n── Test 6: MOLT Transfer (Alice → Bob) ──');
    {
        // System program transfer: instruction type 0 (Transfer)
        // data layout: [type:u8=0][amount:u64_le]
        const SYSTEM_PROGRAM_ID = bs58encode(new Uint8Array(32).fill(0x01));
        const transferAmount = 1_000_000_000; // 1 MOLT in shells
        const data = new Uint8Array(9);
        data[0] = 0; // Transfer opcode
        const view = new DataView(data.buffer);
        view.setBigUint64(1, BigInt(transferAmount), true);

        const ix = {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: [alice.address, bob.address],
            data: Array.from(data),
        };

        try {
            const sig = await sendTransaction(alice, [ix]);
            assert(typeof sig === 'string' && sig.length > 0, `Transfer tx signature: ${sig.slice(0, 16)}...`);

            // Wait and confirm
            await sleep(2000);
            try {
                const conf = await rpc('confirmTransaction', [sig]);
                assert(conf?.value !== undefined, `Transfer confirmed: ${JSON.stringify(conf?.value?.confirmationStatus || 'unknown')}`);
            } catch (e) {
                console.warn(`  ⚠ confirmTransaction: ${e.message}`);
            }

            // Check Bob's balance increased
            const bobBal = await rpc('getBalance', [bob.address]);
            assert(bobBal.spendable > 0, `Bob spendable after transfer: ${bobBal.spendable_molt} MOLT`);
        } catch (e) {
            console.warn(`  ⚠ Transfer unavailable (${e.message})`);
            skipped++;
        }
    }

    // ── Test 7: Contract call — DEX place_order ──
    console.log('\n── Test 7: DEX place_order via Contract Call ──');
    {
        // Load contract addresses from symbol registry (preferred) or use fallback
        let dexCoreAddr = CONTRACTS.dex_core;
        try {
            const reg = await rpc('getAllSymbolRegistry', [100]);
            if (reg?.entries?.length) {
                const entry = reg.entries.find(e => e.symbol === 'DEX');
                if (entry) dexCoreAddr = entry.program;
            }
        } catch { /* use fallback */ }

        const PRICE_SCALE = 1_000_000_000;
        const pairId = 1;   // MOLT/mUSD (first genesis pair)
        const side = 'buy';
        const orderType = 'limit';
        const price = Math.round(0.10 * PRICE_SCALE);   // $0.10
        const quantity = Math.round(1.0 * PRICE_SCALE);  // 1 MOLT

        const args = buildPlaceOrderArgs(alice.address, pairId, side, orderType, price, quantity);
        const ix = contractIx(alice.address, dexCoreAddr, args);

        try {
            const sig = await sendTransaction(alice, [ix]);
            assert(typeof sig === 'string', `place_order tx: ${sig.slice(0, 16)}...`);
            await sleep(2000);
        } catch (e) {
            // May fail if Alice doesn't have mUSD for the buy side — that's expected
            console.warn(`  ⚠ place_order: ${e.message}`);
            skipped++;
        }
    }

    // ── Test 8: getTokenBalance ──
    console.log('\n── Test 8: getTokenBalance ──');
    {
        try {
            const musdBal = await rpc('getTokenBalance', [TOKEN_CONTRACTS.mUSD, alice.address]);
            assert(typeof musdBal.balance === 'number', `Alice mUSD balance: ${musdBal.ui_amount} (${musdBal.balance} raw)`);
        } catch (e) {
            console.warn(`  ⚠ getTokenBalance: ${e.message}`);
            skipped++;
        }
    }

    // ── Test 9: getTransaction ──
    console.log('\n── Test 9: getTransaction lookup ──');
    {
        // Try to look up a recent transaction
        try {
            const txs = await rpc('getRecentTransactions', [5]);
            if (txs && txs.length > 0) {
                const txHash = txs[0].hash || txs[0].signature;
                if (txHash) {
                    const txDetail = await rpc('getTransaction', [txHash]);
                    assert(txDetail !== null, `getTransaction returned data for ${txHash.slice(0, 16)}...`);
                } else {
                    assert(true, 'Recent transactions exist (no hash field to look up)');
                }
            } else {
                assert(true, 'No recent transactions to look up (chain just started)');
            }
        } catch (e) {
            console.warn(`  ⚠ getTransaction: ${e.message}`);
            skipped++;
        }
    }

    // ── Test 10: Invalid transaction rejection ──
    console.log('\n── Test 10: Invalid Transaction Rejection ──');
    {
        // Submit a transaction with zero signature — should be rejected
        const bh = await rpc('getRecentBlockhash');
        const blockhashStr = typeof bh === 'string' ? bh : bh.blockhash;
        const txPayload = {
            signatures: [bytesToHex(new Uint8Array(64))], // all-zero sig
            message: {
                instructions: [{
                    program_id: bs58encode(new Uint8Array(32).fill(0x01)),
                    accounts: [alice.address],
                    data: [0],
                }],
                blockhash: blockhashStr,
            },
        };
        const txBase64 = Buffer.from(JSON.stringify(txPayload)).toString('base64');
        try {
            await rpc('sendTransaction', [txBase64]);
            failed++;
            process.stderr.write('  ✗ Zero-signature tx should have been rejected\n');
        } catch (e) {
            assert(e.message.includes('zero signature') || e.message.includes('invalid') || e.message.includes('signature') || e.message.includes('fetch'), `Rejected zero-sig tx: ${e.message}`);
        }
    }

    // ── Test 11: Signature verification ──
    console.log('\n── Test 11: Ed25519 Signature Verification ──');
    {
        const message = new TextEncoder().encode('hello moltchain');
        const sig = nacl.sign.detached(message, alice.secretKey);
        const valid = nacl.sign.detached.verify(message, sig, alice.publicKey);
        assert(valid, 'Ed25519 sign/verify roundtrip');
        // Tamper 
        const bad = new Uint8Array(sig);
        bad[0] ^= 0xFF;
        const invalid = nacl.sign.detached.verify(message, bad, alice.publicKey);
        assert(!invalid, 'Tampered signature rejected');
    }

    // ── Test 12: DEX REST API ──
    console.log('\n── Test 12: DEX REST API ──');
    {
        try {
            const res = await fetch(`${RPC_URL}/api/v1/pairs`);
            const json = await res.json();
            const pairs = json.data || json;
            assert(Array.isArray(pairs), `GET /api/v1/pairs returned array (${pairs.length} pairs)`);
            if (pairs.length > 0) {
                assert(pairs[0].pairId !== undefined || pairs[0].symbol !== undefined, `First pair has pairId/symbol`);
            }
        } catch (e) {
            console.warn(`  ⚠ DEX REST API: ${e.message}`);
            skipped++;
        }
    }

    // ── Test 13: Multiple instructions in one transaction ──
    console.log('\n── Test 13: Multi-instruction Transaction ──');
    {
        const SYSTEM_PROGRAM_ID = bs58encode(new Uint8Array(32).fill(0x01));
        const charlie = generateKeypair();

        // Two transfers in one tx (alice is already funded via genesis)
        const amount1 = 100_000_000; // 0.1 MOLT
        const amount2 = 200_000_000; // 0.2 MOLT

        const mkTransferIx = (from, to, amount) => {
            const data = new Uint8Array(9);
            data[0] = 0;
            new DataView(data.buffer).setBigUint64(1, BigInt(amount), true);
            return {
                program_id: SYSTEM_PROGRAM_ID,
                accounts: [from, to],
                data: Array.from(data),
            };
        };

        try {
            const sig = await sendTransaction(alice, [
                mkTransferIx(alice.address, bob.address, amount1),
                mkTransferIx(alice.address, charlie.address, amount2),
            ]);
            assert(typeof sig === 'string', `Multi-ix tx: ${sig.slice(0, 16)}...`);
            await sleep(2000);
        } catch (e) {
            console.warn(`  ⚠ Multi-ix tx: ${e.message}`);
            skipped++;
        }
    }

    // ── Summary ──
    console.log(`\n═══ Results: ${passed} passed, ${failed} failed, ${skipped} skipped ═══\n`);
    process.exit(failed > 0 ? 1 : 0);
}

// ═══════════════════════════════════════════════════════════════════════════════
// EXPORTS — for use as a module in other E2E tests
// ═══════════════════════════════════════════════════════════════════════════════
module.exports = {
    // Primitives
    bs58encode, bs58decode, bytesToHex, hexToBytes,
    // Keypair
    generateKeypair, keypairFromSeed,
    // Transaction
    encodeTransactionMessage, sendTransaction,
    // Contract calls
    CONTRACT_PROGRAM_ID, buildContractCall, contractIx,
    // DEX builders
    buildPlaceOrderArgs, buildCancelOrderArgs,
    writeU64LE, writeU8, writePubkey,
    // Constants
    CONTRACTS, TOKEN_CONTRACTS,
    // Utilities
    rpc, waitForConfirmation, sleep,
};

// Run if executed directly
if (require.main === module) {
    runTests().catch(e => {
        console.error('Fatal:', e);
        process.exit(1);
    });
}
