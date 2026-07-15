/**
 * Lichen DEX E2E Test Setup Helper
 *
 * Provides a complete "zero-to-trading" setup for E2E tests against LIVE
 * VPS/testnet deployments where real WASM contracts run real token operations.
 *
 * WHY THIS EXISTS:
 * Local test harness uses #[cfg(not(target_arch = "wasm32"))] stubs that
 * bypass token transfers/approvals. On VPS, real contracts require:
 *   1. LICN airdrop (native token for fees & buy-side orders)
 *   2. Wrapped token minting (lUSD, wSOL, etc.) by the genesis minter
 *   3. Token approvals (trader → DEX/AMM contract as spender)
 *   4. LichenID registration + reputation boost (for governance/prediction)
 *   5. Prediction market creation (requires reputation ≥ 500)
 *
 * USAGE:
 *   const { setupDexEnvironment } = require('./helpers/dex-setup');
 *   const env = await setupDexEnvironment({ rpcUrl, wallets, contracts, adminKeypair });
 *
 * The admin keypair is the genesis-primary key (= operational_token_admin = minter).
 */
'use strict';

const fs = require('fs');
const path = require('path');
const pq = require('./pq-node');
const { waitForSuccessfulTransaction } = require('./tx-receipt');
const { encodeNativeTransactionBase64, signNativeTransaction } = require('./tx-wire');
const {
    loadFundedWallets,
    findGenesisAdminKeypair,
    loadKeypairFile,
    bs58encode,
    bs58decode,
} = require('./funded-wallets');

const SPORES_PER_LICN = 1_000_000_000;
const AIRDROP_AMOUNT = 10; // LICN per round
const AIRDROP_COOLDOWN_MS = 61_000;
const TARGET_BALANCE_LICN = 100; // Minimum LICN each wallet needs

function loadFirstGenesisKeypairByPrefix(keysDir, prefix) {
    if (!keysDir || !fs.existsSync(keysDir)) return null;
    const file = fs.readdirSync(keysDir)
        .filter((name) => name.startsWith(prefix) && name.endsWith('.json'))
        .sort()[0];
    if (!file) return null;
    return loadKeypairFile(path.join(keysDir, file));
}

function findGenesisKeypairByPrefix(prefix, anchorKeypair = null) {
    const tried = [];
    const roots = [process.cwd(), path.resolve(process.cwd(), '..')];
    const anchorSource = anchorKeypair && anchorKeypair.source;
    if (anchorSource) {
        tried.push(path.dirname(anchorSource));
    }

    for (const root of roots) {
        tried.push(path.join(root, 'artifacts', 'testnet', 'genesis-keys'));
        const dataDir = path.join(root, 'data');
        if (fs.existsSync(dataDir)) {
            for (const stateDir of fs.readdirSync(dataDir).filter((name) => name.startsWith('state-') || name.startsWith('matrix-sdk-state-')).sort()) {
                tried.push(path.join(dataDir, stateDir, 'genesis-keys'));
                tried.push(path.join(dataDir, stateDir, 'blockchain.db', 'genesis-keys'));
            }
        }
    }

    for (const keysDir of tried) {
        try {
            const keypair = loadFirstGenesisKeypairByPrefix(keysDir, prefix);
            if (keypair) return keypair;
        } catch (_) { }
    }
    return null;
}

// ═══════════════════════════════════════════════════════════════════════════════
// Binary helpers
// ═══════════════════════════════════════════════════════════════════════════════
function writeU64LE(view, off, n) {
    const value = typeof n === 'bigint' ? n : BigInt(Math.round(Number(n)));
    if (value < 0n || value > 0xffff_ffff_ffff_ffffn) {
        throw new RangeError(`u64 out of range: ${value}`);
    }
    view.setBigUint64(off, value, true);
}

function writePubkey(arr, off, addr) {
    const decoded = bs58decode(addr);
    arr.set(decoded.subarray(0, 32), off);
}

function pubkeyBytes(addr) {
    return bs58decode(addr).subarray(0, 32);
}

function u32LE(n) {
    const out = new Uint8Array(4);
    new DataView(out.buffer).setUint32(0, Number(n), true);
    return out;
}

function u64LE(n) {
    const out = new Uint8Array(8);
    writeU64LE(new DataView(out.buffer), 0, n);
    return out;
}

function padBytes(bytes, len) {
    const out = new Uint8Array(len);
    out.set(bytes.subarray(0, len), 0);
    return out;
}

function buildLayoutArgs(layout, chunks) {
    const header = new Uint8Array(1 + layout.length);
    header[0] = 0xAB;
    header.set(layout, 1);
    const total = chunks.reduce((sum, chunk) => sum + chunk.length, header.length);
    const out = new Uint8Array(total);
    out.set(header, 0);
    let off = header.length;
    for (const chunk of chunks) {
        out.set(chunk, off);
        off += chunk.length;
    }
    return out;
}

// ═══════════════════════════════════════════════════════════════════════════════
// Instruction builders (named exports for token contracts)
// ═══════════════════════════════════════════════════════════════════════════════
const CONTRACT_PID = bs58encode(new Uint8Array(32).fill(0xFF));

function namedCallIx(callerAddr, contractAddr, funcName, argsBytes, value = 0) {
    const data = JSON.stringify({ Call: { function: funcName, args: Array.from(argsBytes), value } });
    return { program_id: CONTRACT_PID, accounts: [callerAddr, contractAddr], data };
}

