import { decryptPrivateKey, signTransaction, bytesToHex } from './crypto-service.js';
import { MoltChainRPC, getConfiguredRpcEndpoint } from './rpc-service.js';
import { patchState } from './state-store.js';
import { serializeMessageForSigning } from './tx-service.js';

const APPROVED_ORIGINS_KEY = 'moltApprovedOrigins';
const APPROVED_ORIGINS_META_KEY = 'moltApprovedOriginsMeta';
const APPROVED_ORIGIN_TTL_MS = 30 * 24 * 60 * 60 * 1000;
const pendingRequests = new Map();
const MAX_PENDING_REQUESTS = 200;
const PENDING_REQUEST_TTL_MS = 3 * 60 * 1000;
const FINALIZED_REQUEST_TTL_MS = 5 * 60 * 1000;

function getNetworkMeta(network = 'local-testnet') {
  const value = String(network || 'local-testnet').trim();
  if (value === 'mainnet') return { chainHex: '0x2710', netVersion: '10000' };
  if (value === 'testnet') return { chainHex: '0x2711', netVersion: '10001' };
  return { chainHex: '0x539', netVersion: '1337' };
}

function networkFromAnyChainId(chainIdInput) {
  const value = String(chainIdInput || '').trim().toLowerCase();
  const normalized = value.startsWith('0x') ? value : `0x${value}`;
  if (normalized === '0x2710') return 'mainnet';
  if (normalized === '0x2711') return 'testnet';
  if (normalized === '0x539') return 'local-testnet';
  return null;
}

function toHexQuantity(value) {
  const bigint = BigInt(Math.max(0, Math.floor(Number(value || 0))));
  return `0x${bigint.toString(16)}`;
}

function prunePendingRequests(now = Date.now()) {
  for (const [requestId, request] of pendingRequests.entries()) {
    if (request?.finalized) {
      const finalizedAt = Number(request?.finalizedAt || request?.createdAt || 0);
      if (finalizedAt > 0 && now - finalizedAt > FINALIZED_REQUEST_TTL_MS) {
        pendingRequests.delete(requestId);
      }
      continue;
    }
    const createdAt = Number(request?.createdAt || 0);
    if (createdAt <= 0) continue;
    if (now - createdAt <= PENDING_REQUEST_TTL_MS) continue;
    request.finalized = { ok: false, error: 'Approval timed out' };
    request.finalizedAt = now;
  }
}

function makeRequestId() {
  return crypto.randomUUID();
}

async function loadApprovedOrigins() {
  const { origins } = await pruneApprovedOrigins();
  return origins;
}

async function saveApprovedOrigins(origins) {
  await chrome.storage.local.set({
    [APPROVED_ORIGINS_KEY]: Array.from(new Set(origins))
  });
}

async function loadApprovedOriginsMeta() {
  const result = await chrome.storage.local.get(APPROVED_ORIGINS_META_KEY);
  const meta = result?.[APPROVED_ORIGINS_META_KEY];
  return meta && typeof meta === 'object' ? meta : {};
}

async function saveApprovedOriginsMeta(meta) {
  await chrome.storage.local.set({
    [APPROVED_ORIGINS_META_KEY]: meta && typeof meta === 'object' ? meta : {}
  });
}

async function pruneApprovedOrigins(now = Date.now()) {
  const [origins, meta] = await Promise.all([loadApprovedOriginsRaw(), loadApprovedOriginsMeta()]);
  const nextMeta = { ...meta };
  const activeOrigins = [];
  let changed = false;

  const seen = new Set();
  for (const entry of origins) {
    const origin = String(entry || '').trim();
    if (!origin || seen.has(origin)) {
      changed = true;
      continue;
    }
    seen.add(origin);
    const expiresAt = Number(nextMeta[origin] || 0);
    if (expiresAt > 0 && expiresAt <= now) {
      delete nextMeta[origin];
      changed = true;
      continue;
    }
    activeOrigins.push(origin);
  }

  for (const origin of Object.keys(nextMeta)) {
    if (!seen.has(origin)) {
      delete nextMeta[origin];
      changed = true;
    }
  }

  if (changed) {
    await Promise.all([
      saveApprovedOrigins(activeOrigins),
      saveApprovedOriginsMeta(nextMeta)
    ]);
  }

  return { origins: activeOrigins, meta: nextMeta };
}

