'use strict';

const fs = require('fs');
const path = require('path');
const { argon2Sync, createDecipheriv, pbkdf2Sync } = require('crypto');
const pq = require('./pq-node');

const BS58 = '123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz';
const KEYPAIR_PASSWORD_ENV = 'LICHEN_KEYPAIR_PASSWORD';
const CANONICAL_ENCRYPTION_VERSION = 2;
const ARGON2_MEMORY_COST_KIB = 19_456;
const ARGON2_ITERATIONS = 2;
const ARGON2_LANES = 1;
const AES_GCM_NONCE_BYTES = 12;
const AES_GCM_TAG_BYTES = 16;

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
    if (i < 0) throw new Error(`Bad b58: ${c}`);
    num = num * 58n + BigInt(i);
  }
  const hex = num === 0n ? '' : num.toString(16);
  const padded = hex.length % 2 ? '0' + hex : hex;
  const bytes = [];
  for (let i = 0; i < padded.length; i += 2) bytes.push(parseInt(padded.slice(i, i + 2), 16));
  let lo = 0;
  for (let i = 0; i < str.length && str[i] === '1'; i++) lo++;
  const out = new Uint8Array(lo + bytes.length);
  out.set(bytes, lo);
  return out;
}

function bytesToHex(bytes) {
  return Array.from(bytes).map((value) => value.toString(16).padStart(2, '0')).join('');
}

function sameBytes(left, right) {
  if (!left || !right || left.length !== right.length) return false;
  for (let i = 0; i < left.length; i++) {
    if (left[i] !== right[i]) return false;
  }
  return true;
}

function decodeStoredBytes(value, fieldName) {
  if (Array.isArray(value)) return Buffer.from(value);
  if (typeof value === 'string') {
    const hex = value.startsWith('0x') ? value.slice(2) : value;
    if (hex.length % 2 !== 0 || !/^[0-9a-fA-F]*$/.test(hex)) {
      throw new Error(`${fieldName} must be a hex string, byte array, or buffer`);
    }
    return Buffer.from(hex, 'hex');
  }
  if (Buffer.isBuffer(value) || value instanceof Uint8Array) return Buffer.from(value);
  throw new Error(`${fieldName} must be a hex string, byte array, or buffer`);
}

function resolveKeypairPassword(password) {
  return password ?? process.env[KEYPAIR_PASSWORD_ENV] ?? null;
}

function deriveCanonicalKey(password, salt) {
  if (typeof argon2Sync !== 'function') {
    throw new Error('Node runtime does not support crypto.argon2Sync for canonical keypair decryption');
  }
  return argon2Sync('argon2id', {
    message: Buffer.from(password, 'utf8'),
    nonce: Buffer.from(salt),
    parallelism: ARGON2_LANES,
    tagLength: 32,
    memory: ARGON2_MEMORY_COST_KIB,
    passes: ARGON2_ITERATIONS,
  });
}

function decryptAes256Gcm(key, nonce, ciphertextWithTag, filePath) {
  if (nonce.length !== AES_GCM_NONCE_BYTES) {
    throw new Error(`Invalid AES-GCM nonce length in ${filePath}`);
  }
  if (ciphertextWithTag.length <= AES_GCM_TAG_BYTES) {
    throw new Error(`Encrypted keypair payload is too short in ${filePath}`);
  }

  const ciphertext = ciphertextWithTag.subarray(0, ciphertextWithTag.length - AES_GCM_TAG_BYTES);
  const authTag = ciphertextWithTag.subarray(ciphertextWithTag.length - AES_GCM_TAG_BYTES);
  const decipher = createDecipheriv('aes-256-gcm', key, nonce);
  decipher.setAuthTag(authTag);
  return Buffer.concat([decipher.update(ciphertext), decipher.final()]);
}

function maybeDeriveWallet(seed) {
  try {
    return pq.keypairFromSeed(new Uint8Array(seed));
  } catch (_) {
    return null;
  }
}

function buildWalletFromSeed(seedBytes, raw, filePath) {
  const seed = Buffer.from(seedBytes);
  if (seed.length !== 32) {
    throw new Error(`Keypair seed must be 32 bytes in ${filePath}`);
  }

  const derived = maybeDeriveWallet(seed);
  const storedPublicKey = raw.publicKey ? new Uint8Array(decodeStoredBytes(raw.publicKey, 'publicKey')) : null;
  const publicKey = storedPublicKey || derived?.publicKey;
  if (!publicKey) {
    throw new Error(`Keypair file missing publicKey and ML-DSA derivation is unavailable: ${filePath}`);
  }
  if (derived && storedPublicKey && !sameBytes(storedPublicKey, derived.publicKey)) {
    throw new Error(`Keypair file publicKey does not match derived seed in ${filePath}`);
  }

  const derivedAddress = derived?.address || pq.bs58encode(pq.publicKeyToAddressBytes(publicKey));
  const address = raw.address || raw.publicKeyBase58 || derivedAddress;
  if (address !== derivedAddress) {
    throw new Error(`Keypair file address does not match derived seed in ${filePath}`);
  }

  return {
    seed: new Uint8Array(seed),
    publicKey: new Uint8Array(publicKey),
    address,
  };
}