function contractIx(callerAddr, contractAddr, argsBytes, value = 0) {
    return namedCallIx(callerAddr, contractAddr, 'call', argsBytes, value);
}

/**
 * Build mint instruction: mint(caller, to, amount)
 * Token contracts use named export "mint" with 72-byte args:
 *   [0-31]: caller (32B)
 *   [32-63]: to (32B)
 *   [64-71]: amount (u64 LE, in spores)
 */
function buildMintArgs(callerAddr, toAddr, amountSpores) {
    const buf = new ArrayBuffer(72);
    const v = new DataView(buf);
    const a = new Uint8Array(buf);
    writePubkey(a, 0, callerAddr);
    writePubkey(a, 32, toAddr);
    writeU64LE(v, 64, amountSpores);
    return a;
}

/**
 * Build approve instruction: approve(owner, spender, amount)
 * Token contracts use named export "approve" with 72-byte args:
 *   [0-31]: owner (32B)
 *   [32-63]: spender (32B)
 *   [64-71]: amount (u64 LE, in spores)
 */
function buildApproveArgs(ownerAddr, spenderAddr, amountSpores) {
    const buf = new ArrayBuffer(72);
    const v = new DataView(buf);
    const a = new Uint8Array(buf);
    writePubkey(a, 0, ownerAddr);
    writePubkey(a, 32, spenderAddr);
    writeU64LE(v, 64, amountSpores);
    return a;
}

function buildDepositMarginInsuranceArgs(callerAddr, amountSpores) {
    const buf = new ArrayBuffer(41);
    const v = new DataView(buf);
    const a = new Uint8Array(buf);
    a[0] = 35;
    writePubkey(a, 1, callerAddr);
    writeU64LE(v, 33, amountSpores);
    return a;
}

/**
 * Build attest_reserves instruction:
 *   attest_reserves(caller, reserve_amount, proof_hash)
 *   [0-31]: caller (32B)
 *   [32-39]: reserve_amount (u64 LE)
 *   [40-71]: proof_hash (32B SHA256)
 */
function buildAttestReservesArgs(callerAddr, reserveAmount) {
    const buf = new ArrayBuffer(72);
    const v = new DataView(buf);
    const a = new Uint8Array(buf);
    writePubkey(a, 0, callerAddr);
    writeU64LE(v, 32, reserveAmount);
    // proof_hash: 32 bytes of zeros for testnet
    return a;
}

/**
 * Build LichenID register_identity:
 *   register_identity(owner, agent_type, name_ptr, name_len)
 * Uses runtime layout descriptor mode because this named export mixes pointer
 * and raw I32 params:
 *   [0xAB][32,1,64,4] + owner(32) + agent_type(1) + name(64 padded) + name_len(u32)
 */
function buildRegisterIdentityArgs(ownerAddr, agentType, name) {
    const nameBytes = Buffer.from(name, 'utf8');
    return buildLayoutArgs([0x20, 0x01, 0x40, 0x04], [
        pubkeyBytes(ownerAddr),
        new Uint8Array([agentType & 0xFF]),
        padBytes(nameBytes, 64),
        u32LE(nameBytes.length),
    ]);
}

/**
 * Build LichenID update_reputation_typed:
 *   update_reputation_typed(caller, target, contribution_type, count)
 * Uses runtime layout descriptor mode because contribution_type is a raw I32
 * while caller and target are pointers:
 *   [0xAB][32,32,1,8] + caller(32) + target(32) + contribution_type(1) + count(u64)
 */
function buildUpdateReputationArgs(adminAddr, targetAddr, contributionType, count) {
    return buildLayoutArgs([0x20, 0x20, 0x01, 0x08], [
        pubkeyBytes(adminAddr),
        pubkeyBytes(targetAddr),
        new Uint8Array([contributionType & 0xFF]),
        u64LE(count),
    ]);
}

/**
 * Build prediction market create_market:
 *   [0]: opcode 1
 *   [1-32]: creator (32B)
 *   [33]: category (1B)
 *   [34-41]: close_slot (u64 LE)
 *   [42]: outcome_count (1B)
 *   [43-74]: question_hash (32B SHA256)
 *   [75-78]: question_len (u32 LE)
 *   [79+]: question UTF-8
 */
function buildCreateMarketArgs(creatorAddr, category, closeSlot, outcomeCount, question) {
    const crypto = require('crypto');
    const questionBytes = Buffer.from(question, 'utf8');
    const questionHash = crypto.createHash('sha256').update(questionBytes).digest();
    const buf = new ArrayBuffer(79 + questionBytes.length);
    const v = new DataView(buf);
    const a = new Uint8Array(buf);
    a[0] = 1;
    writePubkey(a, 1, creatorAddr);
    a[33] = category;
    writeU64LE(v, 34, closeSlot);
    a[42] = outcomeCount;
    a.set(questionHash, 43);
    v.setUint32(75, questionBytes.length, true);
    a.set(questionBytes, 79);
    return a;
}

function buildPredictionInitialLiquidityArgs(providerAddr, marketId, amountLusd) {
    const buf = new ArrayBuffer(49);
    const v = new DataView(buf);
    const a = new Uint8Array(buf);
    a[0] = 2;
    writePubkey(a, 1, providerAddr);
    writeU64LE(v, 33, marketId);
    writeU64LE(v, 41, amountLusd);
    return a;
}

// ═══════════════════════════════════════════════════════════════════════════════
// RPC / Transaction helpers
// ═══════════════════════════════════════════════════════════════════════════════
let rpcIdCounter = 9000;