async function loadApprovedOriginsRaw() {
  const result = await chrome.storage.local.get(APPROVED_ORIGINS_KEY);
  const list = result?.[APPROVED_ORIGINS_KEY];
  return Array.isArray(list) ? list : [];
}

async function isOriginApproved(origin) {
  if (!origin) return false;
  const origins = await loadApprovedOrigins();
  return origins.includes(origin);
}

async function approveOrigin(origin) {
  if (!origin) return;
  const { origins, meta } = await pruneApprovedOrigins();
  if (!origins.includes(origin)) {
    origins.push(origin);
  }
  meta[origin] = Date.now() + APPROVED_ORIGIN_TTL_MS;
  await Promise.all([
    saveApprovedOrigins(origins),
    saveApprovedOriginsMeta(meta)
  ]);
}

async function revokeOrigin(origin) {
  if (!origin) return;
  const { origins, meta } = await pruneApprovedOrigins();
  const next = origins.filter((entry) => entry !== origin);
  delete meta[origin];
  await Promise.all([
    saveApprovedOrigins(next),
    saveApprovedOriginsMeta(meta)
  ]);
}

export async function listApprovedOrigins() {
  return loadApprovedOrigins();
}

export async function revokeApprovedOrigin(origin) {
  await revokeOrigin(origin);
  return true;
}

export function listPendingRequests(limit = 20) {
  prunePendingRequests();

  const items = Array.from(pendingRequests.values())
    .filter((entry) => !entry.finalized)
    .sort((a, b) => Number(b.createdAt || 0) - Number(a.createdAt || 0))
    .slice(0, Math.max(1, Math.min(200, Number(limit || 20))))
    .map((entry) => ({
      requestId: entry.requestId,
      method: normalizeMethod(entry.payload?.method || null),
      origin: entry.origin || null,
      createdAt: entry.createdAt || Date.now()
    }));

  return items;
}

function getPendingRequest(requestId) {
  prunePendingRequests();
  const request = pendingRequests.get(requestId) || null;
  if (!request || request.finalized) return null;
  return request;
}

function consumeFinalizedResult(requestId) {
  prunePendingRequests();
  const request = pendingRequests.get(requestId);
  if (!request || !request.finalized) return null;
  pendingRequests.delete(requestId);
  return request.finalized;
}

function createPendingRequest(payload, context) {
  prunePendingRequests();

  if (pendingRequests.size >= MAX_PENDING_REQUESTS) {
    const oldest = Array.from(pendingRequests.values())
      .sort((a, b) => Number(a?.createdAt || 0) - Number(b?.createdAt || 0))[0];
    if (oldest?.requestId) {
      pendingRequests.delete(oldest.requestId);
    }
  }

  const requestId = makeRequestId();
  pendingRequests.set(requestId, {
    requestId,
    payload,
    origin: context.origin || null,
    createdAt: Date.now(),
    finalized: null
  });
  return requestId;
}

function normalizeParams(payload) {
  const params = payload?.params;
  if (Array.isArray(params)) {
    if (params.length === 1 && typeof params[0] === 'object' && params[0] !== null) {
      return params[0];
    }
    return { args: params };
  }
  return params || {};
}

