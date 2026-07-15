#!/usr/bin/env node
/**
 * Lichen SporePump Launchpad & Governance E2E Test Suite
 *
 * Comprehensive end-to-end tests covering:
 *   1.  Contract discovery (SPOREPUMP, DEXGOV via symbol registry)
 *   2.  Multi-wallet funding (4 wallets via faucet)
 *   3.  Token creation (2 tokens via SporePump, verify 10 LICN fee)
 *   4.  Bonding curve buy (multi-wallet buys, price increase)
 *   5.  Bonding curve sell (cooldown, partial exit)
 *   6.  Buy quote accuracy (get_buy_quote matches actual buy)
 *   7.  Token info read (supply, price, market cap, graduated flag)
 *   8.  Platform stats (token count, fees collected)
 *   9.  Multi-token scenario (second token, isolated curves)
 *  10.  Governance: propose new pair listing
 *  11.  Governance: vote on proposal (multi-voter)
 *  12.  Governance: finalize + execute proposal
 *  13.  Governance: proposal info read
 *  14.  Governance stats (proposal count, total votes)
 *  15.  Edge cases (double create, zero buy, insufficient funds)
 *
 * Usage:
 *   node tests/e2e-launchpad.js
 *
 * Prerequisites:
 *   - Validator running with --dev-mode on port 8899
 *   - Contracts deployed (genesis auto-deploy)
 */
'use strict';

const pq = require('./helpers/pq-node');
const { loadFundedWallets, findGenesisAdminKeypair } = require('./helpers/funded-wallets');
const { waitForSuccessfulTransaction } = require('./helpers/tx-receipt');
const { encodeNativeTransactionBase64, signNativeTransaction } = require('./helpers/tx-wire');
const fs = require('fs');
const path = require('path');

const RPC_URL = process.env.LICHEN_RPC || 'http://127.0.0.1:8899';
const REST_BASE = `${RPC_URL}/api/v1`;
const SPORES_PER_LICN = 1_000_000_000;  // 1 LICN = 1e9 spores
const GOVERNANCE_REPUTATION_THRESHOLD = 500;

// ═══════════════════════════════════════════════════════════════════════════════
// Test harness
// ═══════════════════════════════════════════════════════════════════════════════
let passed = 0, failed = 0, skipped = 0;
function assert(cond, msg) {
    if (cond) { passed++; process.stdout.write(`  ✓ ${msg}\n`); }
    else { failed++; process.stderr.write(`  ✗ ${msg}\n`); }
}
function assertEq(a, b, msg) { assert(a === b, `${msg} (expected ${b}, got ${a})`); }
function assertGt(a, b, msg) { assert(a > b, `${msg} (expected > ${b}, got ${a})`); }
function assertGte(a, b, msg) { assert(a >= b, `${msg} (expected >= ${b}, got ${a})`); }
function section(name) { console.log(`\n── ${name} ──`); }

// ═══════════════════════════════════════════════════════════════════════════════
// Base58
// ═══════════════════════════════════════════════════════════════════════════════
const BS58 = '123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz';
function bs58encode(bytes) {
    let lz = 0; for (let i = 0; i < bytes.length && bytes[i] === 0; i++) lz++;
    let num = 0n; for (const b of bytes) num = num * 256n + BigInt(b);
    let enc = ''; while (num > 0n) { enc = BS58[Number(num % 58n)] + enc; num /= 58n; }
    return '1'.repeat(lz) + enc;
}
function bs58decode(str) {
    let num = 0n;
    for (const c of str) { const i = BS58.indexOf(c); if (i < 0) throw new Error(`Bad b58: ${c}`); num = num * 58n + BigInt(i); }
    const hex = num === 0n ? '' : num.toString(16); const padded = hex.length % 2 ? '0' + hex : hex;
    const bytes = []; for (let i = 0; i < padded.length; i += 2) bytes.push(parseInt(padded.slice(i, i + 2), 16));
    let lo = 0; for (let i = 0; i < str.length && str[i] === '1'; i++) lo++;
    const r = new Uint8Array(lo + bytes.length); r.set(bytes, lo); return r;
}
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
let rpcId = 1;
async function rpc(method, params = []) {
    const res = await fetch(RPC_URL, {
        method: 'POST', headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ jsonrpc: '2.0', id: rpcId++, method, params }),
    });
    const json = await res.json();
    if (json.error) throw new Error(`RPC ${json.error.code}: ${json.error.message}`);
    return json.result;
}
async function rest(path) {
    const res = await fetch(`${REST_BASE}${path}`);
    if (!res.ok) return null;
    return res.json();
}
const sleep = ms => new Promise(r => setTimeout(r, ms));

async function pollRest(path, predicate, timeoutMs = 20000, pollMs = 500) {
    const started = Date.now();
    let last = null;
    while ((Date.now() - started) < timeoutMs) {
        last = await rest(path);
        if (last && predicate(last)) return last;
        await sleep(pollMs);
    }
    return last;
}

function launchpadTokenRows(payload) {
    return Array.isArray(payload?.data?.tokens) ? payload.data.tokens : [];
}

async function launchpadTokenIds() {
    const response = await rest('/launchpad/tokens?limit=200');
    return launchpadTokenRows(response).map((token) => Number(token.id)).filter((id) => id > 0);
}

async function waitForNewLaunchpadToken(previousIds, timeoutMs = 30_000) {
    const response = await pollRest(
        '/launchpad/tokens?limit=200',
        (payload) => launchpadTokenRows(payload).some((token) => !previousIds.has(Number(token.id))),
        timeoutMs,
        500,
    );
    return launchpadTokenRows(response)
        .filter((token) => !previousIds.has(Number(token.id)))
        .sort((left, right) => Number(right.id) - Number(left.id))[0] || null;
}

function proposalIdOf(proposal) {
    return Number(proposal?.id ?? proposal?.proposalId ?? proposal?.proposal_id ?? 0);
}

async function maxGovernanceProposalId() {
    const proposals = await rest('/governance/proposals');
    const ids = (proposals?.data || []).map(proposalIdOf).filter((id) => id > 0);
    return ids.length ? Math.max(...ids) : 0;
}

function canonicalDuplicateCount(pairs) {
    const seen = new Set();
    let duplicates = 0;
    for (const pair of pairs || []) {
        const base = String(pair.baseToken || '');
        const quote = String(pair.quoteToken || '');
        const key = base < quote ? `${base}|${quote}` : `${quote}|${base}`;
        if (seen.has(key)) duplicates += 1;
        else seen.add(key);
    }
    return duplicates;
}

// ═══════════════════════════════════════════════════════════════════════════════
// Keypair generation
// ═══════════════════════════════════════════════════════════════════════════════
function genKeypair() {
    return pq.generateKeypair();
}