function parseStoredKeypair(raw, filePath = '<memory>', options = {}) {
  if (!raw || typeof raw !== 'object') {
    throw new Error(`Invalid keypair JSON in ${filePath}`);
  }

  const password = resolveKeypairPassword(options.password);

  if (raw.encrypted) {
    if (!password) {
      throw new Error(`Encrypted keypair requires ${KEYPAIR_PASSWORD_ENV} or an explicit password: ${filePath}`);
    }
    const version = Number(raw.encryption_version || 0);
    if (version !== CANONICAL_ENCRYPTION_VERSION) {
      throw new Error(`Unsupported canonical keypair encryption version ${version} in ${filePath}`);
    }
    const salt = decodeStoredBytes(raw.salt, 'salt');
    const encryptedSeed = decodeStoredBytes(raw.privateKey, 'privateKey');
    const nonce = encryptedSeed.subarray(0, AES_GCM_NONCE_BYTES);
    const ciphertextWithTag = encryptedSeed.subarray(AES_GCM_NONCE_BYTES);
    const key = deriveCanonicalKey(password, salt);
    const plaintext = decryptAes256Gcm(key, nonce, ciphertextWithTag, filePath);
    return buildWalletFromSeed(plaintext.subarray(0, 32), raw, filePath);
  }

  if (raw.encrypted_seed !== undefined) {
    if (!password) {
      throw new Error(`Encrypted keypair requires ${KEYPAIR_PASSWORD_ENV} or an explicit password: ${filePath}`);
    }
    const salt = decodeStoredBytes(raw.salt, 'salt');
    const nonce = decodeStoredBytes(raw.nonce, 'nonce');
    const ciphertext = decodeStoredBytes(raw.encrypted_seed, 'encrypted_seed');
    const authTag = decodeStoredBytes(raw.tag, 'tag');
    const key = pbkdf2Sync(Buffer.from(password, 'utf8'), salt, 600_000, 32, 'sha256');
    const plaintext = decryptAes256Gcm(key, nonce, Buffer.concat([ciphertext, authTag]), filePath);
    return buildWalletFromSeed(plaintext.subarray(0, 32), raw, filePath);
  }

  if (raw.seed !== undefined) {
    return buildWalletFromSeed(decodeStoredBytes(raw.seed, 'seed'), raw, filePath);
  }

  if (raw.privateKey !== undefined) {
    const seed = decodeStoredBytes(raw.privateKey, 'privateKey');
    if (seed.length !== 32) {
      throw new Error(`Unsupported plaintext privateKey length ${seed.length} in ${filePath}`);
    }
    return buildWalletFromSeed(seed, raw, filePath);
  }

  throw new Error(`Keypair file missing supported seed/privateKey fields in ${filePath}`);
}

function genKeypair() {
  return pq.generateKeypair();
}

function walletFromStoredKeypair(raw, options = {}) {
  try {
    return parseStoredKeypair(raw, options.filePath || '<memory>', options);
  } catch (_) {
    return null;
  }
}

function loadKeypairFile(filePath, options = {}) {
  const raw = JSON.parse(fs.readFileSync(filePath, 'utf8'));
  return {
    ...parseStoredKeypair(raw, filePath, options),
    source: filePath,
  };
}

function loadFirstMatchingKeypair(keysDir, predicate, options = {}) {
  if (!fs.existsSync(keysDir)) return null;
  const files = fs.readdirSync(keysDir).filter((file) => predicate(file)).sort();
  for (const file of files) {
    const filePath = path.join(keysDir, file);
    try {
      return loadKeypairFile(filePath, options);
    } catch (_) { }
  }
  return null;
}

function findGenesisAdminKeypair(options = {}) {
  const roots = options.roots || [process.cwd(), path.resolve(process.cwd(), '..')];
  for (const root of roots) {
    const artifactKeypair = loadFirstMatchingKeypair(
      path.join(root, 'artifacts', 'testnet', 'genesis-keys'),
      (file) => file.startsWith('genesis-primary'),
      options,
    );
    if (artifactKeypair) return artifactKeypair;

    const dataDir = path.join(root, 'data');
    if (fs.existsSync(dataDir)) {
      const stateDirs = fs.readdirSync(dataDir)
        .filter((name) => name.startsWith('state-') || name.startsWith('matrix-sdk-state-'))
        .sort();
      for (const stateDir of stateDirs) {
        for (const keysDir of [
          path.join(dataDir, stateDir, 'genesis-keys'),
          path.join(dataDir, stateDir, 'blockchain.db', 'genesis-keys'),
        ]) {
          const keypair = loadFirstMatchingKeypair(keysDir, (file) => file.startsWith('genesis-primary'), options);
          if (keypair) return keypair;
        }
      }
    }

    const keypair = loadFirstMatchingKeypair(path.join(root, 'keypairs'), (file) => file === 'deployer.json', options);
    if (keypair) return keypair;
  }

  return null;
}