function normalizeMethod(rawMethod) {
  const method = String(rawMethod || '').trim();
  const aliasMap = {
    molt_getAccounts: 'molt_accounts',
    molt_request_accounts: 'molt_requestAccounts',
    molt_sign_message: 'molt_signMessage',
    molt_sign_transaction: 'molt_signTransaction',
    molt_send_transaction: 'molt_sendTransaction',
    molt_get_transactions: 'molt_getTransactions',
    molt_get_transactions_by_address: 'molt_getTransactions',
    molt_latest_block: 'molt_getLatestBlock',
    molt_get_provider_state: 'molt_getProviderState',
    molt_is_connected: 'molt_isConnected',
    eth_accounts: 'molt_accounts',
    eth_requestAccounts: 'molt_requestAccounts',
    personal_sign: 'molt_signMessage',
    eth_sign: 'molt_signMessage',
    eth_signTransaction: 'molt_signTransaction',
    eth_sendTransaction: 'molt_sendTransaction',
    eth_getBalance: 'molt_getBalance',
    eth_getTransactionCount: 'molt_getTransactions',
    eth_chainId: 'molt_ethChainId',
    net_version: 'molt_netVersion',
    eth_coinbase: 'molt_coinbase',
    molt_connect: 'molt_requestAccounts',
    wallet_getPermissions: 'molt_permissions',
    wallet_requestPermissions: 'molt_requestAccounts',
    wallet_revokePermissions: 'molt_disconnect',
    molt_getPermissions: 'molt_permissions',
    wallet_switchEthereumChain: 'molt_switchNetwork',
    wallet_addEthereumChain: 'molt_addNetwork',
    wallet_watchAsset: 'molt_watchAsset',
    eth_blockNumber: 'molt_blockNumber',
    eth_getCode: 'molt_getCode',
    eth_estimateGas: 'molt_estimateGas',
    eth_gasPrice: 'molt_gasPrice',
    web3_clientVersion: 'molt_clientVersion',
    net_listening: 'molt_netListening'
  };
  return aliasMap[method] || method;
}

function getAddressFromParams(params, connectedAddress) {
  if (params?.address && typeof params.address === 'string') {
    return params.address;
  }

  if (Array.isArray(params?.args)) {
    const candidate = params.args[0];
    if (typeof candidate === 'string' && candidate.length > 0) {
      return candidate;
    }
  }

  return connectedAddress;
}

function getTransactionFromParams(params) {
  if (params?.transaction) return params.transaction;
  if (params?.tx) return params.tx;
  if (params?.unsignedTransaction) return params.unsignedTransaction;

  if (Array.isArray(params?.args) && params.args.length > 0) {
    return params.args[0];
  }

  return null;
}

function getMessageFromParams(params, rawMethod) {
  if (typeof params?.message === 'string') return params.message;
  if (typeof params?.data === 'string') return params.data;

  if (Array.isArray(params?.args)) {
    if (rawMethod === 'personal_sign') {
      return typeof params.args[0] === 'string' ? params.args[0] : '';
    }

    if (rawMethod === 'eth_sign') {
      if (typeof params.args[1] === 'string') return params.args[1];
      return typeof params.args[0] === 'string' ? params.args[0] : '';
    }

    return typeof params.args[0] === 'string' ? params.args[0] : '';
  }

  return '';
}

function encodeBase64Object(value) {
  const bytes = new TextEncoder().encode(JSON.stringify(value));
  return btoa(String.fromCharCode(...bytes));
}

function decodeBase64Object(base64String) {
  const raw = atob(base64String);
  const bytes = Uint8Array.from(raw, (ch) => ch.charCodeAt(0));
  return JSON.parse(new TextDecoder().decode(bytes));
}

const BS58_ALPHABET = '123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz';

function bs58decode(str) {
  let num = 0n;
  for (let i = 0; i < str.length; i++) {
    const idx = BS58_ALPHABET.indexOf(str[i]);
    if (idx < 0) throw new Error('Invalid base58 character');
    num = num * 58n + BigInt(idx);
  }
  let hex = num === 0n ? '' : num.toString(16);
  if (hex.length % 2) hex = `0${hex}`;
  const bytes = [];
  for (let i = 0; i < hex.length; i += 2) {
    bytes.push(parseInt(hex.slice(i, i + 2), 16));
  }
  let leadingOnes = 0;
  for (let i = 0; i < str.length && str[i] === '1'; i++) leadingOnes++;
  const out = new Uint8Array(leadingOnes + bytes.length);
  out.set(bytes, leadingOnes);
  return out;
}