function hexToBytes(h) {
    const c = h.startsWith('0x') ? h.slice(2) : h;
    const o = new Uint8Array(c.length / 2);
    for (let i = 0; i < o.length; i++) o[i] = parseInt(c.slice(i * 2, i * 2 + 2), 16);
    return o;
}

function encodeMsg(instructions, blockhash, signer) {
    const parts = [];
    function pushU64(n) {
        const buf = new ArrayBuffer(8);
        const v = new DataView(buf);
        v.setUint32(0, n & 0xFFFFFFFF, true);
        v.setUint32(4, Math.floor(n / 0x100000000) & 0xFFFFFFFF, true);
        parts.push(new Uint8Array(buf));
    }
    pushU64(instructions.length);
    for (const ix of instructions) {
        parts.push(bs58decode(ix.program_id));
        const accts = ix.accounts || [signer];
        pushU64(accts.length);
        for (const a of accts) parts.push(bs58decode(a));
        const d = typeof ix.data === 'string' ? new TextEncoder().encode(ix.data) : new Uint8Array(ix.data);
        pushU64(d.length);
        parts.push(d);
    }
    parts.push(hexToBytes(blockhash));
    parts.push(new Uint8Array([0x00])); // compute_budget: None
    parts.push(new Uint8Array([0x00])); // compute_unit_price: None
    const total = parts.reduce((s, a) => s + a.length, 0);
    const out = new Uint8Array(total);
    let off = 0;
    for (const a of parts) { out.set(a, off); off += a.length; }
    return out;
}

async function rpcCall(rpcUrl, method, params = []) {
    const res = await fetch(rpcUrl, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ jsonrpc: '2.0', id: rpcIdCounter++, method, params }),
    });
    const json = await res.json();
    if (json.error) throw new Error(`RPC ${json.error.code}: ${json.error.message}`);
    return json.result;
}

async function sendSetupTx(rpcUrl, keypair, instructions, options = {}) {
    const bhRes = await rpcCall(rpcUrl, 'getRecentBlockhash');
    const bh = typeof bhRes === 'string' ? bhRes : bhRes.blockhash;
    const nix = instructions.map(ix => ({
        program_id: ix.program_id,
        accounts: ix.accounts || [keypair.address],
        data: typeof ix.data === 'string' ? Array.from(new TextEncoder().encode(ix.data)) : Array.from(ix.data),
    }));
    const msg = encodeMsg(nix, bh, keypair.address);
    const sig = signNativeTransaction(pq, keypair, msg);
    const b64 = encodeNativeTransactionBase64(
        [sig],
        { instructions: nix, blockhash: bh },
    );
    const sim = await rpcCall(rpcUrl, 'simulateTransaction', [b64]);
    const returnCode = sim?.returnCode === null || sim?.returnCode === undefined ? 0 : Number(sim.returnCode);
    const returnValue = readU64LEFromBase64(sim?.returnData || sim?.return_data || '');
    if (!sim?.success || (options.requireReturnValue ? returnValue === 0n : returnCode !== 0)) {
        const logs = Array.isArray(sim?.logs) ? sim.logs.slice(-4).join(' | ') : '';
        const reason = sim?.error || `contract returnCode=${returnCode}`;
        throw new Error(`${reason}${logs ? ` (${logs})` : ''}`);
    }

    const signature = await rpcCall(rpcUrl, 'sendTransaction', [b64]);
    await waitForSuccessfulTransaction(
        (method, params) => rpcCall(rpcUrl, method, params),
        signature,
        60_000,
        250,
    );
    return options.returnResult ? { signature, returnValue } : signature;
}

const sleep = ms => new Promise(r => setTimeout(r, ms));

function readU64LEFromBase64(value) {
    if (!value || typeof value !== 'string') return 0n;
    const bytes = Buffer.from(value, 'base64');
    if (bytes.length < 8) return 0n;
    return bytes.readBigUInt64LE(0);
}

function allowanceArgs(ownerAddr, spenderAddr) {
    const args = new Uint8Array(64);
    args.set(pubkeyBytes(ownerAddr), 0);
    args.set(pubkeyBytes(spenderAddr), 32);
    return Buffer.from(args).toString('base64');
}

async function getTokenBalanceRaw(rpcUrl, tokenAddr, holderAddr) {
    const balance = await rpcCall(rpcUrl, 'getTokenBalance', [tokenAddr, holderAddr]);
    return BigInt(balance?.balance || 0);
}

async function getTokenAllowanceRaw(rpcUrl, tokenAddr, ownerAddr, spenderAddr) {
    const result = await rpcCall(rpcUrl, 'callContract', [
        tokenAddr,
        'allowance',
        allowanceArgs(ownerAddr, spenderAddr),
        ownerAddr,
    ]);
    if (!result?.success && result?.returnData === undefined) {
        throw new Error(result?.error || 'allowance read failed');
    }
    return readU64LEFromBase64(result.returnData);
}

async function waitForTokenBalance(rpcUrl, tokenAddr, holderAddr, minBalance, timeoutMs = 30000) {
    const started = Date.now();
    const min = typeof minBalance === 'bigint' ? minBalance : BigInt(minBalance);
    let last = 0n;
    while (Date.now() - started < timeoutMs) {
        last = await getTokenBalanceRaw(rpcUrl, tokenAddr, holderAddr);
        if (last >= min) return last;
        await sleep(500);
    }
    throw new Error(`token balance did not reach ${min}; last=${last}`);
}