// ═══════════════════════════════════════════════════════════════════════════════
// Transaction building & signing
// ═══════════════════════════════════════════════════════════════════════════════
function encodeMsg(instructions, blockhash, signer) {
    const parts = [];
    function pushU64(n) {
        const buf = new ArrayBuffer(8); const v = new DataView(buf);
        v.setUint32(0, n & 0xFFFFFFFF, true); v.setUint32(4, Math.floor(n / 0x100000000) & 0xFFFFFFFF, true);
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
    parts.push(new Uint8Array([0x00]));  // compute_budget: None
    parts.push(new Uint8Array([0x00]));  // compute_unit_price: None
    const total = parts.reduce((s, a) => s + a.length, 0);
    const out = new Uint8Array(total); let off = 0;
    for (const a of parts) { out.set(a, off); off += a.length; }
    return out;
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
    const pqSig = signNativeTransaction(pq, keypair, msg);
    const b64 = encodeNativeTransactionBase64(
        [pqSig],
        { instructions: nix, blockhash: bh },
    );
    const signature = await rpc('sendTransaction', [b64]);
    await waitForSuccessfulTransaction(rpc, signature, 60_000, 250);
    return signature;
}

// Simulate a transaction without submitting it — returns { success, stateChanges, returnCode, logs }
async function simulateTx(keypair, instructions) {
    const bhRes = await rpc('getRecentBlockhash');
    const bh = typeof bhRes === 'string' ? bhRes : bhRes.blockhash;
    const nix = instructions.map(ix => ({
        program_id: ix.program_id,
        accounts: ix.accounts || [keypair.address],
        data: typeof ix.data === 'string' ? Array.from(new TextEncoder().encode(ix.data)) : Array.from(ix.data),
    }));
    const msg = encodeMsg(nix, bh, keypair.address);
    const pqSig = signNativeTransaction(pq, keypair, msg);
    const b64 = encodeNativeTransactionBase64(
        [pqSig],
        { instructions: nix, blockhash: bh },
    );
    return rpc('simulateTransaction', [b64]);
}

function simulationSummary(sim) {
    if (!sim) return 'no simulation result';
    return `success=${sim.success}, stateChanges=${sim.stateChanges}, returnCode=${sim.returnCode}, error=${sim.error || 'none'}`;
}

function assertSimulationRejected(sim, msg) {
    const rejected = sim && sim.success === false;
    assert(rejected, `${msg} (${simulationSummary(sim)})`);
}

// ═══════════════════════════════════════════════════════════════════════════════
// Contract call helpers
// ═══════════════════════════════════════════════════════════════════════════════
const CONTRACT_PID = bs58encode(new Uint8Array(32).fill(0xFF));
const NATIVE_LICN = bs58encode(new Uint8Array(32));

// Opcode-based ABI (DEX Governance uses opcode 0=byte)
function contractIx(callerAddr, contractAddr, argsBytes) {
    const data = JSON.stringify({ Call: { function: "call", args: Array.from(argsBytes), value: 0 } });
    return { program_id: CONTRACT_PID, accounts: [callerAddr, contractAddr], data };
}

// Named-export ABI (SporePump uses named WASM exports)
function namedCallIx(callerAddr, contractAddr, funcName, argsBytes, value = 0) {
    const data = JSON.stringify({ Call: { function: funcName, args: Array.from(argsBytes), value } });
    return { program_id: CONTRACT_PID, accounts: [callerAddr, contractAddr], data };
}

function deployContractIx(deployerAddr, contractAddr, wasm, abi) {
    const initData = new TextEncoder().encode(JSON.stringify({
        name: 'launchpad_token',
        template: 'sporepump-graduated-token-v1',
        make_public: true,
        abi,
    }));
    const data = JSON.stringify({
        Deploy: { code: Array.from(wasm), init_data: Array.from(initData) },
    });
    return { program_id: CONTRACT_PID, accounts: [deployerAddr, contractAddr], data };
}

// ═══════════════════════════════════════════════════════════════════════════════
// Binary encoding helpers
// ═══════════════════════════════════════════════════════════════════════════════
function writeU64LE(view, off, n) { view.setBigUint64(off, BigInt(Math.round(n)), true); }
function writeI16LE(view, off, n) { view.setInt16(off, n, true); }
function writeU16LE(view, off, n) { view.setUint16(off, n, true); }
function writeU8(arr, off, n) { arr[off] = n & 0xFF; }
function writePubkey(arr, off, addr) { arr.set(bs58decode(addr).subarray(0, 32), off); }
function readU64LE(data, off) {
    const dv = new DataView(data.buffer || new Uint8Array(data).buffer, data.byteOffset || 0);
    return Number(dv.getBigUint64(off, true));
}

// ═══════════════════════════════════════════════════════════════════════════════
// SporePump instruction builders (Named-export ABI)
// ═══════════════════════════════════════════════════════════════════════════════

// create_token(creator_addr[32] + fee_paid[u64]) → returns token_id (u64)
function buildCreateToken(creatorAddr) {
    const buf = new ArrayBuffer(40); const v = new DataView(buf); const a = new Uint8Array(buf);
    writePubkey(a, 0, creatorAddr);
    writeU64LE(v, 32, 10_000_000_000);  // CREATION_FEE = 10 LICN
    return a;
}

function buildCreateTokenWithMetadata(creatorAddr, name, symbol) {
    const nameBytes = new TextEncoder().encode(String(name).trim());
    const symbolBytes = new TextEncoder().encode(String(symbol).trim().toUpperCase());
    const nameStride = Math.max(32, nameBytes.length);
    const symbolStride = Math.max(32, symbolBytes.length);
    const layoutSize = 7;
    const buf = new ArrayBuffer(layoutSize + 32 + nameStride + 4 + symbolStride + 4 + 8);
    const v = new DataView(buf);
    const a = new Uint8Array(buf);
    a.set([0xAB, 32, nameStride, 4, symbolStride, 4, 8], 0);
    let offset = layoutSize;
    writePubkey(a, offset, creatorAddr);
    offset += 32;
    a.set(nameBytes, offset);
    offset += nameStride;
    v.setUint32(offset, nameBytes.length, true);
    offset += 4;
    a.set(symbolBytes, offset);
    offset += symbolStride;
    v.setUint32(offset, symbolBytes.length, true);
    offset += 4;
    writeU64LE(v, offset, 10_000_000_000);
    return a;
}

// buy(buyer_addr[32] + token_id[u64] + licn_amount[u64]) → returns tokens_received
function buildBuy(buyerAddr, tokenId, licnAmount) {
    const buf = new ArrayBuffer(48); const v = new DataView(buf); const a = new Uint8Array(buf);
    writePubkey(a, 0, buyerAddr);
    writeU64LE(v, 32, tokenId);
    writeU64LE(v, 40, licnAmount);
    return a;
}

// sell(seller_addr[32] + token_id[u64] + token_amount[u64]) → returns licn_refund
function buildSell(sellerAddr, tokenId, tokenAmount) {
    const buf = new ArrayBuffer(48); const v = new DataView(buf); const a = new Uint8Array(buf);
    writePubkey(a, 0, sellerAddr);
    writeU64LE(v, 32, tokenId);
    writeU64LE(v, 40, tokenAmount);
    return a;
}

// get_token_info(token_id[u64]) → return_data: 33 bytes
function buildGetTokenInfo(tokenId) {
    const buf = new ArrayBuffer(8); const v = new DataView(buf);
    writeU64LE(v, 0, tokenId);
    return new Uint8Array(buf);
}

// get_buy_quote(token_id[u64] + licn_amount[u64]) → returns tokens_you_get
function buildGetBuyQuote(tokenId, licnAmount) {
    const buf = new ArrayBuffer(16); const v = new DataView(buf);
    writeU64LE(v, 0, tokenId);
    writeU64LE(v, 8, licnAmount);
    return new Uint8Array(buf);
}

function buildGraduatedTokenInitialize(sporepump, tokenId, creator, maxSupply, obligations) {
    const buf = new ArrayBuffer(88); const v = new DataView(buf); const a = new Uint8Array(buf);
    writePubkey(a, 0, sporepump);
    v.setBigUint64(32, BigInt(tokenId), true);
    writePubkey(a, 40, creator);
    v.setBigUint64(72, BigInt(maxSupply), true);
    v.setBigUint64(80, BigInt(obligations), true);
    return a;
}

function buildBeginMigration(keeper, tokenId, candidate) {
    const buf = new ArrayBuffer(72); const v = new DataView(buf); const a = new Uint8Array(buf);
    writePubkey(a, 0, keeper);
    v.setBigUint64(32, BigInt(tokenId), true);
    writePubkey(a, 40, candidate);
    return a;
}

function buildKeeperTokenId(keeper, tokenId) {
    const buf = new ArrayBuffer(40); const v = new DataView(buf); const a = new Uint8Array(buf);
    writePubkey(a, 0, keeper);
    v.setBigUint64(32, BigInt(tokenId), true);
    return a;
}

function buildApprove(owner, spender, amount) {
    const buf = new ArrayBuffer(72); const v = new DataView(buf); const a = new Uint8Array(buf);
    writePubkey(a, 0, owner);
    writePubkey(a, 32, spender);
    v.setBigUint64(64, BigInt(amount), true);
    return a;
}

function buildAmmSwapExactIn(trader, poolId, tokenAIn, amountIn, minOut = 1n) {
    const buf = new ArrayBuffer(66); const v = new DataView(buf); const a = new Uint8Array(buf);
    a[0] = 6;
    writePubkey(a, 1, trader);
    v.setBigUint64(33, BigInt(poolId), true);
    a[41] = tokenAIn ? 1 : 0;
    v.setBigUint64(42, BigInt(amountIn), true);
    v.setBigUint64(50, BigInt(minOut), true);
    v.setBigUint64(58, 0n, true);
    return a;
}

async function readContractU64(contract, fn, args, from) {
    const result = await rpc('callContract', [
        contract,
        fn,
        Buffer.from(args).toString('base64'),
        from,
    ]);
    if (!result?.success || !result.returnData) throw new Error(`${fn} read failed`);
    const bytes = Buffer.from(result.returnData, 'base64');
    return Number(bytes.readBigUInt64LE(0));
}

// ═══════════════════════════════════════════════════════════════════════════════
// DEX Governance instruction builders (Opcode ABI)
// ═══════════════════════════════════════════════════════════════════════════════

// propose_new_pair: opcode 1, proposer[32] + base_token[32] + quote_token[32]
function buildProposeNewPair(proposerAddr, baseTokenAddr, quoteTokenAddr) {
    const buf = new ArrayBuffer(97); const a = new Uint8Array(buf);
    writeU8(a, 0, 1);
    writePubkey(a, 1, proposerAddr);
    writePubkey(a, 33, baseTokenAddr);
    writePubkey(a, 65, quoteTokenAddr);
    return a;
}

// vote: opcode 2, voter[32] + proposal_id[u64] + approve[u8]
function buildVote(voterAddr, proposalId, approve) {
    const buf = new ArrayBuffer(42); const v = new DataView(buf); const a = new Uint8Array(buf);
    writeU8(a, 0, 2);
    writePubkey(a, 1, voterAddr);
    writeU64LE(v, 33, proposalId);
    writeU8(a, 41, approve ? 1 : 0);
    return a;
}

// finalize_proposal: opcode 3, proposal_id[u64]
function buildFinalizeProposal(proposalId) {
    const buf = new ArrayBuffer(9); const v = new DataView(buf); const a = new Uint8Array(buf);
    writeU8(a, 0, 3);
    writeU64LE(v, 1, proposalId);
    return a;
}

// execute_proposal: opcode 4, proposal_id[u64]
function buildExecuteProposal(proposalId) {
    const buf = new ArrayBuffer(9); const v = new DataView(buf); const a = new Uint8Array(buf);
    writeU8(a, 0, 4);
    writeU64LE(v, 1, proposalId);
    return a;
}

// get_proposal_count: opcode 7
function buildGetProposalCount() {
    return new Uint8Array([7]);
}

// get_proposal_info: opcode 8, proposal_id[u64]
function buildGetProposalInfo(proposalId) {
    const buf = new ArrayBuffer(9); const v = new DataView(buf); const a = new Uint8Array(buf);
    writeU8(a, 0, 8);
    writeU64LE(v, 1, proposalId);
    return a;
}

// get_governance_stats: opcode 18
function buildGetGovernanceStats() {
    return new Uint8Array([18]);
}

// LichenID: admin_register_reserved_name — [admin 32B][owner 32B][name bytes][name_len 4B LE][agent_type 1B]
function buildAdminRegisterReservedName(adminAddr, ownerAddr, name, agentType = 0) {
    const nameBytes = new TextEncoder().encode(name);
    const total = 32 + 32 + nameBytes.length + 4 + 1;
    const buf = new ArrayBuffer(total);
    const a = new Uint8Array(buf);
    const v = new DataView(buf);
    writePubkey(a, 0, adminAddr);
    writePubkey(a, 32, ownerAddr);
    a.set(nameBytes, 64);
    v.setUint32(64 + nameBytes.length, nameBytes.length, true);
    writeU8(a, 64 + nameBytes.length + 4, agentType);
    return a;
}

// propose_fee_change: opcode 9, proposer[32] + pair_id[u64] + maker_fee[i16] + taker_fee[u16]
function buildProposeFeeChange(proposerAddr, pairId, makerFee, takerFee) {
    const buf = new ArrayBuffer(45); const v = new DataView(buf); const a = new Uint8Array(buf);
    writeU8(a, 0, 9);
    writePubkey(a, 1, proposerAddr);
    writeU64LE(v, 33, pairId);
    writeI16LE(v, 41, makerFee);
    writeU16LE(v, 43, takerFee);
    return a;
}

// emergency_delist: opcode 10, caller[32] + pair_id[u64]
function buildEmergencyDelist(callerAddr, pairId) {
    const buf = new ArrayBuffer(41); const v = new DataView(buf); const a = new Uint8Array(buf);
    writeU8(a, 0, 10);
    writePubkey(a, 1, callerAddr);
    writeU64LE(v, 33, pairId);
    return a;
}

// ═══════════════════════════════════════════════════════════════════════════════
// Contract discovery
// ═══════════════════════════════════════════════════════════════════════════════
const CONTRACTS = {};
async function discoverContracts() {
    const result = await rpc('getAllSymbolRegistry', [{ limit: 100 }]);
    const entries = result?.entries || [];
    const symbolMap = {
        'DEX': 'dex_core', 'DEXAMM': 'dex_amm', 'DEXROUTER': 'dex_router',
        'DEXMARGIN': 'dex_margin', 'DEXREWARDS': 'dex_rewards', 'DEXGOV': 'dex_governance',
        'ANALYTICS': 'dex_analytics', 'PREDICT': 'prediction_market',
        'LUSD': 'lusd_token', 'WSOL': 'wsol_token', 'WETH': 'weth_token',
        'ORACLE': 'lichenoracle', 'SPOREPUMP': 'sporepump', 'MOSS': 'moss_token',
        'YID': 'lichenid',
    };
    for (const e of entries) {
        const key = symbolMap[e.symbol] || e.symbol.toLowerCase();
        CONTRACTS[key] = e.program;
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Wallet setup helpers
// ═══════════════════════════════════════════════════════════════════════════════
async function fundWallet(wallet, amountLicn = 10) {
    try {
        const result = await rpc('requestAirdrop', [wallet.address, amountLicn]);
        return result;
    } catch (e) {
        if (String(e.message || '').includes('requestAirdrop is disabled in multi-validator mode')) {
            return { success: true, skipped: true };
        }
        throw e;
    }
}

async function getBalance(addr) {
    const result = await rpc('getBalance', [addr]);
    if (typeof result === 'number') return result;
    return result?.spendable ?? result?.spores ?? result?.value ?? 0;
}

async function getLichenIdReputation(addr) {
    try {
        const result = await rpc('getLichenIdReputation', [addr]);
        return Number(result?.score || 0);
    } catch {
        return 0;
    }
}

async function selectLaunchpadWallets(count, adminKeypair) {
    const adminAddress = adminKeypair?.address;
    const authorityKeyPattern = /^(genesis-primary|genesis-signer|community_treasury)/;
    const candidates = loadFundedWallets(count + 16).filter((wallet) => {
        if (wallet.address === adminAddress) return false;
        const sourceName = path.basename(wallet.source || '');
        return !authorityKeyPattern.test(sourceName);
    });
    const ranked = await Promise.all(candidates.map(async (wallet) => {
        let spendable = 0;
        let reputation = 0;
        try { spendable = await getBalance(wallet.address); } catch (_) { }
        try { reputation = await getLichenIdReputation(wallet.address); } catch (_) { }
        return { wallet, spendable: Number(spendable || 0), reputation: Number(reputation || 0) };
    }));
    const selected = ranked
        .filter(({ spendable, reputation }) => (
            spendable > 0 && reputation >= GOVERNANCE_REPUTATION_THRESHOLD
        ))
        .sort((left, right) => {
            if (right.spendable !== left.spendable) return right.spendable - left.spendable;
            return right.reputation - left.reputation;
        })
        .map(({ wallet }) => wallet);
    if (selected.length < count) {
        throw new Error(`need ${count} funded governance-eligible launchpad wallets, found ${selected.length}`);
    }
    return selected.slice(0, count);
}

// ═══════════════════════════════════════════════════════════════════════════════
// Main test runner
// ═══════════════════════════════════════════════════════════════════════════════
async function runTests() {
    await pq.init();
    console.log('╔═══════════════════════════════════════════════════════════════╗');
    console.log('║   Lichen SporePump Launchpad & Governance E2E Tests        ║');
    console.log('╚═══════════════════════════════════════════════════════════════╝');

    // ══════════════════════════════════════════════════════════════════════
    // 0. Health check
    // ══════════════════════════════════════════════════════════════════════
    section('0. Validator Health');
    try {
        const slot = await rpc('getSlot');
        assert(typeof slot === 'number' && slot >= 0, `Validator reachable, slot=${slot}`);
    } catch (e) {
        console.error(`FATAL: Cannot reach validator at ${RPC_URL}: ${e.message}`);
        process.exit(1);
    }

    // ══════════════════════════════════════════════════════════════════════
    // 1. Contract discovery
    // ══════════════════════════════════════════════════════════════════════
    section('1. Contract Discovery');
    await discoverContracts();
    assert(!!CONTRACTS.sporepump, `SporePump contract discovered: ${CONTRACTS.sporepump?.slice(0, 12)}...`);
    assert(!!CONTRACTS.dex_governance, `DEX Governance discovered: ${CONTRACTS.dex_governance?.slice(0, 12)}...`);
    assert(!!CONTRACTS.dex_core, `DEX Core discovered: ${CONTRACTS.dex_core?.slice(0, 12)}...`);
    assert(!!CONTRACTS.dex_amm, `DEX AMM discovered: ${CONTRACTS.dex_amm?.slice(0, 12)}...`);

    const hasSporePump = !!CONTRACTS.sporepump;
    const hasGov = !!CONTRACTS.dex_governance;

    let baselinePairDuplicates = 0;
    try {
        const baselinePairs = await rest('/pairs');
        baselinePairDuplicates = canonicalDuplicateCount(baselinePairs?.data || []);
        assert(baselinePairs?.data?.length > 0, `Baseline duplicate canonical pairs: ${baselinePairDuplicates}`);
    } catch (e) {
        assert(false, `Baseline pair-duplicate snapshot failed: ${e.message}`);
    }

    // ══════════════════════════════════════════════════════════════════════
    // 2. Multi-wallet funding
    // ══════════════════════════════════════════════════════════════════════
    section('2. Multi-Wallet Funding');
    const genesisAdmin = findGenesisAdminKeypair();
    const funded = await selectLaunchpadWallets(4, genesisAdmin);
    const alice = funded[0] || genKeypair();
    const bob = funded[1] || genKeypair();
    const charlie = funded[2] || genKeypair();
    const dave = funded[3] || genKeypair();
    console.log(`  Alice:   ${alice.address.slice(0, 16)}...`);
    console.log(`  Bob:     ${bob.address.slice(0, 16)}...`);
    console.log(`  Charlie: ${charlie.address.slice(0, 16)}...`);
    console.log(`  Dave:    ${dave.address.slice(0, 16)}...`);

    if (funded.length >= 4) {
        assert(true, 'Loaded funded genesis wallets (airdrop not required)');
    }
    for (const w of [alice, bob, charlie, dave]) {
        try {
            await fundWallet(w, 10);
        } catch (e) {
            assert(false, `Wallet funding failed for ${w.address.slice(0, 12)}: ${e.message}`);
        }
        await sleep(500);
    }
    await sleep(3000);  // wait for block confirmations

    const aliceBal = await getBalance(alice.address);
    assert(aliceBal > 0, `Alice funded: ${(aliceBal / SPORES_PER_LICN).toFixed(1)} LICN`);
    const bobBal = await getBalance(bob.address);
    assert(bobBal > 0, `Bob funded: ${(bobBal / SPORES_PER_LICN).toFixed(1)} LICN`);
    const charlieBal = await getBalance(charlie.address);
    assert(charlieBal > 0, `Charlie funded: ${(charlieBal / SPORES_PER_LICN).toFixed(1)} LICN`);

    let governancePrereqsReady = false;
    section('2b. Governance Identity Verification');
    if (hasGov && CONTRACTS.lichenid) {
        const currentReputations = new Map();
        for (const wallet of [alice, bob, charlie, dave]) {
            currentReputations.set(wallet.address, await getLichenIdReputation(wallet.address));
        }
        for (const [label, wallet] of [['Alice', alice], ['Bob', bob], ['Charlie', charlie], ['Dave', dave]]) {
            const score = currentReputations.get(wallet.address) || 0;
            assert(score >= GOVERNANCE_REPUTATION_THRESHOLD, `${label} governance reputation ready: ${score}`);
        }
        governancePrereqsReady = [alice, bob, charlie, dave].every(
            (wallet) => (currentReputations.get(wallet.address) || 0) >= GOVERNANCE_REPUTATION_THRESHOLD,
        );
    } else if (hasGov) {
        assert(false, 'LichenID contract required for governance was not discovered');
    }

    // ══════════════════════════════════════════════════════════════════════
    // 3. SporePump: Create Token #1
    // ══════════════════════════════════════════════════════════════════════
    let launchpadWritesObserved = false;
    if (hasSporePump) {
        section('3. SporePump: Create Token');
        const balBefore = await getBalance(alice.address);
        const tokenName1 = `Launch E2E ${Date.now()}`;
        const tokenSymbol1 = `L${Date.now().toString(36).toUpperCase().slice(-8)}`;

        let tokenId1 = 0;
        try {
            const previousTokenIds = new Set(await launchpadTokenIds());
            const creationFee = 10 * SPORES_PER_LICN;
            const result = await sendTx(alice, [
                namedCallIx(
                    alice.address,
                    CONTRACTS.sporepump,
                    'create_token_with_metadata',
                    buildCreateTokenWithMetadata(alice.address, tokenName1, tokenSymbol1),
                    creationFee,
                )
            ]);
            assert(!!result, 'Token #1 creation tx submitted');
            await sleep(2000);

            const balAfter = await getBalance(alice.address);
            const spent = balBefore - balAfter;
            assert(spent >= creationFee, `Creation fee deducted: ${(spent / SPORES_PER_LICN).toFixed(3)} LICN`);
            launchpadWritesObserved = true;

            const createdToken = await waitForNewLaunchpadToken(previousTokenIds);
            tokenId1 = Number(createdToken?.id || 0);
            assert(tokenId1 > 0, `Token #1 created and indexed (id=${tokenId1})`);
            assertEq(createdToken?.name, tokenName1, 'Custom token name is indexed exactly');
            assertEq(createdToken?.symbol, tokenSymbol1, 'Custom token symbol is indexed exactly');
        } catch (e) {
            assert(false, `Token creation failed: ${e.message}`);
        }

        // ══════════════════════════════════════════════════════════════════
        // 4. SporePump: Buy on bonding curve (multi-wallet)
        // ══════════════════════════════════════════════════════════════════
        section('4. SporePump: Buy on Bonding Curve');
        if (tokenId1 > 0) {
            const initialInfo = await rest(`/launchpad/tokens/${tokenId1}`);
            const initialPrice = Number(initialInfo?.data?.current_price || 0);
            const initialQuote = await rest(`/launchpad/tokens/${tokenId1}/quote?amount=5`);
            const quotedAliceTokensRaw = Math.round(Number(initialQuote?.data?.tokens_received || 0) * SPORES_PER_LICN);
            const aliceHolderBefore = await rest(`/launchpad/tokens/${tokenId1}/holders?address=${alice.address}`);
            const aliceTokensBefore = Number(aliceHolderBefore?.data?.balance_raw || 0);
            assert(quotedAliceTokensRaw > 0, 'Alice 5 LICN buy quote returns tokens');

            // Alice buys 5 LICN worth
            try {
                const buyAmount1 = 5n * BigInt(SPORES_PER_LICN);
                const result = await sendTx(alice, [
                    namedCallIx(alice.address, CONTRACTS.sporepump, 'buy', buildBuy(alice.address, tokenId1, Number(buyAmount1)), Number(buyAmount1))
                ]);
                assert(!!result, `Alice bought tokens for 5 LICN`);
                await sleep(2500);  // wait for buy cooldown (2s) + confirmation
            } catch (e) {
                assert(false, `Alice buy failed: ${e.message}`);
            }

            // Bob buys 10 LICN worth (price should be higher now)
            try {
                const buyAmount2 = 10n * BigInt(SPORES_PER_LICN);
                const result = await sendTx(bob, [
                    namedCallIx(bob.address, CONTRACTS.sporepump, 'buy', buildBuy(bob.address, tokenId1, Number(buyAmount2)), Number(buyAmount2))
                ]);
                assert(!!result, `Bob bought tokens for 10 LICN`);
                await sleep(2500);
            } catch (e) {
                assert(false, `Bob buy failed: ${e.message}`);
            }

            // Charlie buys 3 LICN worth
            try {
                const buyAmount3 = 3n * BigInt(SPORES_PER_LICN);
                const result = await sendTx(charlie, [
                    namedCallIx(charlie.address, CONTRACTS.sporepump, 'buy', buildBuy(charlie.address, tokenId1, Number(buyAmount3)), Number(buyAmount3))
                ]);
                assert(!!result, `Charlie bought tokens for 3 LICN`);
                await sleep(2500);
            } catch (e) {
                assert(false, `Charlie buy failed: ${e.message}`);
            }

            const boughtInfo = await pollRest(
                `/launchpad/tokens/${tokenId1}`,
                (response) => Number(response?.data?.current_price || 0) > initialPrice,
                30_000,
                500,
            );
            assert(Number(boughtInfo?.data?.current_price || 0) > initialPrice, 'Bonding-curve price increased after buys');
            for (const [label, wallet] of [['Alice', alice], ['Bob', bob], ['Charlie', charlie]]) {
                const holder = await rest(`/launchpad/tokens/${tokenId1}/holders?address=${wallet.address}`);
                assert(Number(holder?.data?.balance_raw || 0) > 0, `${label} launchpad token balance is indexed`);
                if (wallet.address === alice.address) {
                    assertEq(
                        Number(holder.data.balance_raw) - aliceTokensBefore,
                        quotedAliceTokensRaw,
                        'Alice received the exact quoted token amount',
                    );
                }
            }
        }

        // ══════════════════════════════════════════════════════════════════
        // 5. SporePump: Read token info
        // ══════════════════════════════════════════════════════════════════
        section('5. SporePump: Token Info');
        if (tokenId1 > 0) {
            const tokenInfo = await rest(`/launchpad/tokens/${tokenId1}`);
            assert(Number(tokenInfo?.data?.id || 0) === tokenId1, 'Token info returns the created token');
            assert(Number(tokenInfo?.data?.supply_sold || 0) > 0, 'Token info reports sold supply');
            assert(Number(tokenInfo?.data?.licn_raised || 0) > 0, 'Token info reports LICN raised');
            assert(Number(tokenInfo?.data?.current_price || 0) > 0, 'Token info reports current price');
        }

        // ══════════════════════════════════════════════════════════════════
        // 6. SporePump: Sell tokens (test cooldown)
        // ══════════════════════════════════════════════════════════════════
        section('6. SporePump: Sell Tokens');
        if (tokenId1 > 0) {
            // Alice sells a small amount of her tokens
            try {
                await sleep(5500);  // wait for sell cooldown (5s)
                const holderBefore = await rest(`/launchpad/tokens/${tokenId1}/holders?address=${alice.address}`);
                const balanceBefore = Number(holderBefore?.data?.balance_raw || 0);
                const licnBefore = await getBalance(alice.address);
                const sellAmount = Math.max(1, Math.floor(balanceBefore / 10));
                const result = await sendTx(alice, [
                    namedCallIx(alice.address, CONTRACTS.sporepump, 'sell', buildSell(alice.address, tokenId1, sellAmount))
                ]);
                assert(!!result, `Alice sold ${sellAmount.toLocaleString()} tokens`);
                await sleep(2000);
                const holderAfter = await rest(`/launchpad/tokens/${tokenId1}/holders?address=${alice.address}`);
                assertEq(Number(holderAfter?.data?.balance_raw || 0), balanceBefore - sellAmount, 'Alice token balance decreased by sold amount');
                assert((await getBalance(alice.address)) > licnBefore, 'Alice received LICN sale proceeds');
            } catch (e) {
                assert(false, `Alice sell failed: ${e.message}`);
            }

            // Bob tries to sell immediately (should hit cooldown or work if enough time passed)
            try {
                await sleep(5500);
                const sellAmount = 500_000;
                const result = await sendTx(bob, [
                    namedCallIx(bob.address, CONTRACTS.sporepump, 'sell', buildSell(bob.address, tokenId1, sellAmount))
                ]);
                assert(!!result, `Bob sold ${sellAmount.toLocaleString()} tokens`);
                await sleep(2000);
            } catch (e) {
                assert(false, `Bob sell failed: ${e.message}`);
            }
        }

        // ══════════════════════════════════════════════════════════════════
        // 7. SporePump: Create Token #2 (isolated curves)
        // ══════════════════════════════════════════════════════════════════
        section('7. SporePump: Second Token (Isolated Curves)');
        let tokenId2 = 0;
        try {
            const previousTokenIds = new Set(await launchpadTokenIds());
            const creationFee2 = 10 * SPORES_PER_LICN;
            const result = await sendTx(bob, [
                namedCallIx(bob.address, CONTRACTS.sporepump, 'create_token', buildCreateToken(bob.address), creationFee2)
            ]);
            assert(!!result, 'Token #2 creation tx submitted (Bob)');
            await sleep(2000);
            const createdToken = await waitForNewLaunchpadToken(previousTokenIds);
            tokenId2 = Number(createdToken?.id || 0);
            assert(tokenId2 > 0 && tokenId2 !== tokenId1, `Second token indexed independently (id=${tokenId2})`);

            // Charlie buys token #2 to verify isolated curves
            const buyAmount = 2n * BigInt(SPORES_PER_LICN);
            const buyResult = await sendTx(charlie, [
                namedCallIx(charlie.address, CONTRACTS.sporepump, 'buy', buildBuy(charlie.address, tokenId2, Number(buyAmount)), Number(buyAmount))
            ]);
            assert(!!buyResult, 'Charlie bought Token #2 for 2 LICN');
            await sleep(2000);
            const token1Info = await rest(`/launchpad/tokens/${tokenId1}`);
            const token2Info = await rest(`/launchpad/tokens/${tokenId2}`);
            assert(Number(token1Info?.data?.id) !== Number(token2Info?.data?.id), 'Bonding curves retain distinct token identities');
        } catch (e) {
            assert(false, `Token #2 creation/buy failed: ${e.message}`);
        }

        // ══════════════════════════════════════════════════════════════════
        // 8. SporePump: Platform stats
        // ══════════════════════════════════════════════════════════════════
        section('8. SporePump: Platform Stats');
        try {
            const stats = await rest('/launchpad/stats');
            assert(Number(stats?.data?.token_count || 0) >= 2, 'Platform stats count both created tokens');
            assert(Number(stats?.data?.fees_collected || 0) >= 20, 'Platform stats include creation and trading fees');
            assert(Number(stats?.data?.total_raised || 0) > 0, 'Platform stats include LICN raised');
        } catch (e) {
            assert(false, `Platform stats failed: ${e.message}`);
        }

        // ══════════════════════════════════════════════════════════════════
        // 9. SporePump: Edge cases
        // ══════════════════════════════════════════════════════════════════
        section('9. SporePump: Edge Cases');

        // 9a. Buy with 0 amount (must reject, no accepted no-op)
        try {
            const sim = await simulateTx(dave, [
                namedCallIx(dave.address, CONTRACTS.sporepump, 'buy', buildBuy(dave.address, tokenId1, 0), 0)
            ]);
            assertSimulationRejected(sim, 'Zero-amount buy rejected at preflight');
        } catch (e) {
            assert(false, `Zero-amount buy preflight failed unexpectedly: ${e.message}`);
        }

        // 9b. Buy non-existent token (id=999) must reject so native LICN value is not accepted.
        const daveBalanceBefore = await getBalance(dave.address);
        const invalidBuyIx = namedCallIx(dave.address, CONTRACTS.sporepump, 'buy', buildBuy(dave.address, 999, SPORES_PER_LICN), SPORES_PER_LICN);
        const invalidBuySimulation = await simulateTx(dave, [invalidBuyIx]);
        assertSimulationRejected(invalidBuySimulation, 'Buy non-existent token rejected at preflight');
        try {
            await sendTx(dave, [invalidBuyIx]);
            assert(false, 'Buy non-existent token must not submit');
        } catch (submitErr) {
            assert(/simulation failed|failure value|token not found/i.test(submitErr.message), `Buy non-existent token send rejected: ${submitErr.message.slice(0, 60)}`);
        }
        const daveBalanceAfter = await getBalance(dave.address);
        assertEq(daveBalanceAfter, daveBalanceBefore, 'Rejected invalid buy left Dave balance unchanged');

        // 9c. Sell more tokens than owned must reject instead of accepting an empty no-op.
        try {
            await sleep(5500);
            const sim = await simulateTx(dave, [
                namedCallIx(dave.address, CONTRACTS.sporepump, 'sell', buildSell(dave.address, tokenId1, 999_999_999_999))
            ]);
            assertSimulationRejected(sim, 'Sell more than owned rejected at preflight');
        } catch (e) {
            assert(false, `Oversized sell preflight failed unexpectedly: ${e.message}`);
        }

    } else {
        assert(false, 'SporePump contract is required for launchpad user-flow coverage');
    }

    // ══════════════════════════════════════════════════════════════════════
    // 10. DEX Governance: Propose new pair
    // ══════════════════════════════════════════════════════════════════════
    if (hasGov && governancePrereqsReady) {
        section('10. Governance: Propose New Pair');
        let proposalId = 0;

        const beforeProposalId = await maxGovernanceProposalId();
        const baseToken = genKeypair().address;
        const quoteToken = CONTRACTS.lusd_token || bob.address;

        try {
            const args = buildProposeNewPair(alice.address, baseToken, quoteToken);
            const result = await sendTx(alice, [
                contractIx(alice.address, CONTRACTS.dex_governance, args)
            ]);
            assert(!!result, 'Governance proposal submitted');
            await sleep(2000);

            const proposals = await pollRest(
                '/governance/proposals',
                (resp) => (resp?.data || []).some((proposal) => proposalIdOf(proposal) > beforeProposalId),
                30000,
                1000,
            );
            const latestProposal = (proposals?.data || [])
                .filter((proposal) => proposalIdOf(proposal) > beforeProposalId)
                .sort((left, right) => proposalIdOf(right) - proposalIdOf(left))[0];
            proposalId = proposalIdOf(latestProposal);
            assert(proposalId > beforeProposalId, `Governance proposal listed: id=${proposalId}`);
        } catch (e) {
            assert(false, `Governance proposal failed: ${e.message}`);
        }

        // ══════════════════════════════════════════════════════════════════
        // 11. Governance: Vote on proposal
        // ══════════════════════════════════════════════════════════════════
        section('11. Governance: Multi-Voter Voting');
        if (proposalId > 0) {
            // Alice votes YES
            try {
                const result = await sendTx(alice, [
                    contractIx(alice.address, CONTRACTS.dex_governance, buildVote(alice.address, proposalId, true))
                ]);
                assert(!!result, 'Alice voted YES');
                await sleep(1000);
            } catch (e) {
                assert(false, `Alice vote failed: ${e.message}`);
            }

            // Bob votes YES
            try {
                const result = await sendTx(bob, [
                    contractIx(bob.address, CONTRACTS.dex_governance, buildVote(bob.address, proposalId, true))
                ]);
                assert(!!result, 'Bob voted YES');
                await sleep(1000);
            } catch (e) {
                assert(false, `Bob vote failed: ${e.message}`);
            }

            // Charlie votes YES
            try {
                const result = await sendTx(charlie, [
                    contractIx(charlie.address, CONTRACTS.dex_governance, buildVote(charlie.address, proposalId, true))
                ]);
                assert(!!result, 'Charlie voted YES');
                await sleep(1000);
            } catch (e) {
                assert(false, `Charlie vote failed: ${e.message}`);
            }

            // Dave votes NO (minority)
            try {
                const result = await sendTx(dave, [
                    contractIx(dave.address, CONTRACTS.dex_governance, buildVote(dave.address, proposalId, false))
                ]);
                assert(!!result, 'Dave voted NO (minority)');
                await sleep(1000);
            } catch (e) {
                assert(false, `Dave vote failed: ${e.message}`);
            }
        }

        // ══════════════════════════════════════════════════════════════════
        // 12. Governance: Finalize proposal
        // ══════════════════════════════════════════════════════════════════
        section('12. Governance: Finalize & Execute');
        if (proposalId > 0) {
            // Try to finalize (may fail if voting period hasn't ended — that's OK)
            try {
                const result = await sendTx(alice, [
                    contractIx(alice.address, CONTRACTS.dex_governance, buildFinalizeProposal(proposalId))
                ]);
                assert(!!result, 'Finalize proposal tx submitted');
                await sleep(2000);
            } catch (e) {
                // Expected: voting period is 172800 slots, so finalize will fail
                assert(/simulation failed|failure code/i.test(e.message), 'Finalize correctly requires voting period to end');
            }

            // Try to execute (should fail — not finalized yet)
            try {
                const sim = await simulateTx(alice, [
                    contractIx(alice.address, CONTRACTS.dex_governance, buildExecuteProposal(proposalId))
                ]);
                // Contract returns non-zero code (2 = not passed) with 0 state changes
                assert(
                    sim?.success === false && sim?.stateChanges === 0,
                    `Execute correctly blocked with no state changes (${simulationSummary(sim)})`,
                );
            } catch (e) {
                assert(/simulation failed|failure code/i.test(e.message), `Execute correctly requires passed status: ${e.message.slice(0, 60)}`);
            }
        }

        // ══════════════════════════════════════════════════════════════════
        // 13. Governance: Read proposal info
        // ══════════════════════════════════════════════════════════════════
        section('13. Governance: Read Proposal Info');
        try {
            const proposals = await rest('/governance/proposals');
            const proposal = (proposals?.data || []).find((entry) => proposalIdOf(entry) === proposalId);
            assert(proposalIdOf(proposal) === proposalId, 'Proposal info is readable through the public API');
            assert(Number(proposal?.yesVotes || 0) > 0, 'Proposal info includes YES votes');
            assert(Number(proposal?.noVotes || 0) > 0, 'Proposal info includes NO votes');
        } catch (e) {
            assert(false, `Proposal info read failed: ${e.message}`);
        }

        // ══════════════════════════════════════════════════════════════════
        // 14. Governance: Stats
        // ══════════════════════════════════════════════════════════════════
        section('14. Governance: Stats');
        try {
            const stats = await rest('/stats/governance');
            assert(stats?.data != null, 'Governance stats API responds');
            assert(Number(stats?.data?.proposalCount || 0) > 0, 'Governance stats include proposals');
            assert(Number(stats?.data?.totalVotes || 0) >= 4, 'Governance stats include all journey votes');
        } catch (e) {
            assert(false, `Governance stats failed: ${e.message}`);
        }

        // ══════════════════════════════════════════════════════════════════
        // 15. Governance: Propose fee change
        // ══════════════════════════════════════════════════════════════════
        section('15. Governance: Fee Change Proposal');
        try {
            const beforeFeeProposalId = await maxGovernanceProposalId();
            const args = buildProposeFeeChange(alice.address, 1, -5, 10);  // pair 1, maker: -5bps, taker: 10bps
            const result = await sendTx(alice, [
                contractIx(alice.address, CONTRACTS.dex_governance, args)
            ]);
            assert(!!result, 'Fee change proposal submitted');
            await sleep(2000);
            const feeProposals = await rest('/governance/proposals');
            assert(
                (feeProposals?.data || []).some((proposal) => proposalIdOf(proposal) > beforeFeeProposalId),
                'Fee change proposal is indexed',
            );
        } catch (e) {
            assert(false, `Fee change proposal failed: ${e.message}`);
        }

    } else {
        assert(false, `Governance prerequisites unavailable (${hasGov ? 'identity eligibility' : 'contract missing'})`);
    }

    // ══════════════════════════════════════════════════════════════════════
    // 16. REST API: Verify pairs data
    // ══════════════════════════════════════════════════════════════════════
    section('16. REST API: DEX Pairs');
    try {
        const pairs = await rest('/pairs');
        assert(pairs && pairs.data && pairs.data.length > 0, `DEX has ${pairs?.data?.length || 0} trading pairs`);
        if (pairs?.data?.length > 0) {
            const first = pairs.data[0];
            assert(first.pairId > 0, `First pair ID: ${first.pairId}`);
            assert(typeof first.lastPrice === 'number', `Has last price: ${first.lastPrice}`);

            // Explicit negative check: no duplicate listing paths (same/reversed pair)
            const duplicateCount = canonicalDuplicateCount(pairs.data);
            assert(
                duplicateCount <= baselinePairDuplicates,
                `Canonical duplicate pair count did not increase (${baselinePairDuplicates} -> ${duplicateCount})`,
            );
        }
    } catch (e) {
        failed++;
        console.error(`  ✗ Pairs API failed: ${e.message}`);
    }

    // ══════════════════════════════════════════════════════════════════════
    // 17. REST API: Verify tickers
    // ══════════════════════════════════════════════════════════════════════
    section('17. REST API: Tickers');
    try {
        const tickers = await rest('/tickers');
        assert(tickers && tickers.data && tickers.data.length > 0, `Tickers API returned ${tickers?.data?.length || 0} pairs`);
        if (tickers?.data?.length > 0) {
            const t = tickers.data[0];
            assert(typeof t.lastPrice === 'number', `Ticker has lastPrice: ${t.lastPrice}`);
            assert(typeof t.change24h === 'number', `Ticker has change24h: ${t.change24h}`);
            assert(typeof t.volume24h === 'number' || typeof t.volume24h === 'string', `Ticker has volume24h`);
        }
    } catch (e) {
        failed++;
        console.error(`  ✗ Tickers API failed: ${e.message}`);
    }

    // ══════════════════════════════════════════════════════════════════════
    // 18. Launchpad graduation -> DEX visibility/tradability
    // ══════════════════════════════════════════════════════════════════════
    section('18. Launchpad Graduation -> DEX Visibility/Tradability');
    try {
        assert(hasSporePump && !!CONTRACTS.dex_amm && !!CONTRACTS.dex_router, 'Graduation dependencies are discovered');
        const previousTokenIds = new Set(await launchpadTokenIds());
        const creationFee = 10 * SPORES_PER_LICN;
        const graduationName = `Graduation E2E ${Date.now()}`;
        const graduationSymbol = `G${Date.now().toString(36).toUpperCase().slice(-8)}`;
        await sendTx(alice, [
            namedCallIx(
                alice.address,
                CONTRACTS.sporepump,
                'create_token_with_metadata',
                buildCreateTokenWithMetadata(alice.address, graduationName, graduationSymbol),
                creationFee,
            ),
        ]);
        const launch = await waitForNewLaunchpadToken(previousTokenIds);
        const graduationTokenId = Number(launch?.id || 0);
        assert(graduationTokenId > 0, `Graduation launch created (id=${graduationTokenId})`);
        assertEq(launch?.name, graduationName, 'Graduation token name is replicated');
        assertEq(launch?.symbol, graduationSymbol, 'Graduation token symbol is replicated');

        const thresholdBuy = 60_000 * SPORES_PER_LICN;
        await sendTx(alice, [
            namedCallIx(
                alice.address,
                CONTRACTS.sporepump,
                'buy',
                buildBuy(alice.address, graduationTokenId, thresholdBuy),
                thresholdBuy,
            ),
        ]);
        const eligible = await pollRest(
            `/launchpad/tokens/${graduationTokenId}`,
            (response) => response?.data?.graduation_state === 'eligible',
            60_000,
            500,
        );
        assertEq(eligible?.data?.graduation_state, 'eligible', 'Threshold crossing enters Eligible');
        assert(Number(eligible?.data?.eligibility_slot || 0) > 0, 'Eligibility slot is persisted');
        assertSimulationRejected(
            await simulateTx(alice, [namedCallIx(
                alice.address,
                CONTRACTS.sporepump,
                'buy',
                buildBuy(alice.address, graduationTokenId, SPORES_PER_LICN),
                SPORES_PER_LICN,
            )]),
            'Eligible launch rejects further curve buys',
        );

        const templateWasm = fs.readFileSync(path.join(__dirname, '..', 'contracts', 'launchpad_token', 'launchpad_token.wasm'));
        const templateAbi = JSON.parse(fs.readFileSync(path.join(__dirname, '..', 'contracts', 'launchpad_token', 'abi.json'), 'utf8'));
        const candidate = genKeypair().address;
        await sendTx(alice, [deployContractIx(alice.address, candidate, templateWasm, templateAbi)]);
        const obligations = BigInt(eligible.data.supply_sold_raw);
        const maxSupply = BigInt(eligible.data.max_supply_raw);
        await sendTx(alice, [namedCallIx(
            alice.address,
            candidate,
            'initialize',
            buildGraduatedTokenInitialize(
                CONTRACTS.sporepump,
                graduationTokenId,
                alice.address,
                maxSupply,
                obligations,
            ),
        )]);

        await sendTx(alice, [namedCallIx(
            alice.address,
            CONTRACTS.sporepump,
            'begin_migration',
            buildBeginMigration(alice.address, graduationTokenId, candidate),
        )]);
        const migrating = await pollRest(
            `/launchpad/tokens/${graduationTokenId}`,
            (response) => response?.data?.graduation_state === 'migrating',
            30_000,
            500,
        );
        assertEq(migrating?.data?.migrated_token_program, bytesToHex(bs58decode(candidate)), 'Canonical candidate is bound');
        assert(Number(migrating?.data?.migration_boundary_slot || 0) > 0, 'Migration boundary slot is persisted');

        const finalizeIx = namedCallIx(
            alice.address,
            CONTRACTS.sporepump,
            'finalize_migration',
            buildKeeperTokenId(alice.address, graduationTokenId),
        );
        const finalizeSimulation = await simulateTx(alice, [finalizeIx]);
        assert(
            finalizeSimulation?.success === true,
            `Atomic graduation finalization preflight succeeds (${simulationSummary(finalizeSimulation)}; logs=${JSON.stringify(finalizeSimulation?.logs || [])})`,
        );
        if (!finalizeSimulation?.success) {
            throw new Error(`graduation preflight failed: ${JSON.stringify(finalizeSimulation)}`);
        }
        await sendTx(alice, [finalizeIx]);
        const graduated = await pollRest(
            `/launchpad/tokens/${graduationTokenId}`,
            (response) => response?.data?.graduation_state === 'graduated',
            60_000,
            500,
        );
        const token = graduated?.data;
        assert(token?.graduated === true, `Graduated token flagged true (id=${graduationTokenId})`);
        assert(Number(token?.pair_id || 0) > 0, `Graduation persisted pair id ${token?.pair_id}`);
        assert(Number(token?.pool_id || 0) > 0, `Graduation persisted pool id ${token?.pool_id}`);
        assert(Number(token?.route_id || 0) > 0 && Number(token?.reverse_route_id || 0) > 0, 'Both router directions are persisted');
        assert(Number(token?.licn_liquidity_raw || 0) > 0 && Number(token?.token_liquidity_raw || 0) > 0, 'Actual initial liquidity is persisted');
        assert((await rest(`/launchpad/tokens/${graduationTokenId}/quote?amount=1`)) === null, 'Graduated curve quote is disabled');

        const pairsNow = await rest('/pairs');
        const poolsNow = await rest('/pools');
        const graduatedPair = (pairsNow?.data || []).find((pair) => Number(pair.pairId) === Number(token.pair_id));
        assert(!!graduatedPair, 'Graduated CLOB pair is publicly indexed');
        assertEq(graduatedPair?.baseSymbol, graduationSymbol, 'Graduated pair preserves launch symbol');
        assertEq(graduatedPair?.symbol, `${graduationSymbol}/LICN`, 'Graduated pair exposes its canonical display symbol');
        assert(Number(graduatedPair?.lastPrice || 0) > 0, `Graduated pair has AMM-derived price: ${graduatedPair?.lastPrice}`);
        assert((poolsNow?.data || []).some((pool) => Number(pool.poolId) === Number(token.pool_id)), 'Graduated AMM pool is publicly indexed');

        const launchBalance = await rest(`/launchpad/tokens/${graduationTokenId}/holders?address=${alice.address}`);
        const claimAmount = BigInt(launchBalance?.data?.claimable_raw || 0);
        assert(claimAmount > 0n, 'Alice has a public frozen claim amount');
        await sendTx(alice, [namedCallIx(alice.address, candidate, 'claim', bs58decode(alice.address))]);
        const claimedHolder = await rest(`/launchpad/tokens/${graduationTokenId}/holders?address=${alice.address}`);
        assert(claimedHolder?.data?.claimed === true && Number(claimedHolder?.data?.balance_raw || 0) === 0, 'Claim consumes the launch balance exactly once');
        assertEq(
            await readContractU64(candidate, 'balance_of', bs58decode(alice.address), alice.address),
            Number(claimAmount),
            'Canonical token balance equals the frozen launch claim',
        );
        assertSimulationRejected(
            await simulateTx(alice, [namedCallIx(alice.address, candidate, 'claim', bs58decode(alice.address))]),
            'Repeated holder claim is rejected',
        );

        const tradeAmount = claimAmount > 1_000_000_000n ? 1_000_000_000n : claimAmount / 2n;
        await sendTx(alice, [namedCallIx(
            alice.address,
            candidate,
            'approve',
            buildApprove(alice.address, CONTRACTS.dex_amm, tradeAmount),
        )]);
        const nativeBeforeSwap = await getBalance(alice.address);
        await sendTx(alice, [contractIx(
            alice.address,
            CONTRACTS.dex_amm,
            buildAmmSwapExactIn(alice.address, token.pool_id, true, tradeAmount),
        )]);
        const tokenAfterSwap = await readContractU64(candidate, 'balance_of', bs58decode(alice.address), alice.address);
        assertEq(tokenAfterSwap, Number(claimAmount - tradeAmount), 'Graduated token debited by a real AMM swap');
        assert((await getBalance(alice.address)) > nativeBeforeSwap, 'Real AMM swap paid native LICN to the holder');
    } catch (e) {
        failed++;
        console.error(`  ✗ Launchpad graduation/DEX linkage check failed: ${e.message}`);
    }

    // ══════════════════════════════════════════════════════════════════════
    // 19. Post-test balance verification
    // ══════════════════════════════════════════════════════════════════════
    section('19. Post-Test Balance Verification');
    const finalBals = {};
    for (const [name, w] of [['Alice', alice], ['Bob', bob], ['Charlie', charlie], ['Dave', dave]]) {
        const bal = await getBalance(w.address);
        finalBals[name] = bal;
        console.log(`  ${name}: ${(bal / SPORES_PER_LICN).toFixed(2)} LICN`);
    }
    // Alice and Bob should have spent some LICN (creation fees + buys) when write-path calls succeeded
    assert(hasSporePump && launchpadWritesObserved && finalBals.Alice < aliceBal, 'Alice spent LICN on confirmed launchpad writes');
    assert(hasSporePump && finalBals.Bob < bobBal, 'Bob spent LICN on confirmed launchpad writes');

    // ══════════════════════════════════════════════════════════════════════
    // 20. REST API: 24h Stats verification
    // ══════════════════════════════════════════════════════════════════════
    section('20. REST API: 24h Stats');
    try {
        const tickers = await rest('/tickers');
        assert((tickers?.data?.length || 0) > 0, 'Ticker list is populated');
        const hasChange = tickers.data.some(t => typeof t.change24h === 'number');
        assert(hasChange, '24h change field present in ticker data');
        const hasHigh = tickers.data.some(t => typeof t.high24h === 'number');
        assert(hasHigh, '24h high field present in ticker data');
    } catch (e) {
        assert(false, `24h stats check failed: ${e.message}`);
    }

    // ══════════════════════════════════════════════════════════════════════
    // Summary
    // ══════════════════════════════════════════════════════════════════════
    console.log(`\n═══════════════════════════════════════════════`);
    console.log(`  Results: ${passed} passed, ${failed} failed, ${skipped} skipped`);
    console.log(`═══════════════════════════════════════════════\n`);
    process.exit(failed > 0 || skipped > 0 ? 1 : 0);
}

runTests().catch(e => { console.error(`FATAL: ${e.message}`); process.exit(1); });