function normalizePubkeyBytes(value) {
  if (Array.isArray(value)) return Uint8Array.from(value);
  if (typeof value === 'string') return bs58decode(value);
  throw new Error('Invalid pubkey format in transaction');
}

function normalizeDataBytes(value) {
  if (Array.isArray(value)) return Uint8Array.from(value);
  if (typeof value === 'string') return new TextEncoder().encode(value);
  return new Uint8Array(0);
}

function normalizeMessageForSigning(messageLike) {
  const blockhash = messageLike?.blockhash || messageLike?.recent_blockhash || messageLike?.recentBlockhash;
  if (typeof blockhash !== 'string' || blockhash.length !== 64) {
    throw new Error('Transaction message is missing a valid blockhash');
  }

  const instructions = Array.isArray(messageLike?.instructions) ? messageLike.instructions : [];
  if (!instructions.length) throw new Error('Transaction message has no instructions');

  return {
    instructions: instructions.map((ix) => ({
      program_id: Array.from(normalizePubkeyBytes(ix?.program_id ?? ix?.programId)),
      accounts: Array.isArray(ix?.accounts) ? ix.accounts.map((a) => Array.from(normalizePubkeyBytes(a))) : [],
      data: Array.from(normalizeDataBytes(ix?.data))
    })),
    blockhash
  };
}

function messageBytesForSigning(txObject) {
  const signTarget = txObject?.message || txObject;
  const normalizedMessage = normalizeMessageForSigning(signTarget);
  return serializeMessageForSigning(normalizedMessage);
}

async function getRpcForContext(context = {}) {
  const endpoint = await getConfiguredRpcEndpoint(context.network || 'local-testnet');
  return new MoltChainRPC(endpoint);
}

function getChainId(context = {}) {
  return `molt:${context.network || 'local-testnet'}`;
}

async function resolveAddressForReadMethod(payload, connectedAddress) {
  const params = normalizeParams(payload);
  const candidate = getAddressFromParams(params, connectedAddress);
  if (!candidate || typeof candidate !== 'string') {
    throw new Error('Address is required');
  }
  return candidate;
}

async function finalizeSignMessage(request, context, approvalInput) {
  const activeWallet = context.activeWallet || null;
  if (!activeWallet) {
    return { ok: false, error: 'No active wallet' };
  }

  const password = approvalInput?.password || '';
  if (!password) {
    return { ok: false, error: 'Password required for signing' };
  }

  const params = normalizeParams(request.payload);
  const rawMethod = String(request?.payload?.method || '');
  const message = getMessageFromParams(params, rawMethod);
  if (!message || typeof message !== 'string') {
    return { ok: false, error: 'Missing message string' };
  }

  let privateKeyHex;
  try {
    privateKeyHex = await decryptPrivateKey(activeWallet.encryptedKey, password);
    const messageBytes = new TextEncoder().encode(message);
    const signature = await signTransaction(privateKeyHex, messageBytes);

    return {
      ok: true,
      result: {
        signature: bytesToHex(signature),
        address: activeWallet.address
      }
    };
  } finally {
    if (typeof privateKeyHex === 'string') privateKeyHex = '0'.repeat(privateKeyHex.length);
  }
}