async function waitForTokenAllowance(rpcUrl, tokenAddr, ownerAddr, spenderAddr, minAllowance, timeoutMs = 30000) {
    const started = Date.now();
    const min = typeof minAllowance === 'bigint' ? minAllowance : BigInt(minAllowance);
    let last = 0n;
    while (Date.now() - started < timeoutMs) {
        last = await getTokenAllowanceRaw(rpcUrl, tokenAddr, ownerAddr, spenderAddr);
        if (last >= min) return last;
        await sleep(500);
    }
    throw new Error(`token allowance did not reach ${min}; last=${last}`);
}

function rawTokenAmount(value) {
    if (typeof value === 'bigint') return value;
    const numeric = Number(value || 0);
    if (!Number.isFinite(numeric) || numeric <= 0) return 0n;
    return BigInt(Math.round(numeric * SPORES_PER_LICN));
}

function u64FromHex(value) {
    if (!value || typeof value !== 'string' || value.length < 16) return 0n;
    return Buffer.from(value.slice(0, 16), 'hex').readBigUInt64LE(0);
}

function pubkeyFromStorageHex(value) {
    if (!value || typeof value !== 'string' || value.length < 64) return null;
    return bs58encode(Buffer.from(value.slice(0, 64), 'hex'));
}

function findLoadedKeypairByAddress(address) {
    if (!address) return null;
    return loadFundedWallets(64).find((wallet) => wallet.address === address) || null;
}

async function getMarginInsuranceRaw(rpcUrl, dexMarginAddr) {
    const storage = await rpcCall(rpcUrl, 'getProgramStorage', [dexMarginAddr, { limit: 1000 }]);
    const entry = (storage.entries || []).find((row) => row.key_decoded === 'mrg_insurance');
    return u64FromHex(entry?.value_hex || entry?.value || '');
}

async function waitForMarginInsurance(rpcUrl, dexMarginAddr, minInsurance, timeoutMs = 30000) {
    const started = Date.now();
    const min = typeof minInsurance === 'bigint' ? minInsurance : BigInt(minInsurance);
    let last = 0n;
    while (Date.now() - started < timeoutMs) {
        last = await getMarginInsuranceRaw(rpcUrl, dexMarginAddr);
        if (last >= min) return last;
        await sleep(500);
    }
    throw new Error(`margin insurance did not reach ${min}; current=${last}`);
}

async function findMarginAdminKeypair(rpcUrl, contracts, fallbackKeypair = null) {
    const dexMarginAddr = contracts.dex_margin;
    if (!dexMarginAddr) return fallbackKeypair;
    try {
        const storage = await rpcCall(rpcUrl, 'getProgramStorage', [dexMarginAddr, { limit: 500 }]);
        const adminEntry = (storage.entries || []).find((row) => row.key_decoded === 'mrg_admin');
        const adminAddress = pubkeyFromStorageHex(adminEntry?.value_hex || adminEntry?.value || '');
        const loaded = findLoadedKeypairByAddress(adminAddress);
        if (loaded) return loaded;
        if (fallbackKeypair?.address === adminAddress) return fallbackKeypair;
    } catch (_) { }
    return findGenesisKeypairByPrefix('community_treasury', fallbackKeypair) || fallbackKeypair;
}

async function bootstrapMarginInsurance(rpcUrl, adminKeypair, contracts, amountLusd, options = {}) {
    const dexMarginAddr = contracts.dex_margin;
    const lusdAddr = contracts.lusd_token;
    const amountRaw = rawTokenAmount(amountLusd);
    const summary = { attempted: amountRaw > 0n, succeeded: false, amountRaw: amountRaw.toString(), error: null };
    if (amountRaw <= 0n) return summary;
    if (!adminKeypair) {
        summary.error = 'missing lUSD minter/admin keypair';
        if (options.require) throw new Error(summary.error);
        return summary;
    }
    if (!dexMarginAddr || !lusdAddr) {
        summary.error = 'dex_margin or lUSD contract missing';
        if (options.require) throw new Error(summary.error);
        return summary;
    }

    const current = await getMarginInsuranceRaw(rpcUrl, dexMarginAddr).catch(() => 0n);
    if (current >= amountRaw) {
        summary.succeeded = true;
        summary.currentRaw = current.toString();
        return summary;
    }

    const topUp = amountRaw - current;
    const contributorKeypair = options.contributorKeypair || adminKeypair;
    if (!contributorKeypair) {
        summary.error = 'missing dex_margin insurance contributor keypair';
        if (options.require) throw new Error(summary.error);
        return summary;
    }

    await fundWalletsWithLicn(rpcUrl, [contributorKeypair], 10);

    await sendSetupTx(rpcUrl, adminKeypair, [
        namedCallIx(adminKeypair.address, lusdAddr, 'mint', buildMintArgs(adminKeypair.address, contributorKeypair.address, topUp)),
    ]);
    await waitForTokenBalance(rpcUrl, lusdAddr, contributorKeypair.address, topUp);

    await sendSetupTx(rpcUrl, contributorKeypair, [
        namedCallIx(
            contributorKeypair.address,
            lusdAddr,
            'approve',
            buildApproveArgs(contributorKeypair.address, dexMarginAddr, topUp),
        ),
    ]);
    await waitForTokenAllowance(rpcUrl, lusdAddr, contributorKeypair.address, dexMarginAddr, topUp);

    await sendSetupTx(rpcUrl, contributorKeypair, [
        contractIx(
            contributorKeypair.address,
            dexMarginAddr,
            buildDepositMarginInsuranceArgs(contributorKeypair.address, topUp),
        ),
    ]);

    let updated = 0n;
    try {
        updated = await waitForMarginInsurance(rpcUrl, dexMarginAddr, amountRaw);
    } catch (e) {
        summary.error = e.message;
        if (options.require) throw new Error(summary.error);
        return summary;
    }
    summary.succeeded = true;
    summary.currentRaw = updated.toString();
    summary.contributor = contributorKeypair.address;
    return summary;
}

