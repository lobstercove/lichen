'use strict';

const fs = require('fs');
const path = require('path');
const nacl = require('tweetnacl');

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
  for (const c of str) { const i = BS58.indexOf(c); if (i < 0) throw new Error(`Bad b58: ${c}`); num = num * 58n + BigInt(i); }
  const hex = num === 0n ? '' : num.toString(16); const padded = hex.length % 2 ? '0' + hex : hex;
  const bytes = []; for (let i = 0; i < padded.length; i += 2) bytes.push(parseInt(padded.slice(i, i + 2), 16));
  let lo = 0; for (let i = 0; i < str.length && str[i] === '1'; i++) lo++;
  const r = new Uint8Array(lo + bytes.length); r.set(bytes, lo); return r;
}

function bytesToHex(b) { return Array.from(b).map(x => x.toString(16).padStart(2, '0')).join(''); }

function genKeypair() {
  const kp = nacl.sign.keyPair();
  return { publicKey: kp.publicKey, secretKey: kp.secretKey, address: bs58encode(kp.publicKey) };
}

function loadKeysFromDir(keysDir, wallets, seen, limit) {
  if (!fs.existsSync(keysDir)) return;
  const files = fs.readdirSync(keysDir)
    .filter((f) => f.endsWith('.json'))
    .sort((a, b) => {
      const priority = (name) => {
        if (name.startsWith('genesis-primary')) return 0;
        if (name.startsWith('treasury')) return 1;
        if (name.startsWith('builder_grants')) return 2;
        if (name.startsWith('community_treasury')) return 3;
        if (name.startsWith('ecosystem_partnerships')) return 4;
        if (name.startsWith('reserve_pool')) return 5;
        if (name.startsWith('validator_rewards')) return 6;
        if (name.startsWith('genesis-signer')) return 7;
        if (name.startsWith('founding_symbionts')) return 8;
        return 9;
      };
      return priority(a) - priority(b) || a.localeCompare(b);
    });

  for (const file of files) {
    if (wallets.length >= limit) return;
    try {
      const raw = JSON.parse(fs.readFileSync(path.join(keysDir, file), 'utf8'));
      const hex = raw.secret_key;
      if (typeof hex !== 'string' || hex.length !== 64) continue;
      const seed = Uint8Array.from(Buffer.from(hex, 'hex'));
      const kp = nacl.sign.keyPair.fromSeed(seed);
      const addr = bs58encode(kp.publicKey);
      if (seen.has(addr)) continue;
      seen.add(addr);
      wallets.push({ publicKey: kp.publicKey, secretKey: kp.secretKey, address: addr, source: path.join(keysDir, file) });
    } catch (_) { }
  }
}

function loadFundedWallets(limit = 2) {
  const roots = [process.cwd(), path.resolve(process.cwd(), '..')];
  const wallets = [];
  const seen = new Set();

  // 1. Check artifacts/testnet/genesis-keys/ (has all 10 keys)
  for (const root of roots) {
    if (wallets.length >= limit) return wallets;
    loadKeysFromDir(path.join(root, 'artifacts', 'testnet', 'genesis-keys'), wallets, seen, limit);
  }

  // 2. Check data/state-*/blockchain.db/genesis-keys/ (runtime location)
  for (const root of roots) {
    if (wallets.length >= limit) return wallets;
    const dataDir = path.join(root, 'data');
    if (!fs.existsSync(dataDir)) continue;
    try {
      const stateDirs = fs.readdirSync(dataDir).filter((n) => n.startsWith('state-') || n.startsWith('matrix-sdk-state-'));
      for (const st of stateDirs) {
        if (wallets.length >= limit) break;
        loadKeysFromDir(path.join(dataDir, st, 'blockchain.db', 'genesis-keys'), wallets, seen, limit);
        loadKeysFromDir(path.join(dataDir, st, 'genesis-keys'), wallets, seen, limit);
      }
    } catch (_) { }
  }

  return wallets;
}

/**
 * Fund an account via airdrop (up to 10 LICN per call), faucet, or deployer transfer.
 * @param {string} address - Base58 address to fund
 * @param {number} amountLicn - Amount in LICN (will be split into max-10-LICN airdrops)
 * @param {string} rpcUrl - RPC endpoint URL (default: http://127.0.0.1:8899)
 * @param {string} faucetUrl - Faucet endpoint URL (default: http://127.0.0.1:9100)
 * @returns {Promise<boolean>} true if funded successfully
 */
async function fundAccount(address, amountLicn = 10, rpcUrl = 'http://127.0.0.1:8899', faucetUrl = 'http://127.0.0.1:9100') {
  const http = require('http');

  function rpcCall(method, params) {
    return new Promise((resolve, reject) => {
      const body = JSON.stringify({ jsonrpc: '2.0', id: 1, method, params: params || [] });
      const url = new URL(rpcUrl);
      const req = http.request({
        hostname: url.hostname, port: url.port, path: url.pathname, method: 'POST',
        headers: { 'Content-Type': 'application/json' }
      }, (res) => {
        let data = ''; res.on('data', (c) => data += c);
        res.on('end', () => { try { resolve(JSON.parse(data)); } catch (e) { reject(e); } });
      });
      req.on('error', reject);
      req.end(body);
    });
  }

  function faucetCall(addr, amt) {
    return new Promise((resolve, reject) => {
      const body = JSON.stringify({ address: addr, amount: amt });
      const url = new URL(faucetUrl);
      const req = http.request({
        hostname: url.hostname, port: url.port, path: '/faucet/request', method: 'POST',
        headers: { 'Content-Type': 'application/json' }
      }, (res) => {
        let data = ''; res.on('data', (c) => data += c);
        res.on('end', () => { try { resolve(JSON.parse(data)); } catch (e) { reject(e); } });
      });
      req.on('error', reject);
      req.end(body);
    });
  }

  // Strategy 1: Airdrop (max 10 LICN per call)
  let funded = 0;
  const MAX_PER_AIRDROP = 10;
  while (funded < amountLicn) {
    const chunk = Math.min(MAX_PER_AIRDROP, amountLicn - funded);
    try {
      const res = await rpcCall('requestAirdrop', [address, chunk]);
      const result = res.result || res;
      if (result && (result.success || typeof result === 'string')) {
        funded += chunk;
        if (funded < amountLicn) await new Promise(r => setTimeout(r, 200));
        continue;
      }
    } catch (_) { }
    break; // airdrop failed, try faucet
  }
  if (funded >= amountLicn) return true;

  // Strategy 2: Faucet (max 10 LICN per request)
  while (funded < amountLicn) {
    const chunk = Math.min(MAX_PER_AIRDROP, amountLicn - funded);
    try {
      const res = await faucetCall(address, chunk);
      if (res && (res.success || res.signature)) {
        funded += chunk;
        if (funded < amountLicn) await new Promise(r => setTimeout(r, 200));
        continue;
      }
    } catch (_) { }
    break;
  }

  return funded > 0;
}

module.exports = { loadFundedWallets, fundAccount, genKeypair, bs58encode, bs58decode, bytesToHex };
