#!/usr/bin/env node
'use strict';

const nacl = require('tweetnacl');
const { loadFundedWallets } = require('./helpers/funded-wallets');

const RPC_URL = process.env.LICHEN_RPC || 'http://127.0.0.1:8899';
const REST_BASE = `${RPC_URL}/api/v1`;
const PRICE_SCALE = 1_000_000_000;

const BS58 = '123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz';
function bs58encode(bytes) {
  let lz = 0;
  for (let i = 0; i < bytes.length && bytes[i] === 0; i++) lz++;
  let num = 0n;
  for (const b of bytes) num = num * 256n + BigInt(b);
  let enc = '';
  while (num > 0n) {
    enc = BS58[Number(num % 58n)] + enc;
    num /= 58n;
  }
  return '1'.repeat(lz) + enc;
}
function bs58decode(str) {
  let num = 0n;
  for (const c of str) {
    const i = BS58.indexOf(c);
    if (i < 0) throw new Error(`Bad base58 char: ${c}`);
    num = num * 58n + BigInt(i);
  }
  const hex = num === 0n ? '' : num.toString(16);
  const padded = hex.length % 2 ? '0' + hex : hex;
  const out = [];
  for (let i = 0; i < padded.length; i += 2) out.push(parseInt(padded.slice(i, i + 2), 16));
  let leading = 0;
  for (let i = 0; i < str.length && str[i] === '1'; i++) leading++;
  const result = new Uint8Array(leading + out.length);
  result.set(out, leading);
  return result;
}
function hexToBytes(h) {
  const c = h.startsWith('0x') ? h.slice(2) : h;
  const out = new Uint8Array(c.length / 2);
  for (let i = 0; i < out.length; i++) out[i] = parseInt(c.slice(i * 2, i * 2 + 2), 16);
  return out;
}
function bytesToHex(b) {
  return Array.from(b).map(x => x.toString(16).padStart(2, '0')).join('');
}

function pubkeyBase58ToHex(addr) {
  return bytesToHex(bs58decode(addr).subarray(0, 32));
}

let rpcId = 1;
async function rpc(method, params = []) {
  const res = await fetch(RPC_URL, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ jsonrpc: '2.0', id: rpcId++, method, params }),
  });
  const json = await res.json();
  if (json.error) throw new Error(`RPC ${json.error.code}: ${json.error.message}`);
  return json.result;
}
async function rest(path) {
  const res = await fetch(`${REST_BASE}${path}`);
  const json = await res.json();
  if (!json.success) throw new Error(json.error || `REST failed: ${path}`);
  return json.data;
}
const sleep = ms => new Promise(r => setTimeout(r, ms));

