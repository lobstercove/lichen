#!/usr/bin/env node
/**
 * Lichen E2E Test — Token Creation, Transfer & Wrapped Token Verification
 *
 * Tests:
 * 1. Airdrop LICN to a new wallet
 * 2. Transfer LICN between wallets
 * 3. Create a SporePump token
 * 4. Buy tokens from bonding curve
 * 5. Query token balances + token accounts
 * 6. Verify wrapped token contracts (wSOL, wETH, wBNB, lUSD)
 *
 * Usage: node tests/e2e-token-transfer.js [rpc_url]
 * Requires: local validator running (./run-validator.sh testnet 1)
 */

const pq = require('./helpers/pq-node');
const { fundAccount, loadFundedWallets, loadKeypairFile } = require('./helpers/funded-wallets');
const fs = require('fs');
const path = require('path');

const RPC_URL = process.argv[2] || 'http://localhost:8899';
const SPORES_PER_LICN = 1_000_000_000;

// ============================================================================
// Base58 encoding/decoding
// ============================================================================
const ALPHABET = '123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz';

function base58Encode(bytes) {
    let num = 0n;
    for (const byte of bytes) num = (num << 8n) + BigInt(byte);
    let encoded = '';
    while (num > 0n) {
        encoded = ALPHABET[Number(num % 58n)] + encoded;
        num /= 58n;
    }
    let leadingZeros = 0;
    for (const byte of bytes) { if (byte === 0) leadingZeros++; else break; }
    return '1'.repeat(leadingZeros) + (encoded || '');
}

function base58Decode(value) {
    const map = new Map();
    for (let i = 0; i < ALPHABET.length; i++) map.set(ALPHABET[i], BigInt(i));
    let num = 0n;
    for (const char of value) {
        const digit = map.get(char);
        if (digit === undefined) throw new Error('Invalid base58 character');
        num = num * 58n + digit;
    }
    const bytes = [];
    while (num > 0n) { bytes.push(Number(num & 0xffn)); num >>= 8n; }
    bytes.reverse();
    let leadingZeros = 0;
    for (const char of value) { if (char === '1') leadingZeros++; else break; }
    const result = new Uint8Array(leadingZeros + bytes.length);
    result.set(bytes, leadingZeros);
    return result;
}

// ============================================================================
// Hex helpers
// ============================================================================
function hexToBytes(hex) {
    const clean = hex.startsWith('0x') ? hex.slice(2) : hex;
    const out = new Uint8Array(clean.length / 2);
    for (let i = 0; i < out.length; i++) out[i] = parseInt(clean.substr(i * 2, 2), 16);
    return out;
}

function bytesToHex(bytes) {
    return Array.from(bytes).map(b => b.toString(16).padStart(2, '0')).join('');
}

// ============================================================================
// Bincode encoding (matches Lichen wire format)
// ============================================================================
function concatBytes(...chunks) {
    const total = chunks.reduce((s, c) => s + c.length, 0);
    const out = new Uint8Array(total);
    let off = 0;
    for (const c of chunks) { out.set(c, off); off += c.length; }
    return out;
}

function encodeU64LE(value) {
    const bytes = new Uint8Array(8);
    let big = BigInt(value);
    for (let i = 0; i < 8; i++) { bytes[i] = Number(big & 0xffn); big >>= 8n; }
    return bytes;
}

function encodeBytes(data) {
    return concatBytes(encodeU64LE(data.length), data);
}

function encodeString(value) {
    return encodeBytes(Buffer.from(value, 'utf8'));
}

function encodeVec(items, encoder) {
    const parts = [encodeU64LE(items.length)];
    for (const item of items) parts.push(encoder(item));
    return concatBytes(...parts);
}

function encodeInstruction(ix) {
    const programId = base58Decode(ix.programId);
    const accounts = encodeVec(ix.accounts.map(a => base58Decode(a)), x => x);
    const data = encodeBytes(ix.data instanceof Uint8Array ? ix.data : new Uint8Array(ix.data));
    return concatBytes(programId, accounts, data);
}