async function finalizeSignTransaction(request, context, approvalInput) {
  const activeWallet = context.activeWallet || null;
  if (!activeWallet) {
    return { ok: false, error: 'No active wallet' };
  }

  const password = approvalInput?.password || '';
  if (!password) {
    return { ok: false, error: 'Password required for signing' };
  }

  const params = normalizeParams(request.payload);
  const incomingTx = getTransactionFromParams(params);
  if (!incomingTx) {
    return { ok: false, error: 'Missing transaction payload' };
  }

  const txObject = typeof incomingTx === 'string' ? decodeBase64Object(incomingTx) : incomingTx;

  let privateKeyHex;
  try {
    privateKeyHex = await decryptPrivateKey(activeWallet.encryptedKey, password);
    const messageBytes = messageBytesForSigning(txObject);
    const signatureBytes = await signTransaction(privateKeyHex, messageBytes);
    const signatureArray = Array.from(signatureBytes);

    const signedTx = {
      ...txObject,
      signatures: Array.isArray(txObject.signatures)
        ? [...txObject.signatures, signatureArray]
        : [signatureArray]
    };

    return {
      ok: true,
      result: {
        signedTransaction: signedTx,
        signedTransactionBase64: encodeBase64Object(signedTx),
        signature: bytesToHex(signatureBytes)
      }
    };
  } finally {
    if (typeof privateKeyHex === 'string') privateKeyHex = '0'.repeat(privateKeyHex.length);
  }
}

async function finalizeSendTransaction(request, context, approvalInput) {
  const activeWallet = context.activeWallet || null;
  if (!activeWallet) {
    return { ok: false, error: 'No active wallet' };
  }

  const password = approvalInput?.password || '';
  if (!password) {
    return { ok: false, error: 'Password required for signing' };
  }

  const params = normalizeParams(request.payload);
  const incomingTx = getTransactionFromParams(params);
  if (!incomingTx) {
    return { ok: false, error: 'Missing transaction payload' };
  }

  const txObject = typeof incomingTx === 'string' ? decodeBase64Object(incomingTx) : incomingTx;

  let privateKeyHex;
  try {
    privateKeyHex = await decryptPrivateKey(activeWallet.encryptedKey, password);
    const messageBytes = messageBytesForSigning(txObject);
    const signatureBytes = await signTransaction(privateKeyHex, messageBytes);
    const signatureArray = Array.from(signatureBytes);

    const signedTx = {
      ...txObject,
      signatures: Array.isArray(txObject.signatures)
        ? [...txObject.signatures, signatureArray]
        : [signatureArray]
    };

    const txBase64 = encodeBase64Object(signedTx);
    const rpc = await getRpcForContext(context);
    const txHash = await rpc.sendTransaction(txBase64);

    return {
      ok: true,
      result: {
        txHash,
        signature: bytesToHex(signatureBytes),
        signedTransaction: signedTx,
        signedTransactionBase64: txBase64
      }
    };
  } finally {
    if (typeof privateKeyHex === 'string') privateKeyHex = '0'.repeat(privateKeyHex.length);
  }
}

async function finalizePendingRequest(requestId, approved, context = {}, approvalInput = {}) {
  prunePendingRequests();
  const request = pendingRequests.get(requestId);
  if (!request) {
    return { ok: false, error: 'Request not found' };
  }

  if (request.finalized) {
    return { ok: false, error: 'Request already finalized' };
  }

  const method = normalizeMethod(request?.payload?.method);

  if (!approved) {
    request.finalized = { ok: false, error: 'User rejected request' };
    request.finalizedAt = Date.now();
    return { ok: true };
  }

  if (request.origin) {
    await approveOrigin(request.origin);
  }

  if (method === 'molt_requestAccounts') {
    const activeAddress = context.activeAddress || null;
    if (!activeAddress) {
      request.finalized = { ok: false, error: 'No active wallet' };
      request.finalizedAt = Date.now();
      return { ok: true };
    }

    request.finalized = { ok: true, result: [activeAddress] };
    request.finalizedAt = Date.now();
    return { ok: true };
  }

  if (method === 'molt_signMessage') {
    request.finalized = await finalizeSignMessage(request, context, approvalInput);
    request.finalizedAt = Date.now();
    return { ok: true };
  }

  if (method === 'molt_signTransaction') {
    request.finalized = await finalizeSignTransaction(request, context, approvalInput);
    request.finalizedAt = Date.now();
    return { ok: true };
  }

  if (method === 'molt_sendTransaction') {
    request.finalized = await finalizeSendTransaction(request, context, approvalInput);
    request.finalizedAt = Date.now();
    return { ok: true };
  }

  request.finalized = {
    ok: false,
    error: `Approved but handler not implemented for ${String(method || 'unknown')}`
  };
  request.finalizedAt = Date.now();
  return { ok: true };
}