async function getSpendableLicn(rpcUrl, address) {
    const balance = await rpcCall(rpcUrl, 'getBalance', [address]);
    return Number(balance?.spendable || 0) / SPORES_PER_LICN;
}

function isRateLimitError(error) {
    return String(error?.message || '').toLowerCase().includes('rate limit');
}

// ═══════════════════════════════════════════════════════════════════════════════
// Setup phases
// ═══════════════════════════════════════════════════════════════════════════════

/**
 * Phase 1: Airdrop LICN to all wallets.
 * Handles 60s rate limit per address by batching with delays.
 */
async function fundWalletsWithLicn(rpcUrl, wallets, targetLicn = TARGET_BALANCE_LICN) {
    const results = {};
    const targetThreshold = targetLicn * 0.9;

    for (const w of wallets) {
        let currentLicn = await getSpendableLicn(rpcUrl, w.address);
        results[w.address] = currentLicn;
        if (currentLicn >= targetThreshold) {
            console.log(`    ✓ ${w.address.slice(0, 12)}... already has ${currentLicn.toFixed(1)} LICN`);
            continue;
        }

        const roundsNeeded = Math.ceil((targetLicn - currentLicn) / AIRDROP_AMOUNT);
        console.log(`    ⟳ ${w.address.slice(0, 12)}... needs ${roundsNeeded} airdrop rounds (${currentLicn.toFixed(1)} → ${targetLicn} LICN)`);

        let attempts = 0;
        while (currentLicn < targetThreshold && attempts < 12) {
            const requestAmount = Math.min(AIRDROP_AMOUNT, Math.max(1, Math.ceil(targetLicn - currentLicn)));
            attempts += 1;
            try {
                await rpcCall(rpcUrl, 'requestAirdrop', [w.address, requestAmount]);
            } catch (e) {
                if (isRateLimitError(e)) {
                    console.log(`    ⏳ Rate limited, waiting 61s...`);
                    await sleep(AIRDROP_COOLDOWN_MS);
                    continue;
                }
                console.error(`    ✗ Airdrop failed for ${w.address.slice(0, 12)}...: ${e.message}`);
                break;
            }

            await sleep(1500);
            currentLicn = await getSpendableLicn(rpcUrl, w.address);
            results[w.address] = currentLicn;
        }

        if (currentLicn >= targetThreshold) {
            console.log(`    ✓ ${w.address.slice(0, 12)}... funded to ${currentLicn.toFixed(1)} LICN`);
        }
    }
    return results;
}

/**
 * Phase 2: Mint wrapped tokens to wallets using the genesis admin (minter) keypair.
 * The minter is the genesis-primary key = operational_token_admin.
 * Before minting: must attest reserves if bootstrap is complete.
 */
async function mintWrappedTokens(rpcUrl, adminKeypair, wallets, contracts) {
    const tokens = [
        { key: 'lusd_token', symbol: 'lUSD', amount: 10_000 * SPORES_PER_LICN },
        { key: 'wsol_token', symbol: 'wSOL', amount: 100 * SPORES_PER_LICN },
        { key: 'weth_token', symbol: 'wETH', amount: 10 * SPORES_PER_LICN },
        { key: 'wbnb_token', symbol: 'wBNB', amount: 100 * SPORES_PER_LICN },
        { key: 'wneo_token', symbol: 'wNEO', amount: 100 * SPORES_PER_LICN },
        { key: 'wgas_token', symbol: 'wGAS', amount: 1_000 * SPORES_PER_LICN },
        { key: 'wbtc_token', symbol: 'wBTC', amount: 2 * SPORES_PER_LICN },
    ];
    const summary = { attempted: 0, succeeded: 0, failed: 0, errors: [] };

    for (const token of tokens) {
        const contractAddr = contracts[token.key];
        if (!contractAddr) {
            const message = `${token.symbol} contract not found`;
            summary.failed += wallets.length || 1;
            summary.errors.push(message);
            console.log(`    ✗ ${message}`);
            continue;
        }

        // Attest reserves first (needed for minting circuit breaker)
        try {
            const attestArgs = buildAttestReservesArgs(adminKeypair.address, 1_000_000_000 * SPORES_PER_LICN);
            await sendSetupTx(rpcUrl, adminKeypair, [
                namedCallIx(adminKeypair.address, contractAddr, 'attest_reserves', attestArgs),
            ]);
            console.log(`    ✓ ${token.symbol} reserves attested`);
        } catch (e) {
            // May fail if already attested or if bootstrap not complete (minting still works)
            console.log(`    ⚠ ${token.symbol} attest_reserves: ${e.message.slice(0, 80)}`);
        }
        await sleep(1500);

        // Mint tokens to each wallet
        for (const w of wallets) {
            summary.attempted += 1;
            try {
                const mintArgs = buildMintArgs(adminKeypair.address, w.address, token.amount);
                await sendSetupTx(rpcUrl, adminKeypair, [
                    namedCallIx(adminKeypair.address, contractAddr, 'mint', mintArgs),
                ]);
                await waitForTokenBalance(rpcUrl, contractAddr, w.address, BigInt(token.amount));
                summary.succeeded += 1;
                console.log(`    ✓ Minted ${token.amount / SPORES_PER_LICN} ${token.symbol} → ${w.address.slice(0, 12)}...`);
            } catch (e) {
                summary.failed += 1;
                const message = `Mint ${token.symbol} → ${w.address.slice(0, 12)}...: ${e.message}`;
                summary.errors.push(message);
                console.log(`    ✗ ${message.slice(0, 120)}`);
            }
            await sleep(1000);
        }
    }
    return summary;
}

