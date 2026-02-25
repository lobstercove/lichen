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

function loadFundedWallets(limit = 2) {
  const roots = [
    process.cwd(),
    path.resolve(process.cwd(), '..'),
  ];
  const stateDirs = ['state-8000', 'state-8001', 'state-8002', 'state-testnet-7001', 'state-testnet-7002', 'state-testnet-7003'];
  const wallets = [];

  for (const root of roots) {
    for (const st of stateDirs) {
      const keysDir = path.join(root, 'data', st, 'genesis-keys');
      if (!fs.existsSync(keysDir)) continue;
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
            return 8;
          };
          const aPrio = priority(a);
          const bPrio = priority(b);
          return aPrio - bPrio || a.localeCompare(b);
        });

      for (const file of files) {
        if (wallets.length >= limit) return wallets;
        try {
          const raw = JSON.parse(fs.readFileSync(path.join(keysDir, file), 'utf8'));
          const hex = raw.secret_key;
          if (typeof hex !== 'string' || hex.length !== 64) continue;
          const seed = Uint8Array.from(Buffer.from(hex, 'hex'));
          const kp = nacl.sign.keyPair.fromSeed(seed);
          wallets.push({
            publicKey: kp.publicKey,
            secretKey: kp.secretKey,
            address: bs58encode(kp.publicKey),
            source: path.join(keysDir, file),
          });
        } catch (_) {}
      }
    }
  }

  return wallets;
}

module.exports = { loadFundedWallets };
