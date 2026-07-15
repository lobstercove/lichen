#!/usr/bin/env node

const assert = require('assert');
const fs = require('fs');
const os = require('os');
const path = require('path');
const esbuild = require('esbuild');

const repoRoot = path.join(__dirname, '..', '..');
const providerRouterPath = path.join(repoRoot, 'wallet', 'extension', 'src', 'core', 'provider-router.js');
const cryptoServicePath = path.join(repoRoot, 'wallet', 'extension', 'src', 'core', 'crypto-service.js');
const txServicePath = path.join(repoRoot, 'wallet', 'extension', 'src', 'core', 'tx-service.js');

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
    import { handleProviderRequest, finalizePendingRequest, consumeFinalizedResult, getPendingRequest } from ${JSON.stringify(providerRouterPath)};
    import { encryptPrivateKey, privateKeyToKeypair, base58Decode } from ${JSON.stringify(cryptoServicePath)};
    import { buildNativeTransferMessage, buildAmountInstructionData } from ${JSON.stringify(txServicePath)};
    export {
      handleProviderRequest,
      finalizePendingRequest,
      consumeFinalizedResult,
      getPendingRequest,
      encryptPrivateKey,
      privateKeyToKeypair,
      base58Decode,
      buildNativeTransferMessage,
      buildAmountInstructionData
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
  const originalChrome = globalThis.chrome;
  const storageData = {};
  globalThis.chrome = {
    storage: {
      local: {
        async get(key) {
          if (typeof key === 'string') {
            return Object.prototype.hasOwnProperty.call(storageData, key) ? { [key]: storageData[key] } : {};
          }
          if (Array.isArray(key)) {
            return key.reduce((out, entry) => {
              if (Object.prototype.hasOwnProperty.call(storageData, entry)) out[entry] = storageData[entry];
              return out;
            }, {});
          }
          return { ...storageData };
        },
        async set(values) {
          Object.assign(storageData, values || {});
        }
      }
    }
  };
  globalThis.fetch = async (_url, options) => {
    const body = JSON.parse(String(options?.body || '{}'));
    if (body.method === 'getNetworkInfo') {
      return { json: async () => ({ jsonrpc: '2.0', id: body.id, result: { chain_id: 'lichen-testnet-1' } }) };
    }
    throw new Error(`signTransaction E2E must not call ${body.method || 'unknown RPC'}`);
  };

  try {
    const {
      handleProviderRequest,
      finalizePendingRequest,
      consumeFinalizedResult,
      getPendingRequest,
      encryptPrivateKey,
      privateKeyToKeypair,
      base58Decode,
      buildNativeTransferMessage,
      buildAmountInstructionData
    } = await loadWalletRuntime();

    const password = 'rg-703-signing-e2e-password';
    const privateKeyHex = '11'.repeat(32);
    const keypair = await privateKeyToKeypair(privateKeyHex);
    const encryptedKey = await encryptPrivateKey(privateKeyHex, password);
    const amountMessage = buildNativeTransferMessage(
      keypair.address,
      keypair.address,
      '1.000000001',
      bytes(32, 0xaa).reduce((hex, byte) => hex + byte.toString(16).padStart(2, '0'), '')
    );
    const nativeTransferData = Uint8Array.from(amountMessage.instructions[0].data);
    assert.strictEqual(
      new DataView(nativeTransferData.buffer, nativeTransferData.byteOffset, nativeTransferData.byteLength).getBigUint64(1, true),
      1_000_000_001n,
      'native transfer amount parser should preserve 9-decimal base units'
    );
    assert.throws(
      () => buildNativeTransferMessage(keypair.address, keypair.address, '1.0000000001', amountMessage.blockhash),
      /at most 9 decimal places/,
      'native transfer amount parser should reject over-precision'
    );
    const stakingData = buildAmountInstructionData(13, '2.000000001', 2);
    assert.strictEqual(new DataView(stakingData.buffer).getBigUint64(1, true), 2_000_000_001n, 'staking amount parser should preserve base units');
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
    const pendingRequest = getPendingRequest(pending.requestId);
    assert.strictEqual(pendingRequest.transactionIntent.intent, 'Unknown transaction', 'governance proposal should not be mislabelled as a transfer');
    assert.match(
      pendingRequest.transactionIntent.warnings.join(' '),
      /System opcode 34 is not decoded|administrative/i,
      'unknown system opcode should carry an administrative warning'
    );

    const finalized = await finalizePendingRequest(pending.requestId, true, context, { password });
    assert.strictEqual(finalized.ok, true, 'approval finalization should succeed');

    const signed = consumeFinalizedResult(pending.requestId);
    assert.strictEqual(signed.ok, true, 'signed result should be finalized successfully');
    assert.strictEqual(signed.result.sourceTransactionFormat, 'lichen_tx_v1');
    assert.strictEqual(signed.result.signedTransactionFormat, 'lichen_tx_v1');
    assert.ok(signed.result.signature, 'signature should be returned');
    assert.ok(signed.result.signedTransactionBase64, 'signed transaction base64 should be returned');

    const signedWire = Buffer.from(signed.result.signedTransactionBase64, 'base64');
    assert.deepStrictEqual(Array.from(signedWire.subarray(0, 4)), [0x4d, 0x54, 0x01, 0x00]);
    const signedTx = signed.result.signedTransaction;
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
      if (body.method === 'getNetworkInfo') {
        return { json: async () => ({ jsonrpc: '2.0', id: body.id, result: { chain_id: 'lichen-testnet-1' } }) };
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
    const blockedPendingRequest = getPendingRequest(blockedPending.requestId);
    assert.strictEqual(blockedPendingRequest.transactionIntent.intent, 'Native transfer', 'native transfer intent should be decoded');
    assert.strictEqual(blockedPendingRequest.transactionIntent.amount, '1.0 LICN', 'native transfer amount should be decoded in LICN');
    assert.strictEqual(blockedPendingRequest.transactionIntent.tokenDecimals, '9', 'native LICN decimals should be visible');
    assert.strictEqual(blockedPendingRequest.transactionIntent.network, 'local-testnet', 'transaction intent should include network');
    assert.strictEqual(blockedPendingRequest.transactionIntent.rpc, 'http://localhost:8899', 'transaction intent should include RPC endpoint');
    assert.ok(blockedPendingRequest.transactionIntent.destination, 'native transfer destination should be visible');

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

    storageData.lichenWalletState = {
      schemaVersion: 1,
      wallets: [],
      activeWalletId: null,
      isLocked: true,
      settings: { currency: 'USD', lockTimeout: 300000 },
      network: { selected: 'local-testnet' }
    };
    const networkContext = {
      origin: 'https://dex.lichen.network',
      network: 'local-testnet',
      hasWallet: false,
      isLocked: true,
      activeAddress: null,
      activeWallet: null
    };

    const switchPending = await handleProviderRequest({
      method: 'wallet_switchEthereumChain',
      params: [{ chainId: '0x2711' }]
    }, networkContext);
    assert.strictEqual(switchPending.ok, true, 'wallet_switchEthereumChain should be accepted');
    assert.strictEqual(switchPending.pending, true, 'wallet_switchEthereumChain should require approval');
    assert.strictEqual(storageData.lichenWalletState.network.selected, 'local-testnet', 'network switch must not mutate before approval');

    await finalizePendingRequest(switchPending.requestId, true, networkContext);
    const switchResult = consumeFinalizedResult(switchPending.requestId);
    assert.strictEqual(switchResult.ok, true, 'approved network switch should finalize successfully');
    assert.strictEqual(storageData.lichenWalletState.network.selected, 'testnet', 'approved network switch should mutate selected network');

    storageData.lichenWalletState.network.selected = 'local-testnet';
    const addPending = await handleProviderRequest({
      method: 'wallet_addEthereumChain',
      params: [{
        chainId: '0x2711',
        rpcUrls: ['https://custom-testnet.lichen.network/rpc']
      }]
    }, networkContext);
    assert.strictEqual(addPending.ok, true, 'wallet_addEthereumChain should be accepted');
    assert.strictEqual(addPending.pending, true, 'wallet_addEthereumChain should require approval');
    assert.strictEqual(storageData.lichenWalletState.network.selected, 'local-testnet', 'addNetwork must not mutate before approval');
    assert.strictEqual(storageData.lichenWalletState.settings.testnetRPC, undefined, 'addNetwork must not save RPC before approval');

    await finalizePendingRequest(addPending.requestId, true, networkContext);
    const addResult = consumeFinalizedResult(addPending.requestId);
    assert.strictEqual(addResult.ok, true, 'approved addNetwork should finalize successfully');
    assert.strictEqual(storageData.lichenWalletState.network.selected, 'testnet', 'approved addNetwork should switch selected network');
    assert.strictEqual(
      storageData.lichenWalletState.settings.testnetRPC,
      'https://custom-testnet.lichen.network/rpc',
      'approved addNetwork should persist the requested RPC endpoint'
    );

    storageData.lichenWalletState.network.selected = 'local-testnet';
    const rejectedPending = await handleProviderRequest({
      method: 'licn_switchNetwork',
      params: [{ chainId: '0x2711' }]
    }, networkContext);
    await finalizePendingRequest(rejectedPending.requestId, false, networkContext);
    const rejectedResult = consumeFinalizedResult(rejectedPending.requestId);
    assert.strictEqual(rejectedResult.ok, false, 'rejected network switch should finalize as rejected');
    assert.strictEqual(storageData.lichenWalletState.network.selected, 'local-testnet', 'rejected network switch must not mutate state');
  } finally {
    globalThis.fetch = originalFetch;
    globalThis.chrome = originalChrome;
  }
}

run()
  .then(() => {
    console.log('Wallet extension RG-703/RG-707/RG-708 signing/provider E2E: 9 passed, 0 failed');
  })
  .catch((error) => {
    console.error(`Wallet extension RG-703/RG-707 signing E2E failed: ${error.stack || error.message}`);
    process.exitCode = 1;
  });