/**
 * Phase 3: Approve DEX contracts as token spenders.
 * Margin uses lUSD transfer_from collateral custody, so dex_margin is included.
 */
async function approveTokenSpenders(rpcUrl, wallets, contracts) {
    const tokens = ['lusd_token', 'wsol_token', 'weth_token', 'wbnb_token', 'wneo_token', 'wgas_token', 'wbtc_token'];
    const spenders = ['dex_core', 'dex_amm', 'dex_margin', 'prediction_market'];
    const MAX_ALLOWANCE = 0xffff_ffff_ffff_ffffn; // u64::MAX
    const WNEO_ALLOWANCE = 1_000_000n * BigInt(SPORES_PER_LICN); // wNEO approvals must be whole NEO lots.
    const summary = { attempted: 0, succeeded: 0, failed: 0, errors: [] };

    for (const w of wallets) {
        for (const tokenKey of tokens) {
            const tokenAddr = contracts[tokenKey];
            if (!tokenAddr) continue;
            for (const spenderKey of spenders) {
                const spenderAddr = contracts[spenderKey];
                if (!spenderAddr) continue;
                summary.attempted += 1;
                try {
                    const allowance = tokenKey === 'wneo_token' ? WNEO_ALLOWANCE : MAX_ALLOWANCE;
                    const approveArgs = buildApproveArgs(w.address, spenderAddr, allowance);
                    await sendSetupTx(rpcUrl, w, [
                        namedCallIx(w.address, tokenAddr, 'approve', approveArgs),
                    ]);
                    await waitForTokenAllowance(rpcUrl, tokenAddr, w.address, spenderAddr, allowance);
                    summary.succeeded += 1;
                    console.log(`    ✓ ${w.address.slice(0, 8)}... approved ${spenderKey} for ${tokenKey}`);
                } catch (e) {
                    summary.failed += 1;
                    const message = `Approve ${tokenKey}→${spenderKey} for ${w.address.slice(0, 8)}...: ${e.message}`;
                    summary.errors.push(message);
                    console.log(`    ✗ ${message.slice(0, 120)}`);
                }
                await sleep(500);
            }
        }
    }
    return summary;
}

/**
 * Phase 4: Register LichenID identities and boost reputation for governance/prediction.
 * Requires admin keypair (LichenID admin = governance_authority).
 */
async function setupIdentities(rpcUrl, adminKeypair, wallets, contracts) {
    const lichenidAddr = contracts.lichenid;
    if (!lichenidAddr) {
        console.log('    ⚠ LichenID contract not found, skipping identity setup');
        return;
    }

    // The LichenID admin is governance_authority (different from token admin).
    // Try finding it from genesis keys.
    for (const [i, w] of wallets.entries()) {
        const name = ['Alice', 'Bob', 'Charlie'][i] || `Trader${i}`;

        // Register identity
        try {
            const regArgs = buildRegisterIdentityArgs(w.address, 1, `E2E-${name}`);
            await sendSetupTx(rpcUrl, w, [
                namedCallIx(w.address, lichenidAddr, 'register_identity', regArgs),
            ]);
            console.log(`    ✓ Registered LichenID: ${name}`);
        } catch (e) {
            const identity = await rpcCall(rpcUrl, 'getLichenIdIdentity', [w.address]).catch(() => null);
            if (!/returnCode=3|failure code 3/i.test(String(e.message)) || !identity?.is_active) {
                throw new Error(`LichenID registration failed for ${name}: ${e.message}`);
            }
            console.log(`    ✓ Existing LichenID verified: ${name}`);
        }
        await sleep(500);

        const reputation = await rpcCall(rpcUrl, 'getLichenIdReputation', [w.address]);
        const score = Number(reputation?.score ?? reputation?.reputation ?? 0);
        if (score < 500) {
            throw new Error(
                `LichenID reputation is below the 500 eligibility threshold for ${name}: ${score}; `
                + 'reputation must be earned or granted through the governed action flow',
            );
        }
        console.log(`    ✓ Reputation verified for ${name} (${score})`);
    }
}

/**
 * Phase 5: Create prediction markets for testing.
 */
