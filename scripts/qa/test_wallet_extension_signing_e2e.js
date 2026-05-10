#!/usr/bin/env node

const assert = require('assert');
const fs = require('fs');
const os = require('os');
const path = require('path');
const esbuild = require('esbuild');

const repoRoot = path.join(__dirname, '..', '..');
const providerRouterPath = path.join(repoRoot, 'wallet', 'extension', 'src', 'core', 'provider-router.js');
const cryptoServicePath = path.join(repoRoot, 'wallet', 'extension', 'src', 'core', 'crypto-service.js');

function writeU64LE(parts, value) {
  const buf = new ArrayBuffer(8);
  new DataView(buf).setBigUint64(0, BigInt(value), true);
  parts.push(new Uint8Array(buf));
}

function writeU32LE(parts, value) {
  const buf = new ArrayBuffer(4);
  new DataView(buf).setUint32(0, value, true);
  parts.push(new Uint8Array(buf));
}

function bytes(length, fill) {
  return new Uint8Array(Array.from({ length }, (_, index) => (fill + index) & 0xff));
}

function concat(parts) {
  const total = parts.reduce((sum, part) => sum + part.length, 0);
  const out = new Uint8Array(total);
  let offset = 0;
  for (const part of parts) {
    out.set(part, offset);
    offset += part.length;
  }
  return out;
}

function toBase64(bytesValue) {
  return Buffer.from(bytesValue).toString('base64');
}

function fromBase64Json(encoded) {
  return JSON.parse(Buffer.from(encoded, 'base64').toString('utf8'));
}

function buildUnsignedRestrictionWireTx() {
  const parts = [];
  parts.push(Uint8Array.from([0x4d, 0x54, 0x01, 0x00])); // magic, version, native tx type

  writeU64LE(parts, 0); // signatures

  writeU64LE(parts, 1); // instructions
  parts.push(new Uint8Array(32)); // governance/system program id placeholder
  writeU64LE(parts, 2); // accounts
  parts.push(bytes(32, 0x11));
  parts.push(bytes(32, 0x41));

  const restrictionGovernanceProposalData = Uint8Array.from([
    34, // existing governance proposal instruction opcode
    10, // append-only restriction subtype used by RG builders
    0x72, 0x67, 0x2d, 0x37, 0x30, 0x33
  ]);
  writeU64LE(parts, restrictionGovernanceProposalData.length);
  parts.push(restrictionGovernanceProposalData);

  parts.push(bytes(32, 0xaa)); // recent blockhash
  parts.push(Uint8Array.from([0x00])); // compute_budget None
  parts.push(Uint8Array.from([0x00])); // compute_unit_price None
  writeU32LE(parts, 0); // payload tx_type native

  return toBase64(concat(parts));
}

function buildUnsignedNativeTransferWireTx(fromPubkeyBytes, toPubkeyBytes, spores) {
  const parts = [];
  parts.push(Uint8Array.from([0x4d, 0x54, 0x01, 0x00])); // magic, version, native tx type

  writeU64LE(parts, 0); // signatures

  writeU64LE(parts, 1); // instructions
  parts.push(new Uint8Array(32)); // system program id
  writeU64LE(parts, 2); // accounts
  parts.push(fromPubkeyBytes);
  parts.push(toPubkeyBytes);

  const data = new Uint8Array(9);
  data[0] = 0;
  new DataView(data.buffer).setBigUint64(1, BigInt(spores), true);
  writeU64LE(parts, data.length);
  parts.push(data);

  parts.push(bytes(32, 0xbb)); // recent blockhash
  parts.push(Uint8Array.from([0x00])); // compute_budget None
  parts.push(Uint8Array.from([0x00])); // compute_unit_price None
  writeU32LE(parts, 0); // payload tx_type native

  return toBase64(concat(parts));
}

async function loadWalletRuntime() {
  const tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), 'lichen-rg-signing-e2e-'));
  const entryPath = path.join(tmpDir, 'entry.mjs');
  const bundlePath = path.join(tmpDir, 'bundle.cjs');
  fs.writeFileSync(entryPath, `
    import { handleProviderRequest, finalizePendingRequest, consumeFinalizedResult } from ${JSON.stringify(providerRouterPath)};
    import { encryptPrivateKey, privateKeyToKeypair, base58Decode } from ${JSON.stringify(cryptoServicePath)};
    export {
      handleProviderRequest,
      finalizePendingRequest,
      consumeFinalizedResult,
      encryptPrivateKey,
      privateKeyToKeypair,
      base58Decode
    };
  `);

  esbuild.buildSync({
    entryPoints: [entryPath],
    outfile: bundlePath,
    bundle: true,
    platform: 'node',
    format: 'cjs',
    logLevel: 'silent'
  });

  return require(bundlePath);
}