export async function handleProviderRequest(payload, context = {}) {
  prunePendingRequests();

  const method = normalizeMethod(payload?.method);
  const origin = context.origin || null;
  const connected = await isOriginApproved(origin);
  const chainId = getChainId(context);
  const isLocked = Boolean(context.isLocked);
  const activeAddress = connected && !isLocked ? (context.activeAddress || null) : null;

  switch (method) {
    case 'molt_getProviderState':
      return {
        ok: true,
        result: {
          connected,
          origin,
          chainId,
          network: context.network || 'local-testnet',
          accounts: activeAddress ? [activeAddress] : [],
          isLocked: Boolean(context.isLocked),
          version: context.appVersion || '0.1.0'
        }
      };

    case 'molt_isConnected':
      return {
        ok: true,
        result: connected
      };

    case 'molt_chainId':
      return {
        ok: true,
        result: chainId
      };

    case 'molt_network':
      return {
        ok: true,
        result: {
          network: context.network || 'local-testnet',
          chainId
        }
      };

    case 'molt_ethChainId': {
      return {
        ok: true,
        result: getNetworkMeta(context.network || 'local-testnet').chainHex
      };
    }

    case 'molt_netVersion': {
      return {
        ok: true,
        result: getNetworkMeta(context.network || 'local-testnet').netVersion
      };
    }

    case 'molt_coinbase': {
      return {
        ok: true,
        result: activeAddress || null
      };
    }

    case 'molt_blockNumber': {
      const rpc = await getRpcForContext(context);
      const latest = await rpc.getLatestBlock();
      const number = Number(latest?.height ?? latest?.number ?? 0);
      return { ok: true, result: toHexQuantity(number) };
    }

    case 'molt_getCode': {
      return { ok: true, result: '0x' };
    }

    case 'molt_estimateGas': {
      return { ok: true, result: '0x5208' };
    }

    case 'molt_gasPrice': {
      return { ok: true, result: '0x3b9aca00' };
    }

    case 'molt_clientVersion': {
      return { ok: true, result: `MoltWallet/${context.appVersion || '0.1.0'}` };
    }

    case 'molt_netListening': {
      return { ok: true, result: true };
    }

    case 'molt_switchNetwork': {
      const params = normalizeParams(payload);
      const argObject = Array.isArray(params?.args) ? params.args[0] : params;
      const targetChainId = argObject?.chainId;
      const nextNetwork = networkFromAnyChainId(targetChainId);
      if (!nextNetwork) {
        return { ok: false, error: 'Unsupported chainId for network switch' };
      }

      await patchState({ network: { selected: nextNetwork } });
      return { ok: true, result: null };
    }

    case 'molt_addNetwork': {
      const params = normalizeParams(payload);
      const spec = Array.isArray(params?.args) ? params.args[0] : params;
      const chainId = spec?.chainId;
      const rpcUrls = Array.isArray(spec?.rpcUrls) ? spec.rpcUrls : [];
      const endpoint = String(rpcUrls[0] || '').trim();

      const network = networkFromAnyChainId(chainId);
      if (!network || !endpoint) {
        return { ok: false, error: 'Invalid chain definition' };
      }

      const settingsPatch =
        network === 'mainnet'
          ? { mainnetRPC: endpoint }
          : network === 'testnet'
            ? { testnetRPC: endpoint }
            : { localTestnetRPC: endpoint };

      await patchState({ settings: settingsPatch, network: { selected: network } });
      return { ok: true, result: null };
    }

    case 'molt_watchAsset': {
      return { ok: true, result: true };
    }

    case 'molt_version':
      return {
        ok: true,
        result: context.appVersion || '0.1.0'
      };

    case 'molt_accounts':
      return {
        ok: true,
        result: activeAddress ? [activeAddress] : []
      };

    case 'molt_disconnect':
      if (!origin) {
        return { ok: false, error: 'Origin unavailable' };
      }

      await revokeOrigin(origin);
      return {
        ok: true,
        result: true
      };

    case 'molt_permissions': {
      const accounts = activeAddress ? [activeAddress] : [];
      if (!connected || !accounts.length) {
        return { ok: true, result: [] };
      }

      return {
        ok: true,
        result: [
          {
            parentCapability: 'eth_accounts',
            caveats: [
              {
                type: 'filterResponse',
                value: accounts
              }
            ],
            date: Date.now(),
            invoker: origin
          }
        ]
      };
    }

    case 'molt_getBalance': {
      const address = await resolveAddressForReadMethod(payload, activeAddress);
      const rpc = await getRpcForContext(context);
      const result = await rpc.getBalance(address);
      const requestedMethod = String(payload?.method || '').trim();
      if (requestedMethod === 'eth_getBalance') {
        const shells = Number(result?.spendable ?? result?.balance ?? 0);
        return { ok: true, result: toHexQuantity(shells) };
      }
      return { ok: true, result };
    }

    case 'molt_getAccount': {
      const address = await resolveAddressForReadMethod(payload, activeAddress);
      const rpc = await getRpcForContext(context);
      const result = await rpc.getAccount(address);
      return { ok: true, result };
    }

    case 'molt_getLatestBlock': {
      const rpc = await getRpcForContext(context);
      const result = await rpc.getLatestBlock();
      return { ok: true, result };
    }

    case 'molt_getTransactions': {
      const params = normalizeParams(payload);
      const address = await resolveAddressForReadMethod(payload, activeAddress);
      const argsLimit = Array.isArray(params.args) ? Number(params.args[1]) : NaN;
      const limit = Math.max(1, Math.min(100, Number.isFinite(argsLimit) ? argsLimit : Number(params.limit || 20)));
      const rpc = await getRpcForContext(context);
      const result = await rpc.getTransactionsByAddress(address, { limit });
      const requestedMethod = String(payload?.method || '').trim();
      if (requestedMethod === 'eth_getTransactionCount') {
        const txs = Array.isArray(result?.transactions)
          ? result.transactions
          : Array.isArray(result?.items)
            ? result.items
            : Array.isArray(result)
              ? result
              : [];
        return { ok: true, result: toHexQuantity(txs.length) };
      }
      return { ok: true, result };
    }

    case 'molt_requestAccounts': {
      if (isLocked) {
        return { ok: false, error: 'Wallet is locked' };
      }

      if (connected) {
        if (!activeAddress) {
          return { ok: false, error: 'No active wallet' };
        }
        return { ok: true, result: [activeAddress] };
      }

      const requestId = createPendingRequest(payload, context);
      return {
        ok: true,
        pending: true,
        requestId
      };
    }

    case 'molt_signMessage': {
      const requestId = createPendingRequest(payload, context);
      return {
        ok: true,
        pending: true,
        requestId
      };
    }

    case 'molt_signTransaction': {
      const requestId = createPendingRequest(payload, context);
      return {
        ok: true,
        pending: true,
        requestId
      };
    }

    case 'molt_sendTransaction': {
      const requestId = createPendingRequest(payload, context);
      return {
        ok: true,
        pending: true,
        requestId
      };
    }

    default:
      return {
        ok: false,
        error: `Unsupported provider method: ${String(method || 'unknown')}`
      };
  }
}

export async function getProviderStateSnapshot(context = {}) {
  const origin = context.origin || null;
  const connected = await isOriginApproved(origin);
  const chainId = getChainId(context);
  const activeAddress = connected ? (context.activeAddress || null) : null;

  return {
    connected,
    origin,
    chainId,
    network: context.network || 'local-testnet',
    activeAddress,
    accounts: activeAddress ? [activeAddress] : [],
    isLocked: Boolean(context.isLocked)
  };
}

export { getPendingRequest, consumeFinalizedResult, finalizePendingRequest };