function loadKeysFromDir(keysDir, wallets, seen, limit) {
  if (!fs.existsSync(keysDir)) return;
  const files = fs.readdirSync(keysDir)
    .filter((file) => file.endsWith('.json'))
    .sort((left, right) => {
      const priority = (name) => {
        if (name.startsWith('genesis-primary')) return 0;
        if (name.startsWith('genesis-signer')) return 1;
        if (name.startsWith('builder_grants')) return 2;
        if (name.startsWith('community_treasury')) return 3;
        if (name.startsWith('ecosystem_partnerships')) return 4;
        if (name.startsWith('reserve_pool')) return 5;
        if (name.startsWith('validator_rewards')) return 6;
        if (name.startsWith('founding_symbionts')) return 7;
        if (name.startsWith('treasury')) return 8;
        return 9;
      };
      return priority(left) - priority(right) || left.localeCompare(right);
    });

  for (const file of files) {
    if (wallets.length >= limit) return;
    try {
      const wallet = loadKeypairFile(path.join(keysDir, file));
      if (seen.has(wallet.address)) continue;
      seen.add(wallet.address);
      wallets.push(wallet);
    } catch (_) { }
  }
}

function loadFundedWallets(limit = 2) {
  const roots = [process.cwd(), path.resolve(process.cwd(), '..')];
  const wallets = [];
  const seen = new Set();

  for (const root of roots) {
    if (wallets.length >= limit) return wallets;
    loadKeysFromDir(path.join(root, 'artifacts', 'testnet', 'genesis-keys'), wallets, seen, limit);
  }

  for (const root of roots) {
    if (wallets.length >= limit) return wallets;
    const dataDir = path.join(root, 'data');
    if (!fs.existsSync(dataDir)) continue;
    try {
      const stateDirs = fs.readdirSync(dataDir).filter((name) => name.startsWith('state-') || name.startsWith('matrix-sdk-state-'));
      for (const stateDir of stateDirs) {
        if (wallets.length >= limit) break;
        loadKeysFromDir(path.join(dataDir, stateDir, 'blockchain.db', 'genesis-keys'), wallets, seen, limit);
        loadKeysFromDir(path.join(dataDir, stateDir, 'genesis-keys'), wallets, seen, limit);
      }
    } catch (_) { }
  }

  for (const root of roots) {
    if (wallets.length >= limit) return wallets;
    try {
      const deployer = loadKeypairFile(path.join(root, 'keypairs', 'deployer.json'));
      if (!seen.has(deployer.address)) {
        seen.add(deployer.address);
        wallets.push(deployer);
      }
    } catch (_) { }
  }

  return wallets;
}

async function fundAccount(address, amountLicn = 10, rpcUrl = 'http://127.0.0.1:8899', faucetUrl = 'http://127.0.0.1:9100') {
  const http = require('http');

  function rpcCall(method, params) {
    return new Promise((resolve, reject) => {
      const body = JSON.stringify({ jsonrpc: '2.0', id: 1, method, params: params || [] });
      const url = new URL(rpcUrl);
      const req = http.request({
        hostname: url.hostname,
        port: url.port,
        path: url.pathname,
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
      }, (res) => {
        let data = '';
        res.on('data', (chunk) => data += chunk);
        res.on('end', () => {
          try {
            resolve(JSON.parse(data));
          } catch (error) {
            reject(error);
          }
        });
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
        hostname: url.hostname,
        port: url.port,
        path: '/faucet/request',
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
      }, (res) => {
        let data = '';
        res.on('data', (chunk) => data += chunk);
        res.on('end', () => {
          try {
            resolve(JSON.parse(data));
          } catch (error) {
            reject(error);
          }
        });
      });
      req.on('error', reject);
      req.end(body);
    });
  }

  let funded = 0;
  const maxPerAirdrop = 10;
  while (funded < amountLicn) {
    const chunk = Math.min(maxPerAirdrop, amountLicn - funded);
    try {
      const res = await rpcCall('requestAirdrop', [address, chunk]);
      const result = res.result || res;
      if (result && (result.success || typeof result === 'string')) {
        funded += chunk;
        if (funded < amountLicn) await new Promise((resolve) => setTimeout(resolve, 200));
        continue;
      }
    } catch (_) { }
    break;
  }
  if (funded >= amountLicn) return true;

  while (funded < amountLicn) {
    const chunk = Math.min(maxPerAirdrop, amountLicn - funded);
    try {
      const res = await faucetCall(address, chunk);
      if (res && (res.success || res.signature)) {
        funded += chunk;
        if (funded < amountLicn) await new Promise((resolve) => setTimeout(resolve, 200));
        continue;
      }
    } catch (_) { }
    break;
  }

  return funded > 0;
}

module.exports = {
  loadFundedWallets,
  fundAccount,
  genKeypair,
  bs58encode,
  bs58decode,
  bytesToHex,
  initCrypto: pq.init.bind(pq),
  loadKeypairFile,
  findGenesisAdminKeypair,
};