async function createPredictionMarkets(rpcUrl, creatorKeypair, contracts) {
    const predAddr = contracts.prediction_market;
    if (!predAddr) {
        console.log('    ⚠ Prediction market contract not found, skipping');
        return;
    }

    // Get current slot for close_slot calculation
    const currentSlot = await rpcCall(rpcUrl, 'getSlot');
    const closeSlot = currentSlot + 100000; // ~22 hours at 800ms blocks

    const runId = `${Date.now()}-${creatorKeypair.address.slice(0, 8)}`;
    const markets = [
        { category: 2, question: `Will LICN reach $1? [${runId}-1]`, outcomes: 2 },
        { category: 0, question: `Will validator count exceed 100? [${runId}-2]`, outcomes: 2 },
        { category: 2, question: `Will DEX volume exceed 10M LICN? [${runId}-3]`, outcomes: 3 },
    ];

    const created = [];
    for (const market of markets) {
        const args = buildCreateMarketArgs(
            creatorKeypair.address,
            market.category,
            closeSlot,
            market.outcomes,
            market.question,
        );
        const result = await sendSetupTx(rpcUrl, creatorKeypair, [
            contractIx(creatorKeypair.address, predAddr, args),
        ], { requireReturnValue: true, returnResult: true });
        const marketId = Number(result.returnValue);
        const liquidityArgs = buildPredictionInitialLiquidityArgs(
            creatorKeypair.address,
            marketId,
            100 * SPORES_PER_LICN,
        );
        await sendSetupTx(rpcUrl, creatorKeypair, [
            contractIx(creatorKeypair.address, predAddr, liquidityArgs),
        ], { requireReturnValue: true });
        const marketState = await rpcCall(rpcUrl, 'getPredictionMarket', [marketId]);
        const status = String(marketState?.status || '').toLowerCase();
        if (!['active', 'open', '1'].includes(status)) {
            throw new Error(`prediction market ${marketId} did not activate (status=${status || 'missing'})`);
        }
        created.push(marketId);
        console.log(`    ✓ Created and activated market ${marketId}: "${market.question.slice(0, 50)}"`);
        await sleep(1500);
    }
    return created;
}

// ═══════════════════════════════════════════════════════════════════════════════
// Main orchestrator
// ═══════════════════════════════════════════════════════════════════════════════

/**
 * Complete E2E environment setup. Call once before test suite runs.
 *
 * @param {Object} opts
 * @param {string} opts.rpcUrl - RPC endpoint URL
 * @param {Array} opts.wallets - Array of {address, seed, publicKey} keypair objects
 * @param {Object} opts.contracts - Map of contract_name → address (from discoverContracts)
 * @param {Object} [opts.adminKeypair] - Genesis primary keypair (minter/admin). Auto-discovered if omitted.
 * @param {boolean} [opts.skipFunding=false] - Skip LICN airdrop phase
 * @param {boolean} [opts.skipMinting=false] - Skip wrapped token minting
 * @param {boolean} [opts.skipApprovals=false] - Skip token approvals
 * @param {boolean} [opts.skipIdentities=false] - Skip LichenID setup
 * @param {boolean} [opts.skipPrediction=false] - Skip prediction market creation
 * @param {boolean} [opts.requireMinting=false] - Fail if any wrapped-token mint fails
 * @param {boolean} [opts.requireApprovals=false] - Fail if any token approval fails
 * @param {number} [opts.marginInsuranceLusd=0] - Minimum dex_margin insurance fund to bootstrap
 * @param {boolean} [opts.requireMarginInsurance=false] - Fail if margin insurance bootstrap fails
 * @param {number} [opts.targetLicn=100] - Target LICN balance per wallet
 * @returns {Object} Setup results summary
 */