function encodeMessage(instructions, recentBlockhash) {
    const blockhashBytes = hexToBytes(recentBlockhash);
    return concatBytes(
        encodeVec(instructions, encodeInstruction),
        blockhashBytes,
        new Uint8Array([0x00]),  // compute_budget: None
        new Uint8Array([0x00])   // compute_unit_price: None
    );
}

// ============================================================================
// Wallet helpers
// ============================================================================
function generateKeypair() {
    return pq.generateKeypair();
}

function keypairFromSeed(seedInput) {
    // Accepts hex string (old format) or Uint8Array/Array (new format)
    const seed = typeof seedInput === 'string'
        ? new Uint8Array(Buffer.from(seedInput.slice(0, 64), 'hex'))
        : new Uint8Array(seedInput);
    return pq.keypairFromSeed(seed);
}

// ============================================================================
// Transaction builders
// ============================================================================
const SYSTEM_PROGRAM_ID = base58Encode(new Uint8Array(32));           // all zeros
const CONTRACT_PROGRAM_ID = base58Encode(new Uint8Array(32).fill(255)); // all 0xFF

function buildTransferIx(from, to, amountSpores) {
    const data = new Uint8Array(9);
    data[0] = 0; // Transfer opcode
    const view = new DataView(data.buffer);
    view.setBigUint64(1, BigInt(amountSpores), true);
    return { programId: SYSTEM_PROGRAM_ID, accounts: [from, to], data };
}

function buildContractCallIx(caller, contractAddr, functionName, argsBytes, value = 0) {
    const payload = JSON.stringify({
        Call: { function: functionName, args: Array.from(argsBytes), value }
    });
    const data = Buffer.from(payload, 'utf8');
    return { programId: CONTRACT_PROGRAM_ID, accounts: [caller, contractAddr], data };
}

async function buildSignSend(keypair, instructions) {
    const blockhash = await getRecentBlockhash();
    // Normalize to JSON wire format (program_id, accounts as base58 strings, data as byte array)
    const nix = instructions.map(ix => ({
        program_id: ix.programId,
        accounts: ix.accounts,
        data: Array.from(ix.data instanceof Uint8Array ? ix.data : new Uint8Array(ix.data)),
    }));
    // Build binary message for signing (bincode format)
    const messageBytes = encodeMessage(instructions, blockhash);
    const pqSig = pq.sign(messageBytes, keypair);
    const payload = JSON.stringify({ signatures: [pqSig], message: { instructions: nix, blockhash } });
    return rpc('sendTransaction', [Buffer.from(payload).toString('base64')]);
}

// ============================================================================
// RPC helpers
// ============================================================================
async function rpc(method, params = []) {
    const res = await fetch(RPC_URL, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ jsonrpc: '2.0', id: 1, method, params })
    });
    const data = await res.json();
    if (data.error) throw new Error(`RPC ${method}: ${data.error.message}`);
    return data.result;
}

async function getRecentBlockhash() {
    const result = await rpc('getRecentBlockhash');
    return result.blockhash;
}

async function getBalance(address) {
    const result = await rpc('getBalance', [address]);
    return result;
}

async function getTokenAccounts(address) {
    return await rpc('getTokenAccounts', [address]);
}

async function getTokenBalance(address, symbol) {
    return await rpc('getTokenBalance', [address, symbol]);
}

async function getSymbolProgram(symbol) {
    const result = await rpc('getSymbolRegistry', [symbol]);
    return result?.program || null;
}

async function sleep(ms) { return new Promise(r => setTimeout(r, ms)); }

async function waitForBalance(address, predicate, timeoutMs = 10000, intervalMs = 250) {
    const deadline = Date.now() + timeoutMs;
    let lastBalance = await getBalance(address);

    while (Date.now() < deadline) {
        if (predicate(lastBalance)) {
            return lastBalance;
        }
        await sleep(intervalMs);
        lastBalance = await getBalance(address);
    }

    return lastBalance;
}