function encodeMsg(instructions, blockhash, signer) {
  const parts = [];
  function pushU64(n) {
    const buf = new ArrayBuffer(8);
    const v = new DataView(buf);
    v.setUint32(0, n & 0xffffffff, true);
    v.setUint32(4, Math.floor(n / 0x100000000) & 0xffffffff, true);
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
  const out = new Uint8Array(total);
  let off = 0;
  for (const p of parts) {
    out.set(p, off);
    off += p.length;
  }
  return out;
}

async function sendTx(keypair, instructions) {
  const b64 = await buildTxBase64(keypair, instructions);
  return rpc('sendTransaction', [b64]);
}

async function buildTxBase64(keypair, instructions) {
  const bhRes = await rpc('getRecentBlockhash');
  const blockhash = typeof bhRes === 'string' ? bhRes : bhRes.blockhash;
  const normalized = instructions.map(ix => ({
    program_id: ix.program_id,
    accounts: ix.accounts || [keypair.address],
    data: typeof ix.data === 'string' ? Array.from(new TextEncoder().encode(ix.data)) : Array.from(ix.data),
  }));
  const msg = encodeMsg(normalized, blockhash, keypair.address);
  const sig = nacl.sign.detached(msg, keypair.secretKey);
  const payload = {
    signatures: [bytesToHex(sig)],
    message: { instructions: normalized, blockhash },
  };
  return Buffer.from(JSON.stringify(payload)).toString('base64');
}

function decodeReturnCodeFromBase64(retB64) {
  if (!retB64) return null;
  const buf = Buffer.from(retB64, 'base64');
  if (buf.length < 8) return null;
  return Number(buf.readBigUInt64LE(0));
}

async function simulateTxAndGetReturnCode(keypair, instructions) {
  const b64 = await buildTxBase64(keypair, instructions);
  const sim = await rpc('simulateTransaction', [b64]);
  const retB64 = sim?.returnData || sim?.return_data || null;
  return { code: decodeReturnCodeFromBase64(retB64), sim };
}

const CONTRACT_PID = bs58encode(new Uint8Array(32).fill(0xff));
function contractIx(callerAddr, contractAddr, argsBytes) {
  const data = JSON.stringify({ Call: { function: 'call', args: Array.from(argsBytes), value: 0 } });
  return { program_id: CONTRACT_PID, accounts: [callerAddr, contractAddr], data };
}

function writeU8(arr, off, n) { arr[off] = n & 0xff; }
function writeU64LE(view, off, n) { view.setBigUint64(off, BigInt(Math.round(n)), true); }
function writePubkey(arr, off, addr) { arr.set(bs58decode(addr).subarray(0, 32), off); }

function buildOpenPosition(trader, pairId, side, size, leverage, margin, marginMode) {
  const buf = new ArrayBuffer(67);
  const v = new DataView(buf);
  const a = new Uint8Array(buf);
  writeU8(a, 0, 2);
  writePubkey(a, 1, trader);
  writeU64LE(v, 33, pairId);
  writeU8(a, 41, side === 'long' ? 0 : 1);
  writeU64LE(v, 42, size);
  writeU64LE(v, 50, leverage);
  writeU64LE(v, 58, margin);
  writeU8(a, 66, marginMode); // 0=isolated, 1=cross
  return a;
}

function buildSetMarkPrice(caller, pairId, price) {
  const buf = new ArrayBuffer(49);
  const v = new DataView(buf);
  const a = new Uint8Array(buf);
  writeU8(a, 0, 1); // set_mark_price
  writePubkey(a, 1, caller);
  writeU64LE(v, 33, pairId);
  writeU64LE(v, 41, price);
  return a;
}

function buildClosePosition(trader, posId) {
  const buf = new ArrayBuffer(41);
  const v = new DataView(buf);
  const a = new Uint8Array(buf);
  writeU8(a, 0, 3);
  writePubkey(a, 1, trader);
  writeU64LE(v, 33, posId);
  return a;
}

async function discoverContracts() {
  const result = await rpc('getAllSymbolRegistry', [100]);
  const entries = result?.entries || [];
  const map = {};
  for (const e of entries) map[e.symbol] = e.program;
  if (!map.DEXMARGIN) throw new Error('DEXMARGIN not found in symbol registry');
  return { dexMargin: map.DEXMARGIN };
}

function newestOpenByType(positions, marginType) {
  return positions
    .filter(p => p.status === 'open' && p.marginType === marginType)
    .sort((a, b) => (b.positionId || 0) - (a.positionId || 0))[0];
}

(async function main() {
  try {
    console.log(`RPC: ${RPC_URL}`);
    const slot = await rpc('getSlot');
    console.log(`Slot: ${slot}`);

    const { dexMargin } = await discoverContracts();
    const funded = loadFundedWallets(1);
    if (!funded.length) throw new Error('No funded genesis wallet found');
    const trader = funded[0];
    console.log(`Trader: ${trader.address}`);

    const enabled = await rest('/margin/enabled-pairs');
    const enabledPairIds = enabled.enabledPairIds || [];
    if (!enabledPairIds.includes(1)) throw new Error('Pair 1 is not margin-enabled on this validator');

    const traderHex = pubkeyBase58ToHex(trader.address);
    const before = await rest(`/margin/positions?trader=${traderHex}`);
    const beforeCount = Array.isArray(before) ? before.length : 0;
    console.log(`Positions before: ${beforeCount}`);

    const markSig = await sendTx(trader, [
      contractIx(trader.address, dexMargin, buildSetMarkPrice(trader.address, 1, Math.round(0.1 * PRICE_SCALE))),
    ]);
    console.log(`Mark price refresh tx: ${String(markSig).slice(0, 16)}...`);
    await sleep(900);

    const size = Math.round(1 * PRICE_SCALE);
    const isolatedLeverage = 5;
    const isolatedMargin = Math.round(0.1 * PRICE_SCALE);

    const isoIx = contractIx(trader.address, dexMargin, buildOpenPosition(trader.address, 1, 'long', size, isolatedLeverage, isolatedMargin, 0));
    const isoSim = await simulateTxAndGetReturnCode(trader, [isoIx]);
    console.log(`Isolated simulate return code: ${isoSim.code}`);
    const isoSig = await sendTx(trader, [isoIx]);
    console.log(`Isolated open tx: ${String(isoSig).slice(0, 16)}...`);
    await sleep(1200);

    const afterIso = await rest(`/margin/positions?trader=${traderHex}`);
    const isoPos = newestOpenByType(afterIso, 'isolated');
    if (!isoPos) throw new Error(`Isolated position not found via API (simulate code=${isoSim.code})`);
    if (isoPos.leverage !== isolatedLeverage) throw new Error(`Isolated leverage mismatch: ${isoPos.leverage}`);
    console.log(`Isolated verified: positionId=${isoPos.positionId}, marginType=${isoPos.marginType}, leverage=${isoPos.leverage}`);

    const crossLeverage = 3;
    const crossMargin = Math.round(0.11 * PRICE_SCALE);
    const crossIx = contractIx(trader.address, dexMargin, buildOpenPosition(trader.address, 1, 'long', size, crossLeverage, crossMargin, 1));
    const crossSim = await simulateTxAndGetReturnCode(trader, [crossIx]);
    console.log(`Cross simulate return code: ${crossSim.code}`);
    const crossSig = await sendTx(trader, [crossIx]);
    console.log(`Cross open tx: ${String(crossSig).slice(0, 16)}...`);
    await sleep(1200);

    const afterCross = await rest(`/margin/positions?trader=${traderHex}`);
    const crossPos = newestOpenByType(afterCross, 'cross');
    if (!crossPos) throw new Error(`Cross position not found via API (simulate code=${crossSim.code})`);
    if (crossPos.leverage !== crossLeverage) throw new Error(`Cross leverage mismatch: ${crossPos.leverage}`);
    console.log(`Cross verified: positionId=${crossPos.positionId}, marginType=${crossPos.marginType}, leverage=${crossPos.leverage}`);

    const countBeforeInvalid = afterCross.length;
    const badCrossSig = await sendTx(trader, [
      contractIx(trader.address, dexMargin, buildOpenPosition(trader.address, 1, 'long', size, 4, crossMargin, 1)),
    ]);
    console.log(`Cross(4x) attempt tx: ${String(badCrossSig).slice(0, 16)}...`);
    await sleep(1200);

    const afterInvalid = await rest(`/margin/positions?trader=${traderHex}`);
    const hasCross4x = (afterInvalid || []).some(p => p.status === 'open' && p.marginType === 'cross' && p.leverage === 4);
    if (hasCross4x) throw new Error('Cross 4x should be rejected but was found open');
    if (afterInvalid.length !== countBeforeInvalid) {
      throw new Error(`Position count changed after invalid cross attempt: before=${countBeforeInvalid}, after=${afterInvalid.length}`);
    }
    console.log('Cross leverage enforcement verified: 4x cross did not open');

    const closeTargets = [isoPos, crossPos].filter(Boolean);
    for (const p of closeTargets) {
      const sig = await sendTx(trader, [
        contractIx(trader.address, dexMargin, buildClosePosition(trader.address, p.positionId)),
      ]);
      console.log(`Closed position ${p.positionId}: ${String(sig).slice(0, 16)}...`);
      await sleep(700);
    }

    const finalPositions = await rest(`/margin/positions?trader=${traderHex}`);
    const finalOpen = (finalPositions || []).filter(p => p.status === 'open');
    console.log(`Final open positions for trader: ${finalOpen.length}`);

    console.log('\nPASS: Live end-to-end isolated/cross flow verified on local validator.');
    process.exit(0);
  } catch (e) {
    console.error(`\nFAIL: ${e.message}`);
    process.exit(1);
  }
})();