async function setupDexEnvironment(opts) {
    const {
        rpcUrl,
        wallets,
        contracts,
        skipFunding = false,
        skipMinting = false,
        skipApprovals = false,
        skipIdentities = false,
        skipPrediction = false,
        requireMinting = false,
        requireApprovals = false,
        marginInsuranceLusd = 0,
        requireMarginInsurance = false,
        targetLicn = TARGET_BALANCE_LICN,
    } = opts;

    let adminKeypair = opts.adminKeypair;

    // Auto-discover admin keypair if not provided.
    // When running against VPS testnet, prefer state-testnet keys (encrypted,
    // require LICHEN_KEYPAIR_PASSWORD) over stale local state-7001 keys.
    if (!adminKeypair) {
        const rpcLower = (rpcUrl || '').toLowerCase();
        if (rpcLower.includes('testnet') || rpcLower.includes('15.204.229.189') || rpcLower.includes('37.59.97.61') || rpcLower.includes('15.235.142.253')) {
            const path = require('path');
            const fw = require('./funded-wallets');
            const testnetKeyDir = path.resolve(__dirname, '../../data/state-testnet/genesis-keys');
            try {
                adminKeypair = fw.loadKeypairFile(path.join(testnetKeyDir, 'genesis-primary-lichen-testnet-1.json'));
                console.log(`    ✓ Admin keypair loaded from state-testnet: ${adminKeypair.address.slice(0, 16)}...`);
            } catch (e) {
                console.log(`    ⚠ Cannot load state-testnet admin key: ${e.message.slice(0, 80)}`);
                console.log('      Set LICHEN_KEYPAIR_PASSWORD to decrypt VPS genesis keys');
            }
        }
        if (!adminKeypair) {
            adminKeypair = findGenesisAdminKeypair();
        }
        if (!adminKeypair) {
            console.log('  ⚠ No genesis admin keypair found — skipping admin-only setup');
            console.log('    (mint, attest, reputation boost, prediction markets will be skipped)');
        }
    }
    if (requireMinting && !adminKeypair) {
        throw new Error('wrapped-token minting required but no genesis admin/minter keypair was available');
    }
    let lichenIdAdminKeypair = opts.lichenIdAdminKeypair || null;
    if (!skipIdentities && !lichenIdAdminKeypair) {
        // Genesis initializes LichenID with the governance authority. In testnet
        // genesis that authority is community_treasury, while token minting uses
        // genesis-primary. Keep the roles separate so identity reputation setup
        // exercises the same admin path as production.
        lichenIdAdminKeypair = findGenesisKeypairByPrefix('community_treasury', adminKeypair);
        if (!lichenIdAdminKeypair && adminKeypair) {
            lichenIdAdminKeypair = adminKeypair;
        }
        if (lichenIdAdminKeypair) {
            console.log(`    ✓ LichenID admin keypair: ${lichenIdAdminKeypair.address.slice(0, 16)}...`);
        }
    }

    const summary = { phases: {} };

    // Phase 1: Fund wallets with LICN (include authorities if they need fees)
    if (!skipFunding) {
        console.log('\n── Setup Phase 1: LICN Funding ──');
        // Fund admin with just enough LICN for fees (~20 txs × 0.001 = 0.02 LICN, 10 LICN is plenty)
        if (adminKeypair && !wallets.some(w => w.address === adminKeypair.address)) {
            await fundWalletsWithLicn(rpcUrl, [adminKeypair], 10);
        }
        if (
            lichenIdAdminKeypair
            && lichenIdAdminKeypair.address !== adminKeypair?.address
            && !wallets.some(w => w.address === lichenIdAdminKeypair.address)
        ) {
            await fundWalletsWithLicn(rpcUrl, [lichenIdAdminKeypair], 10);
        }
        summary.phases.funding = await fundWalletsWithLicn(rpcUrl, wallets, targetLicn);
        await sleep(2000);
    }

    // Phase 2: Mint wrapped tokens (requires admin)
    if (!skipMinting && adminKeypair) {
        console.log('\n── Setup Phase 2: Mint Wrapped Tokens ──');
        const minting = await mintWrappedTokens(rpcUrl, adminKeypair, wallets, contracts);
        summary.phases.minting = minting;
        if (requireMinting && (minting.failed > 0 || minting.succeeded !== minting.attempted)) {
            throw new Error(`wrapped-token minting incomplete: ${minting.succeeded}/${minting.attempted} succeeded; ${minting.errors.slice(0, 3).join('; ')}`);
        }
        await sleep(2000);
    }

    // Phase 3: Approve DEX/AMM as spenders
    if (!skipApprovals) {
        console.log('\n── Setup Phase 3: Token Approvals ──');
        const approvals = await approveTokenSpenders(rpcUrl, wallets, contracts);
        summary.phases.approvals = approvals;
        if (requireApprovals && (approvals.failed > 0 || approvals.succeeded !== approvals.attempted)) {
            throw new Error(`token approvals incomplete: ${approvals.succeeded}/${approvals.attempted} succeeded; ${approvals.errors.slice(0, 3).join('; ')}`);
        }
        await sleep(2000);
    }

    if (marginInsuranceLusd > 0) {
        console.log('\n── Setup Phase 3b: Margin Insurance ──');
        const insurance = await bootstrapMarginInsurance(
            rpcUrl,
            adminKeypair,
            contracts,
            marginInsuranceLusd,
            { require: requireMarginInsurance },
        );
        summary.phases.marginInsurance = insurance;
        if (insurance.succeeded) {
            console.log(`    ✓ dex_margin insurance fund ≥ ${marginInsuranceLusd} lUSD`);
        } else if (insurance.error) {
            console.log(`    ⚠ dex_margin insurance bootstrap skipped: ${insurance.error}`);
        }
        await sleep(2000);
    }

    // Phase 4: LichenID identities + reputation (requires LichenID admin)
    if (!skipIdentities && lichenIdAdminKeypair) {
        console.log('\n── Setup Phase 4: LichenID Identities ──');
        await setupIdentities(rpcUrl, lichenIdAdminKeypair, wallets, contracts);
        summary.phases.identities = true;
        await sleep(2000);
    }
    if (!skipIdentities && !lichenIdAdminKeypair) {
        throw new Error('LichenID setup required but no LichenID governance admin keypair was available');
    }

    // Phase 5: Create prediction markets (requires reputation)
    if (!skipPrediction && adminKeypair && wallets.length > 0) {
        console.log('\n── Setup Phase 5: Prediction Markets ──');
        summary.phases.prediction = await createPredictionMarkets(rpcUrl, wallets[0], contracts);
        await sleep(2000);
    }

    // Final balance check
    console.log('\n── Setup Complete: Final Balances ──');
    for (const w of wallets) {
        try {
            const bal = await rpcCall(rpcUrl, 'getBalance', [w.address]);
            console.log(`    ${w.address.slice(0, 12)}... : ${bal.spendable_licn} LICN`);
        } catch { }
    }

    return summary;
}

module.exports = {
    setupDexEnvironment,
    fundWalletsWithLicn,
    mintWrappedTokens,
    approveTokenSpenders,
    setupIdentities,
    createPredictionMarkets,
    namedCallIx,
    contractIx,
    sendSetupTx,
    rpcCall,
    buildMintArgs,
    buildApproveArgs,
    buildAttestReservesArgs,
    buildRegisterIdentityArgs,
    buildUpdateReputationArgs,
    buildDepositMarginInsuranceArgs,
    bootstrapMarginInsurance,
    buildCreateMarketArgs,
    getTokenBalanceRaw,
    getTokenAllowanceRaw,
    getMarginInsuranceRaw,
    waitForMarginInsurance,
    SPORES_PER_LICN,
};