async function run() {
  if (typeof globalThis.atob !== 'function') {
    globalThis.atob = (value) => Buffer.from(value, 'base64').toString('binary');
  }
  if (typeof globalThis.btoa !== 'function') {
    globalThis.btoa = (value) => Buffer.from(value, 'binary').toString('base64');
  }

  const originalFetch = globalThis.fetch;
  globalThis.fetch = async () => {
    throw new Error('signTransaction E2E must not submit or call RPC');
  };

  try {
    const {
      handleProviderRequest,
      finalizePendingRequest,
      consumeFinalizedResult,
      encryptPrivateKey,
      privateKeyToKeypair,
      base58Decode
    } = await loadWalletRuntime();

    const password = 'rg-703-signing-e2e-password';
    const privateKeyHex = '11'.repeat(32);
    const keypair = await privateKeyToKeypair(privateKeyHex);
    const encryptedKey = await encryptPrivateKey(privateKeyHex, password);
    const unsignedWireTx = buildUnsignedRestrictionWireTx();
    const context = {
      origin: null,
      network: 'local-testnet',
      hasWallet: true,
      isLocked: false,
      activeAddress: keypair.address,
      activeWallet: {
        address: keypair.address,
        encryptedKey
      }
    };

    const pending = await handleProviderRequest({
      method: 'licn_signTransaction',
      params: [{ transaction: unsignedWireTx }]
    }, context);
    assert.strictEqual(pending.ok, true, 'signTransaction request should be accepted');
    assert.strictEqual(pending.pending, true, 'signTransaction should require approval');
    assert.ok(pending.requestId, 'signTransaction should return a pending request id');

    const finalized = await finalizePendingRequest(pending.requestId, true, context, { password });
    assert.strictEqual(finalized.ok, true, 'approval finalization should succeed');

    const signed = consumeFinalizedResult(pending.requestId);
    assert.strictEqual(signed.ok, true, 'signed result should be finalized successfully');
    assert.strictEqual(signed.result.sourceTransactionFormat, 'lichen_tx_v1');
    assert.strictEqual(signed.result.signedTransactionFormat, 'wallet_json_base64');
    assert.ok(signed.result.signature, 'signature should be returned');
    assert.ok(signed.result.signedTransactionBase64, 'signed transaction base64 should be returned');

    const signedTx = fromBase64Json(signed.result.signedTransactionBase64);
    assert.strictEqual(signedTx.tx_type, 'native');
    assert.strictEqual(signedTx.message.blockhash, bytes(32, 0xaa).reduce((hex, byte) => hex + byte.toString(16).padStart(2, '0'), ''));
    assert.deepStrictEqual(signedTx.message.instructions[0].data.slice(0, 2), [34, 10]);
    assert.strictEqual(signedTx.signatures.length, 1);
    assert.strictEqual(signedTx.signatures[0].public_key.scheme_version, 1);
    assert.ok(signedTx.signatures[0].sig.length > 32, 'PQ signature should be populated');

    const rpcRequests = [];
    globalThis.fetch = async (url, options) => {
      const body = JSON.parse(String(options?.body || '{}'));
      rpcRequests.push({ url: String(url || ''), body });
      if (body.method === 'getRestrictionStatus') {
        return {
          json: async () => ({
            jsonrpc: '2.0',
            id: body.id,
            result: {
              active: false,
              target: body.params?.[0] || null,
              active_restriction_ids: []
            }
          })
        };
      }
      if (body.method === 'getContractLifecycleStatus') {
        return {
          json: async () => ({
            jsonrpc: '2.0',
            id: body.id,
            result: {
              contract: body.params?.[0] || null,
              lifecycle_status: 'active',
              blocked: false,
              active_restriction_ids: []
            }
          })
        };
      }
      if (body.method === 'getIncidentStatus') {
        return { json: async () => ({ jsonrpc: '2.0', id: body.id, result: { mode: 'normal', severity: 'info' } }) };
      }
      if (body.method === 'canTransfer') {
        const transfer = body.params?.[0] || {};
        if (Number(transfer.amount || 0) === 25) {
          return {
            json: async () => ({
              jsonrpc: '2.0',
              id: body.id,
              result: {
                allowed: true,
                blocked: false,
                active_restriction_ids: []
              }
            })
          };
        }
        return {
          json: async () => ({
            jsonrpc: '2.0',
            id: body.id,
            result: {
              allowed: false,
              blocked: true,
              active: true,
              active_restriction_ids: [77],
              active_restrictions: [{ id: 77, mode: 'outgoing_only' }]
            }
          })
        };
      }
      if (body.method === 'sendTransaction') {
        throw new Error('blocked transfer preflight must not broadcast');
      }
      throw new Error(`unexpected RPC method in signing E2E: ${body.method}`);
    };

    const fromBytes = base58Decode(keypair.address);
    const toBytes = bytes(32, 0x77);
    const restrictedTransfer = buildUnsignedNativeTransferWireTx(fromBytes, toBytes, 1_000_000_000);
    const blockedPending = await handleProviderRequest({
      method: 'licn_signTransaction',
      params: [{ transaction: restrictedTransfer }]
    }, context);
    assert.strictEqual(blockedPending.ok, true, 'restricted transfer signing request should still require approval');
    assert.strictEqual(blockedPending.pending, true, 'restricted transfer should be represented as a pending approval');

    const blockedFinalized = await finalizePendingRequest(blockedPending.requestId, true, context, {
      password: 'intentionally-wrong-password'
    });
    assert.strictEqual(blockedFinalized.ok, true, 'approval finalization wrapper should complete');

    const blocked = consumeFinalizedResult(blockedPending.requestId);
    assert.strictEqual(blocked.ok, false, 'restricted transfer must not be signed');
    assert.match(blocked.error, /consensus restriction #77|restriction #77/i);
    assert.ok(rpcRequests.some((request) => request.body.method === 'getIncidentStatus'), 'restricted transfer should check incident status');
    assert.ok(rpcRequests.some((request) => request.body.method === 'canTransfer'), 'restricted transfer should call canTransfer');
    assert.ok(!rpcRequests.some((request) => request.body.method === 'sendTransaction'), 'restricted signTransaction must not broadcast');

    const dappReadOnlyContext = {
      origin: null,
      network: 'local-testnet',
      hasWallet: false,
      isLocked: true,
      activeAddress: null,
      activeWallet: null
    };
    const beforeDappRpcCount = rpcRequests.length;

    const transferPreflight = await handleProviderRequest({
      method: 'lichen_canTransfer',
      params: [{
        from: keypair.address,
        to: keypair.address,
        asset: 'native',
        amount: 25n
      }]
    }, dappReadOnlyContext);
    assert.strictEqual(transferPreflight.ok, true, 'dapp canTransfer preflight should succeed without wallet approval');
    assert.strictEqual(transferPreflight.result.allowed, true, 'dapp canTransfer should return RPC result');

    const targetStatus = await handleProviderRequest({
      method: 'lichen_getRestrictionStatus',
      params: [{ type: 'account', account: keypair.address }]
    }, dappReadOnlyContext);
    assert.strictEqual(targetStatus.ok, true, 'dapp getRestrictionStatus should succeed');
    assert.deepStrictEqual(targetStatus.result.target, { type: 'account', account: keypair.address });

    const lifecycle = await handleProviderRequest({
      method: 'lichen_getContractLifecycleStatus',
      params: [keypair.address]
    }, dappReadOnlyContext);
    assert.strictEqual(lifecycle.ok, true, 'dapp getContractLifecycleStatus should succeed');
    assert.strictEqual(lifecycle.result.lifecycle_status, 'active');

    const dappRpcRequests = rpcRequests.slice(beforeDappRpcCount);
    assert.deepStrictEqual(
      dappRpcRequests.map((request) => request.body.method),
      ['canTransfer', 'getRestrictionStatus', 'getContractLifecycleStatus'],
      'dapp restriction helper methods should map to canonical read-only RPC methods'
    );
    assert.strictEqual(dappRpcRequests[0].body.params[0].amount, '25', 'dapp BigInt transfer amounts should be serialized safely');
    for (const request of dappRpcRequests) {
      assert.strictEqual(request.url, 'http://localhost:8899', 'dapp restriction methods must use trusted local-testnet RPC');
    }
  } finally {
    globalThis.fetch = originalFetch;
  }
}

run()
  .then(() => {
    console.log('Wallet extension RG-703/RG-707/RG-708 signing/provider E2E: 5 passed, 0 failed');
  })
  .catch((error) => {
    console.error(`Wallet extension RG-703/RG-707 signing E2E failed: ${error.stack || error.message}`);
    process.exitCode = 1;
  });