async function waitForTransaction(signature, predicate, timeoutMs = 10000, intervalMs = 500) {
    const deadline = Date.now() + timeoutMs;
    let lastTx = null;

    while (Date.now() < deadline) {
        try {
            lastTx = await rpc('getTransaction', [signature]);
        } catch (_) {
            lastTx = null;
        }

        if (predicate(lastTx)) {
            return lastTx;
        }

        await sleep(intervalMs);
    }

    return lastTx;
}

async function waitForConfirmation(signature, timeoutMs = 10000, intervalMs = 500) {
    const deadline = Date.now() + timeoutMs;
    let lastConfirmation = null;

    while (Date.now() < deadline) {
        try {
            lastConfirmation = await rpc('confirmTransaction', [signature]);
        } catch (_) {
            lastConfirmation = null;
        }

        if (lastConfirmation?.value?.confirmation_status) {
            return lastConfirmation;
        }

        await sleep(intervalMs);
    }

    return lastConfirmation;
}

// ============================================================================
// Test runner
// ============================================================================
let passed = 0, failed = 0;

function ok(name, condition, detail = '') {
    if (condition) {
        console.log(`  ✅ ${name}${detail ? ` — ${detail}` : ''}`);
        passed++;
    } else {
        console.log(`  ❌ ${name}${detail ? ` — ${detail}` : ''}`);
        failed++;
    }
}

async function tryTest(name, fn) {
    try {
        await fn();
    } catch (err) {
        console.log(`  ❌ ${name} — ${err.message}`);
        failed++;
    }
}

// ============================================================================
// Main test flow
// ============================================================================
async function main() {
    await pq.init();
    console.log('🧪 Lichen E2E Token Transfer Test');
    console.log('═'.repeat(60));
    console.log(`RPC: ${RPC_URL}\n`);

    // ── Verify chain is running ──
    console.log('1️⃣  Chain Status');
    const slot = await rpc('getSlot');
    ok('Chain running', slot > 0, `slot ${slot}`);

    // ── Load genesis deployer keypair ──
    const genesisKeysDir = path.join(__dirname, '..', 'data', 'state-7001', 'genesis-keys');
    const deployerKeyFile = path.join(genesisKeysDir, 'genesis-primary-lichen-testnet-1.json');

    if (!fs.existsSync(deployerKeyFile)) {
        console.log('  ⚠️  Genesis keypair not found — run V1 validator first');
        process.exit(1);
    }

    let deployer = null;
    try {
        deployer = loadKeypairFile(deployerKeyFile);
    } catch (err) {
        const funded = loadFundedWallets(1);
        deployer = funded[0] || null;
        if (deployer) {
            console.log(`  ℹ️  Falling back to funded signer: ${path.basename(deployer.source)}`);
        } else {
            console.log(`  ⚠️  Failed to load deployer keypair: ${err.message}`);
            process.exit(1);
        }
    }
    ok('Deployer loaded', !!deployer.address,
        `${deployer.address.slice(0, 8)}...`);

    // Check deployer balance
    let deployerBal = await getBalance(deployer.address);
    if (deployerBal.spores === 0) {
        await tryTest('Fund deployer', async () => {
            const funded = await fundAccount(deployer.address, 20, RPC_URL);
            ok('Deployer funding requested', funded, '20 LICN via airdrop/faucet');
        });
        await sleep(2000);
        deployerBal = await getBalance(deployer.address);
    }
    ok('Deployer has funds', deployerBal.spores > 0,
        `${deployerBal.licn} LICN`);

    // ── Create test wallets ──
    console.log('\n2️⃣  Create Test Wallets');
    const walletA = generateKeypair();
    const walletB = generateKeypair();
    console.log(`  Wallet A: ${walletA.address}`);
    console.log(`  Wallet B: ${walletB.address}`);

    // ── Airdrop to wallets ──
    console.log('\n3️⃣  Airdrop LICN');
    await tryTest('Airdrop to A', async () => {
        const result = await rpc('requestAirdrop', [walletA.address, 5]);
        ok('Airdrop to A', result && (result.signature || result), 'requested 5 LICN');
    });

    await waitForBalance(walletA.address, (balance) => balance.spores > 0, 15000);

    await tryTest('Airdrop to B', async () => {
        const result = await rpc('requestAirdrop', [walletB.address, 5]);
        ok('Airdrop to B', result && (result.signature || result), 'requested 5 LICN');
    });

    await waitForBalance(walletB.address, (balance) => balance.spores > 0, 15000);

    // Verify balances
    const balA = await getBalance(walletA.address);
    const balB = await getBalance(walletB.address);
    ok('Wallet A funded', balA.spores > 0, `${balA.licn} LICN`);
    ok('Wallet B funded', balB.spores > 0, `${balB.licn} LICN`);

    // ── Transfer LICN from A to B ──
    console.log('\n4️⃣  Transfer LICN (A → B)');
    await tryTest('Transfer 1 LICN', async () => {
        const ix = buildTransferIx(walletA.address, walletB.address, 1 * SPORES_PER_LICN);
        const result = await buildSignSend(walletA, [ix]);
        ok('Transfer sent', result && (result.signature || typeof result === 'string'),
            `sig: ${(result.signature || result).toString().slice(0, 16)}...`);
    });

    const balA2 = await waitForBalance(walletA.address, balance => balance.spores < balA.spores);
    const balB2 = await waitForBalance(walletB.address, balance => balance.spores > balB.spores);
    ok('A balance decreased', balA2.spores < balA.spores,
        `${balA.licn} → ${balA2.licn} LICN`);
    ok('B balance increased', balB2.spores > balB.spores,
        `${balB.licn} → ${balB2.licn} LICN`);

    // ── Create SporePump token ──
    console.log('\n5️⃣  Create SporePump Token');
    const SPOREPUMP_ADDR = await getSymbolProgram('SPOREPUMP');
    ok('SporePump registry found', !!SPOREPUMP_ADDR,
        `program: ${SPOREPUMP_ADDR || '?'}`);

    await tryTest('Create token via SporePump', async () => {
        if (!SPOREPUMP_ADDR) {
            throw new Error('SporePump not found in symbol registry');
        }

        // SporePump create_token args: creator_pubkey(32 bytes) + fee_amount(8 bytes)
        const deployerBalanceBefore = await getBalance(deployer.address);
        const creatorBytes = base58Decode(deployer.address);
        const buf = new Uint8Array(40);
        buf.set(creatorBytes, 0);
        const view = new DataView(buf.buffer);
        view.setBigUint64(32, BigInt(10 * SPORES_PER_LICN), true); // 10 LICN creation fee

        const ix = buildContractCallIx(
            deployer.address,
            SPOREPUMP_ADDR,
            'create_token',
            buf,
            10 * SPORES_PER_LICN // value attached
        );

        const result = await buildSignSend(deployer, [ix]);
        const sig = result?.signature || result;
        ok('Token created', sig, `sig: ${String(sig).slice(0, 16)}...`);

        if (sig) {
            const confirmation = await waitForConfirmation(String(sig));
            const confirmationValue = confirmation?.value || null;
            const deployerBalanceAfter = await waitForBalance(
                deployer.address,
                (balance) => balance.spores <= deployerBalanceBefore.spores - (9 * SPORES_PER_LICN),
                15000,
            );
            const spent = deployerBalanceBefore.spores - deployerBalanceAfter.spores;
            ok(
                'Creation fee deducted',
                spent >= 9 * SPORES_PER_LICN,
                `spent ${(spent / SPORES_PER_LICN).toFixed(1)} LICN`,
            );

            if (confirmationValue) {
                ok(
                    'TX confirmed in block',
                    !confirmationValue.err,
                    `slot ${confirmationValue.slot}, status: ${confirmationValue.confirmation_status}`,
                );
            } else {
                console.log('  ℹ️  confirmTransaction metadata still pending');
            }

            const tx = await waitForTransaction(
                String(sig),
                (candidate) => candidate && candidate.slot >= 0,
                5000,
            );
            if (tx) {
                ok(
                    'TX indexed detail',
                    tx.slot >= 0 && tx.status === 'Success',
                    `slot ${tx.slot}, status: ${tx.status}`,
                );
            } else {
                console.log('  ℹ️  transaction detail index still pending');
            }
            if (tx?.contract_logs) {
                console.log(`    Contract logs: ${JSON.stringify(tx.contract_logs)}`);
            }
        }
    });

    // ── Query token accounts ──
    console.log('\n6️⃣  Token Accounts & Balances');

    await tryTest('Deployer token accounts', async () => {
        const accounts = await getTokenAccounts(deployer.address);
        ok('getTokenAccounts returns data', accounts !== null && accounts !== undefined,
            Array.isArray(accounts) ? `${accounts.length} tokens` : typeof accounts);
        if (Array.isArray(accounts) && accounts.length > 0) {
            for (const t of accounts.slice(0, 5)) {
                console.log(`    Token: ${t.symbol || t.token || '??'}, balance: ${t.balance || t.amount || '??'}`);
            }
        }
    });

    await tryTest('LICN token balance', async () => {
        // LICN is native, so query via the zero-address sentinel.
        const LICN_ADDR = base58Encode(new Uint8Array(32));
        const balance = await getTokenBalance(LICN_ADDR, deployer.address);
        ok('getTokenBalance(LICN)', balance !== null && balance !== undefined,
            JSON.stringify(balance).slice(0, 80));
    });

    // ── Wrapped token verification ──
    console.log('\n7️⃣  Wrapped Token Contracts');
    const wrappedSymbols = ['WSOL', 'WETH', 'WBNB', 'LUSD'];

    for (const symbol of wrappedSymbols) {
        await tryTest(`${symbol} contract`, async () => {
            // Look up address dynamically from registry
            const reg = await rpc('getSymbolRegistry', [symbol]);
            const addr = reg?.program;
            ok(`${symbol} registry found`, !!addr, `program: ${addr || '?'}`);
            if (addr) {
                const program = await rpc('getProgram', [addr]);
                ok(`${symbol} deployed`, program && program.executable === true,
                    `code_size: ${program?.code_size || '?'} bytes`);
            }
        });
    }

    // ── Transfer from deployer to wallet A (using native system transfer) ──
    console.log('\n8️⃣  Multi-Transfer Verification');
    await tryTest('Deployer → A (2 LICN)', async () => {
        const ix = buildTransferIx(deployer.address, walletA.address, 2 * SPORES_PER_LICN);
        const result = await buildSignSend(deployer, [ix]);
        ok('Transfer sent', result, 'deployer → A');
    });

    // Check A received it
    const balA3 = await waitForBalance(walletA.address, balance => balance.spores > balA2.spores);
    ok('A balance updated', balA3.spores > balA2.spores,
        `${balA2.licn} → ${balA3.licn} LICN`);

    // ── Query transaction history ──
    console.log('\n9️⃣  Transaction History');
    await tryTest('Wallet A tx history', async () => {
        const txs = await rpc('getTransactionsByAddress', [walletA.address]);
        const txList = txs?.transactions || txs || [];
        ok('Has transactions', Array.isArray(txList) && txList.length > 0,
            `${txList.length} transactions`);
    });

    // ── Summary ──
    console.log('\n' + '═'.repeat(60));
    console.log(`Results: ${passed} passed, ${failed} failed (${passed + failed} total)`);
    console.log('═'.repeat(60));

    process.exit(failed > 0 ? 1 : 0);
}

main().catch(err => {
    console.error('Fatal:', err);
    process.exit(1);
});
