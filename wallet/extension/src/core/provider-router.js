import { decryptPrivateKey, signTransaction, bytesToHex } from './crypto-service.js';
import { LichenRPC, getConfiguredRpcEndpoint } from './rpc-service.js';
import { patchState } from './state-store.js';
import { serializeMessageForSigning } from './tx-service.js';
import {
  getTrustedRestrictionRpc,
  preflightTransactionRestrictions,
  RESTRICTION_METHODS,
  restrictionPreflightSummary
} from './restriction-service.js';

const APPROVED_ORIGINS_KEY = 'lichenApprovedOrigins';
const APPROVED_ORIGINS_META_KEY = 'lichenApprovedOriginsMeta';
const APPROVED_ORIGIN_TTL_MS = 30 * 24 * 60 * 60 * 1000;
const pendingRequests = new Map();
const MAX_PENDING_REQUESTS = 200;
const PENDING_REQUEST_TTL_MS = 3 * 60 * 1000;
const FINALIZED_REQUEST_TTL_MS = 5 * 60 * 1000;
const EXTERNAL_EVM_NETWORKS = Object.freeze({
  neox: {
    chainHex: '0xba93',
    netVersion: '47763',
    chainName: 'Neo X Mainnet',
    nativeCurrency: { name: 'GAS', symbol: 'GAS', decimals: 18 }
  },
  neoxTestnetT4: {
    chainHex: '0xba9304',
    netVersion: '12227332',
    chainName: 'Neo X Testnet T4',
    nativeCurrency: { name: 'GAS', symbol: 'GAS', decimals: 18 }
  }
});

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

function externalEvmNetworkFromAnyChainId(chainIdInput) {
  const value = String(chainIdInput || '').trim().toLowerCase();
  const normalized = value.startsWith('0x') ? value : `0x${value}`;
  if (normalized === EXTERNAL_EVM_NETWORKS.neox.chainHex) return EXTERNAL_EVM_NETWORKS.neox;
  if (normalized === EXTERNAL_EVM_NETWORKS.neoxTestnetT4.chainHex) return EXTERNAL_EVM_NETWORKS.neoxTestnetT4;
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
      createdAt: entry.createdAt || Date.now(),
      restrictionBlocked: entry.restrictionPreflight?.allowed === false
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

function createPendingRequest(payload, context, extra = {}) {
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
    finalized: null,
    restrictionPreflight: extra.restrictionPreflight || null
  });
  return requestId;
}

function findPendingRequestByOriginAndMethod(origin, method) {
  prunePendingRequests();
  const targetOrigin = String(origin || '').trim();
  const targetMethod = normalizeMethod(method);

  for (const request of pendingRequests.values()) {
    if (!request || request.finalized) continue;
    if (String(request.origin || '').trim() !== targetOrigin) continue;
    if (normalizeMethod(request.payload?.method) !== targetMethod) continue;
    return request;
  }

  return null;
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
    licn_getAccounts: 'licn_accounts',
    licn_request_accounts: 'licn_requestAccounts',
    licn_sign_message: 'licn_signMessage',
    licn_sign_transaction: 'licn_signTransaction',
    licn_send_transaction: 'licn_sendTransaction',
    licn_get_transactions: 'licn_getTransactions',
    licn_get_transactions_by_address: 'licn_getTransactions',
    licn_latest_block: 'licn_getLatestBlock',
    licn_get_provider_state: 'licn_getProviderState',
    licn_is_connected: 'licn_isConnected',
    lichen_getRestrictionStatus: 'licn_getRestrictionStatus',
    lichen_canTransfer: 'licn_canTransfer',
    lichen_getContractLifecycleStatus: 'licn_getContractLifecycleStatus',
    eth_accounts: 'licn_accounts',
    eth_requestAccounts: 'licn_requestAccounts',
    personal_sign: 'licn_signMessage',
    eth_sign: 'licn_signMessage',
    eth_signTransaction: 'licn_signTransaction',
    eth_sendTransaction: 'licn_sendTransaction',
    eth_getBalance: 'licn_getBalance',
    eth_getTransactionCount: 'licn_getTransactions',
    eth_chainId: 'licn_ethChainId',
    net_version: 'licn_netVersion',
    eth_coinbase: 'licn_coinbase',
    licn_connect: 'licn_requestAccounts',
    wallet_getPermissions: 'licn_permissions',
    wallet_requestPermissions: 'licn_requestAccounts',
    wallet_revokePermissions: 'licn_disconnect',
    licn_getPermissions: 'licn_permissions',
    wallet_switchEthereumChain: 'licn_switchNetwork',
    wallet_addEthereumChain: 'licn_addNetwork',
    wallet_watchAsset: 'licn_watchAsset',
    eth_blockNumber: 'licn_blockNumber',
    eth_getCode: 'licn_getCode',
    eth_estimateGas: 'licn_estimateGas',
    eth_gasPrice: 'licn_gasPrice',
    web3_clientVersion: 'lichen_clientVersion',
    net_listening: 'licn_netListening'
  };
  return aliasMap[method] || method;
}

function singleProviderParam(payload, expectedMessage) {
  const params = payload?.params;
  if (Array.isArray(params)) {
    if (params.length !== 1) {
      throw new Error(expectedMessage);
    }
    return params[0];
  }
  if (params === undefined || params === null) {
    throw new Error(expectedMessage);
  }
  return params;
}

function stringFieldFromObject(object, fieldNames) {
  if (!object || typeof object !== 'object') return null;
  for (const fieldName of fieldNames) {
    const value = object[fieldName];
    if (typeof value === 'string' && value.trim()) return value.trim();
  }
  return null;
}

function toJsonRpcSafe(value, fieldName = 'value') {
  if (value === null || value === undefined) return value;
  if (typeof value === 'bigint') {
    if (value < 0n) throw new Error(`${fieldName} must be non-negative`);
    return value.toString();
  }
  if (Array.isArray(value)) {
    return value.map((entry, index) => toJsonRpcSafe(entry, `${fieldName}[${index}]`));
  }
  if (typeof value === 'object') {
    const out = {};
    for (const [key, entry] of Object.entries(value)) {
      if (entry === undefined) continue;
      if (typeof entry === 'function' || typeof entry === 'symbol') {
        throw new Error(`${fieldName}.${key} is not JSON-RPC serializable`);
      }
      out[key] = toJsonRpcSafe(entry, `${fieldName}.${key}`);
    }
    return out;
  }
  if (typeof value === 'function' || typeof value === 'symbol') {
    throw new Error(`${fieldName} is not JSON-RPC serializable`);
  }
  return value;
}

function normalizeRestrictionAmount(value) {
  if (value === undefined || value === null || value === '') return undefined;
  if (typeof value === 'bigint') {
    if (value < 0n) throw new Error('lichen_canTransfer amount must be a non-negative integer');
    return value.toString();
  }
  if (typeof value === 'number') {
    if (!Number.isSafeInteger(value) || value < 0) {
      throw new Error('lichen_canTransfer amount must be a non-negative integer');
    }
    return value;
  }
  if (typeof value === 'string') {
    const amount = value.trim();
    if (!/^\d+$/.test(amount)) {
      throw new Error('lichen_canTransfer amount must be a non-negative integer');
    }
    return amount;
  }
  throw new Error('lichen_canTransfer amount must be a non-negative integer');
}

function normalizeRestrictionStatusParams(payload) {
  const target = singleProviderParam(payload, 'lichen_getRestrictionStatus expects one restriction target object');
  if (!target || typeof target !== 'object' || Array.isArray(target)) {
    throw new Error('lichen_getRestrictionStatus expects one restriction target object');
  }
  return [toJsonRpcSafe(target, 'restriction target')];
}

function normalizeContractLifecycleParams(payload) {
  const raw = singleProviderParam(payload, 'lichen_getContractLifecycleStatus expects one contract address');
  const contract = typeof raw === 'string'
    ? raw.trim()
    : stringFieldFromObject(raw, ['contract', 'contract_id', 'contractId', 'account', 'address']);
  if (!contract) {
    throw new Error('lichen_getContractLifecycleStatus expects one contract address');
  }
  return [contract];
}

function normalizeCanTransferParams(payload) {
  const raw = singleProviderParam(payload, 'lichen_canTransfer expects one transfer object');
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) {
    throw new Error('lichen_canTransfer expects one transfer object');
  }

  const from = stringFieldFromObject(raw, ['from', 'source']);
  const to = stringFieldFromObject(raw, ['to', 'recipient']);
  const asset = typeof raw.asset === 'string' && raw.asset.trim() ? raw.asset.trim() : null;
  if (!from || !to || !asset) {
    throw new Error('lichen_canTransfer requires from, to, and asset');
  }

  const transfer = { from, to, asset };
  const amount = normalizeRestrictionAmount(raw.amount);
  if (amount !== undefined) transfer.amount = amount;
  return [transfer];
}

async function callProviderRestrictionMethod(payload, context, rpcMethod, normalizeParamsFn) {
  const rpc = getTrustedRestrictionRpc(context.network || 'local-testnet');
  return rpc.call(rpcMethod, normalizeParamsFn(payload));
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

function bytesToBase64(bytes) {
  let binary = '';
  for (const byte of bytes) binary += String.fromCharCode(byte);
  if (typeof btoa === 'function') return btoa(binary);
  return Buffer.from(bytes).toString('base64');
}

function base64ToBytes(value) {
  const encoded = String(value || '').trim();
  if (!encoded) throw new Error('Missing base64 transaction payload');
  if (typeof atob === 'function') {
    const raw = atob(encoded);
    return Uint8Array.from(raw, (ch) => ch.charCodeAt(0));
  }
  return new Uint8Array(Buffer.from(encoded, 'base64'));
}

function decodeBase64Object(base64String) {
  const bytes = base64ToBytes(base64String);
  return JSON.parse(new TextDecoder().decode(bytes));
}

class LichenWireReader {
  constructor(bytes, offset = 0) {
    this.bytes = bytes;
    this.offset = offset;
  }

  remaining() {
    return this.bytes.length - this.offset;
  }

  readU8(fieldName) {
    if (this.remaining() < 1) throw new Error(`Truncated ${fieldName}`);
    return this.bytes[this.offset++];
  }

  readU32LE(fieldName) {
    if (this.remaining() < 4) throw new Error(`Truncated ${fieldName}`);
    const value = this.bytes[this.offset]
      | (this.bytes[this.offset + 1] << 8)
      | (this.bytes[this.offset + 2] << 16)
      | (this.bytes[this.offset + 3] << 24);
    this.offset += 4;
    return value >>> 0;
  }

  readU64Number(fieldName) {
    if (this.remaining() < 8) throw new Error(`Truncated ${fieldName}`);
    let value = 0n;
    for (let i = 0; i < 8; i++) {
      value |= BigInt(this.bytes[this.offset + i]) << BigInt(i * 8);
    }
    this.offset += 8;
    if (value > BigInt(Number.MAX_SAFE_INTEGER)) {
      throw new Error(`${fieldName} exceeds JavaScript safe integer range`);
    }
    return Number(value);
  }

  readBytes(length, fieldName) {
    if (!Number.isSafeInteger(length) || length < 0) throw new Error(`Invalid ${fieldName} length`);
    if (this.remaining() < length) throw new Error(`Truncated ${fieldName}`);
    const value = this.bytes.slice(this.offset, this.offset + length);
    this.offset += length;
    return value;
  }

  readVecBytes(fieldName) {
    return this.readBytes(this.readU64Number(`${fieldName} length`), fieldName);
  }

  readPubkey(fieldName) {
    return Array.from(this.readBytes(32, fieldName));
  }
}

function decodeLichenWirePqSignature(reader) {
  const schemeVersion = reader.readU8('signature scheme_version');
  const publicKeySchemeVersion = reader.readU8('signature public_key scheme_version');
  const publicKeyBytes = reader.readVecBytes('signature public_key bytes');
  const signatureBytes = reader.readVecBytes('signature bytes');
  return {
    scheme_version: schemeVersion,
    public_key: {
      scheme_version: publicKeySchemeVersion,
      bytes: bytesToHex(publicKeyBytes)
    },
    sig: bytesToHex(signatureBytes)
  };
}

function decodeLichenWireOptionU64(reader, fieldName) {
  const tag = reader.readU8(`${fieldName} option`);
  if (tag === 0) return undefined;
  if (tag !== 1) throw new Error(`Invalid ${fieldName} option tag`);
  return reader.readU64Number(fieldName);
}

function decodeLichenWireMessage(reader) {
  const instructionCount = reader.readU64Number('instruction count');
  const instructions = [];
  for (let i = 0; i < instructionCount; i++) {
    const programId = reader.readPubkey(`instruction ${i} program_id`);
    const accountCount = reader.readU64Number(`instruction ${i} account count`);
    const accounts = [];
    for (let accountIndex = 0; accountIndex < accountCount; accountIndex++) {
      accounts.push(reader.readPubkey(`instruction ${i} account ${accountIndex}`));
    }
    const data = Array.from(reader.readVecBytes(`instruction ${i} data`));
    instructions.push({ program_id: programId, accounts, data });
  }

  const blockhash = bytesToHex(reader.readBytes(32, 'recent_blockhash'));
  const computeBudget = decodeLichenWireOptionU64(reader, 'compute_budget');
  const computeUnitPrice = decodeLichenWireOptionU64(reader, 'compute_unit_price');
  const message = { instructions, blockhash };
  if (computeBudget !== undefined) message.compute_budget = computeBudget;
  if (computeUnitPrice !== undefined) message.compute_unit_price = computeUnitPrice;
  return message;
}

function decodeLichenWireTransactionBase64(encodedTransaction) {
  const bytes = base64ToBytes(encodedTransaction);
  if (bytes.length < 4 || bytes[0] !== 0x4d || bytes[1] !== 0x54) {
    throw new Error('Transaction is not a lichen_tx_v1 wire envelope');
  }
  if (bytes[2] !== 1) throw new Error(`Unsupported Lichen transaction wire version: ${bytes[2]}`);

  const envelopeTxType = bytes[3];
  if (envelopeTxType !== 0 && envelopeTxType !== 1) {
    throw new Error(`Unknown Lichen transaction type byte: ${envelopeTxType}`);
  }

  const reader = new LichenWireReader(bytes, 4);
  const signatureCount = reader.readU64Number('signature count');
  const signatures = [];
  for (let i = 0; i < signatureCount; i++) {
    signatures.push(decodeLichenWirePqSignature(reader));
  }
  const message = decodeLichenWireMessage(reader);

  let payloadTxType = envelopeTxType;
  if (reader.remaining() >= 4) {
    payloadTxType = reader.readU32LE('transaction type');
  }

  return {
    signatures,
    message,
    tx_type: payloadTxType === 1 ? 'evm' : 'native'
  };
}

function decodeTransactionInputForSigning(incomingTx) {
  if (incomingTx && typeof incomingTx === 'object') {
    return { txObject: incomingTx, sourceFormat: 'object' };
  }

  if (typeof incomingTx !== 'string') {
    throw new Error('Unsupported transaction payload');
  }

  try {
    return { txObject: decodeBase64Object(incomingTx), sourceFormat: 'wallet_json_base64' };
  } catch (jsonError) {
    try {
      return { txObject: decodeLichenWireTransactionBase64(incomingTx), sourceFormat: 'lichen_tx_v1' };
    } catch (wireError) {
      throw new Error(`Unsupported transaction payload: ${wireError.message || jsonError.message}`);
    }
  }
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

function normalizeOptionalU64(value, fieldName) {
  if (value === undefined || value === null) return undefined;
  const numeric = typeof value === 'string' ? Number(value) : value;
  if (!Number.isFinite(numeric) || numeric < 0 || !Number.isInteger(numeric)) {
    throw new Error(`Transaction message has invalid ${fieldName}`);
  }
  return numeric > 0 ? numeric : undefined;
}

function normalizeMessageForSigning(messageLike) {
  const blockhash = messageLike?.blockhash || messageLike?.recent_blockhash || messageLike?.recentBlockhash;
  if (typeof blockhash !== 'string' || blockhash.length !== 64) {
    throw new Error('Transaction message is missing a valid blockhash');
  }

  const instructions = Array.isArray(messageLike?.instructions) ? messageLike.instructions : [];
  if (!instructions.length) throw new Error('Transaction message has no instructions');

  const computeBudget = normalizeOptionalU64(
    messageLike?.compute_budget ?? messageLike?.computeBudget,
    'compute_budget'
  );
  const computeUnitPrice = normalizeOptionalU64(
    messageLike?.compute_unit_price ?? messageLike?.computeUnitPrice,
    'compute_unit_price'
  );

  return {
    instructions: instructions.map((ix) => ({
      program_id: Array.from(normalizePubkeyBytes(ix?.program_id ?? ix?.programId)),
      accounts: Array.isArray(ix?.accounts) ? ix.accounts.map((a) => Array.from(normalizePubkeyBytes(a))) : [],
      data: Array.from(normalizeDataBytes(ix?.data))
    })),
    blockhash,
    compute_budget: computeBudget,
    compute_unit_price: computeUnitPrice,
  };
}

function messageBytesForSigning(txObject) {
  const signTarget = txObject?.message || txObject;
  const normalizedMessage = normalizeMessageForSigning(signTarget);
  return serializeMessageForSigning(normalizedMessage);
}

async function previewRestrictionPreflightForPayload(payload, context = {}) {
  const params = normalizeParams(payload);
  const incomingTx = getTransactionFromParams(params);
  if (!incomingTx) return null;

  try {
    const { txObject } = decodeTransactionInputForSigning(incomingTx);
    return await preflightTransactionRestrictions({
      transaction: txObject,
      fromAddress: context.activeAddress || null,
      network: context.network || 'local-testnet'
    });
  } catch (error) {
    return {
      allowed: false,
      skipped: false,
      network: context.network || 'local-testnet',
      targets: null,
      checks: [],
      warnings: [],
      blocks: [`Restriction preflight failed: ${error?.message || error}`]
    };
  }
}

async function enforceRestrictionPreflight(txObject, activeWallet, context = {}) {
  const preflight = await preflightTransactionRestrictions({
    transaction: txObject,
    fromAddress: activeWallet?.address || null,
    network: context.network || 'local-testnet'
  });
  if (preflight.allowed === false) {
    return {
      ok: false,
      error: restrictionPreflightSummary(preflight) || 'Transaction blocked by consensus restriction',
      restrictionPreflight: preflight
    };
  }
  return null;
}

async function getRpcForContext(context = {}) {
  const endpoint = await getConfiguredRpcEndpoint(context.network || 'local-testnet');
  return new LichenRPC(endpoint);
}

function getChainId(context = {}) {
  return `lichen:${context.network || 'local-testnet'}`;
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
        signature: signature.sig,
        pqSignature: signature,
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

  const { txObject, sourceFormat } = decodeTransactionInputForSigning(incomingTx);
  const preflightError = await enforceRestrictionPreflight(txObject, activeWallet, context);
  if (preflightError) return preflightError;

  let privateKeyHex;
  try {
    privateKeyHex = await decryptPrivateKey(activeWallet.encryptedKey, password);
    const messageBytes = messageBytesForSigning(txObject);
    const signature = await signTransaction(privateKeyHex, messageBytes);

    const signedTx = {
      ...txObject,
      signatures: Array.isArray(txObject.signatures)
        ? [...txObject.signatures, signature]
        : [signature]
    };

    return {
      ok: true,
      result: {
        signedTransaction: signedTx,
        signedTransactionBase64: encodeBase64Object(signedTx),
        signedTransactionFormat: 'wallet_json_base64',
        sourceTransactionFormat: sourceFormat,
        signature: signature.sig,
        pqSignature: signature
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

  const { txObject, sourceFormat } = decodeTransactionInputForSigning(incomingTx);
  const preflightError = await enforceRestrictionPreflight(txObject, activeWallet, context);
  if (preflightError) return preflightError;

  let privateKeyHex;
  try {
    privateKeyHex = await decryptPrivateKey(activeWallet.encryptedKey, password);
    const messageBytes = messageBytesForSigning(txObject);
    const signature = await signTransaction(privateKeyHex, messageBytes);

    const signedTx = {
      ...txObject,
      signatures: Array.isArray(txObject.signatures)
        ? [...txObject.signatures, signature]
        : [signature]
    };

    const txBase64 = encodeBase64Object(signedTx);
    const rpc = await getRpcForContext(context);
    const txHash = await rpc.sendTransaction(txBase64);

    return {
      ok: true,
      result: {
        txHash,
        signature: signature.sig,
        pqSignature: signature,
        signedTransaction: signedTx,
        signedTransactionBase64: txBase64,
        signedTransactionFormat: 'wallet_json_base64',
        sourceTransactionFormat: sourceFormat
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

  if (method === 'licn_requestAccounts') {
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

  if (method === 'licn_signMessage') {
    request.finalized = await finalizeSignMessage(request, context, approvalInput);
    request.finalizedAt = Date.now();
    return { ok: true };
  }

  if (method === 'licn_signTransaction') {
    request.finalized = await finalizeSignTransaction(request, context, approvalInput);
    request.finalizedAt = Date.now();
    return { ok: true };
  }

  if (method === 'licn_sendTransaction') {
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
  const approved = await isOriginApproved(origin);
  const hasWallet = Boolean(context.hasWallet);
  const connected = approved && hasWallet;
  const chainId = getChainId(context);
  const isLocked = Boolean(context.isLocked);
  const activeAddress = connected && !isLocked ? (context.activeAddress || null) : null;

  switch (method) {
    case 'licn_getProviderState':
      return {
        ok: true,
        result: {
          connected,
          origin,
          chainId,
          network: context.network || 'local-testnet',
          externalEvmNetworks: EXTERNAL_EVM_NETWORKS,
          accounts: activeAddress ? [activeAddress] : [],
          hasWallet,
          isLocked: Boolean(context.isLocked),
          version: context.appVersion || '0.1.0'
        }
      };

    case 'licn_isConnected':
      return {
        ok: true,
        result: connected
      };

    case 'licn_chainId':
      return {
        ok: true,
        result: chainId
      };

    case 'licn_network':
      return {
        ok: true,
        result: {
          network: context.network || 'local-testnet',
          chainId
        }
      };

    case 'licn_ethChainId': {
      return {
        ok: true,
        result: getNetworkMeta(context.network || 'local-testnet').chainHex
      };
    }

    case 'licn_netVersion': {
      return {
        ok: true,
        result: getNetworkMeta(context.network || 'local-testnet').netVersion
      };
    }

    case 'licn_coinbase': {
      return {
        ok: true,
        result: activeAddress || null
      };
    }

    case 'licn_blockNumber': {
      const rpc = await getRpcForContext(context);
      const latest = await rpc.getLatestBlock();
      const number = Number(latest?.height ?? latest?.number ?? 0);
      return { ok: true, result: toHexQuantity(number) };
    }

    case 'licn_getCode': {
      return { ok: true, result: '0x' };
    }

    case 'licn_estimateGas': {
      return { ok: true, result: '0x5208' };
    }

    case 'licn_gasPrice': {
      return { ok: true, result: '0x3b9aca00' };
    }

    case 'lichen_clientVersion': {
      return { ok: true, result: `LichenWallet/${context.appVersion || '0.1.0'}` };
    }

    case 'licn_netListening': {
      return { ok: true, result: true };
    }

    case 'licn_switchNetwork': {
      const params = normalizeParams(payload);
      const argObject = Array.isArray(params?.args) ? params.args[0] : params;
      const targetChainId = argObject?.chainId;
      const nextNetwork = networkFromAnyChainId(targetChainId);
      if (!nextNetwork) {
        const externalMeta = externalEvmNetworkFromAnyChainId(targetChainId);
        if (externalMeta) {
          return { ok: false, error: `${externalMeta.chainName} metadata is recognized, but Lichen wallet does not switch external EVM signing networks yet` };
        }
        return { ok: false, error: 'Unsupported chainId for network switch' };
      }

      await patchState({ network: { selected: nextNetwork } });
      return { ok: true, result: null };
    }

    case 'licn_addNetwork': {
      const params = normalizeParams(payload);
      const spec = Array.isArray(params?.args) ? params.args[0] : params;
      const chainId = spec?.chainId;
      const rpcUrls = Array.isArray(spec?.rpcUrls) ? spec.rpcUrls : [];
      const endpoint = String(rpcUrls[0] || '').trim();

      const network = networkFromAnyChainId(chainId);
      if (!network || !endpoint) {
        const externalMeta = externalEvmNetworkFromAnyChainId(chainId);
        if (externalMeta) {
          return { ok: false, error: `${externalMeta.chainName} metadata is recognized, but external EVM network addition is not enabled in Lichen wallet` };
        }
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

    case 'licn_watchAsset': {
      return { ok: true, result: true };
    }

    case 'licn_version':
      return {
        ok: true,
        result: context.appVersion || '0.1.0'
      };

    case 'licn_accounts':
      return {
        ok: true,
        result: activeAddress ? [activeAddress] : []
      };

    case 'licn_disconnect':
      if (!origin) {
        return { ok: false, error: 'Origin unavailable' };
      }

      await revokeOrigin(origin);
      return {
        ok: true,
        result: true
      };

    case 'licn_openExtension': {
      const params = normalizeParams(payload);
      const requestConnect = Boolean(params?.requestConnect);
      const connectRequest = requestConnect && origin && !approved
        ? (findPendingRequestByOriginAndMethod(origin, 'licn_requestAccounts')
          || { requestId: createPendingRequest({ method: 'licn_requestAccounts', params: [] }, context) })
        : null;
      const popupUrl = new URL(chrome.runtime.getURL('src/popup/popup.html'));
      if (connectRequest?.requestId) {
        popupUrl.searchParams.set('requestId', connectRequest.requestId);
      }
      try {
        if (chrome.windows?.create) {
          await chrome.windows.create({
            url: popupUrl.toString(),
            type: 'popup',
            focused: true,
            width: 420,
            height: 760
          });
        } else {
          await chrome.tabs.create({ url: popupUrl.toString() });
        }
      } catch {
        await chrome.tabs.create({ url: chrome.runtime.getURL('src/pages/full.html') });
      }
      return {
        ok: true,
        result: {
          opened: true,
          requestId: connectRequest?.requestId || null
        }
      };
    }

    case 'licn_permissions': {
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

    case 'licn_getBalance': {
      const address = await resolveAddressForReadMethod(payload, activeAddress);
      const rpc = await getRpcForContext(context);
      const result = await rpc.getBalance(address);
      const requestedMethod = String(payload?.method || '').trim();
      if (requestedMethod === 'eth_getBalance') {
        const spores = Number(result?.spendable ?? result?.balance ?? 0);
        return { ok: true, result: toHexQuantity(spores) };
      }
      return { ok: true, result };
    }

    case 'licn_getAccount': {
      const address = await resolveAddressForReadMethod(payload, activeAddress);
      const rpc = await getRpcForContext(context);
      const result = await rpc.getAccount(address);
      return { ok: true, result };
    }

    case 'licn_getLatestBlock': {
      const rpc = await getRpcForContext(context);
      const result = await rpc.getLatestBlock();
      return { ok: true, result };
    }

    case 'licn_getTransactions': {
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

    case 'licn_getRestrictionStatus': {
      const result = await callProviderRestrictionMethod(
        payload,
        context,
        RESTRICTION_METHODS.targetStatus,
        normalizeRestrictionStatusParams
      );
      return { ok: true, result };
    }

    case 'licn_canTransfer': {
      const result = await callProviderRestrictionMethod(
        payload,
        context,
        RESTRICTION_METHODS.canTransfer,
        normalizeCanTransferParams
      );
      return { ok: true, result };
    }

    case 'licn_getContractLifecycleStatus': {
      const result = await callProviderRestrictionMethod(
        payload,
        context,
        RESTRICTION_METHODS.contractLifecycleStatus,
        normalizeContractLifecycleParams
      );
      return { ok: true, result };
    }

    case 'licn_requestAccounts': {
      if (!hasWallet) {
        return { ok: false, error: 'No wallet is configured in the extension' };
      }

      if (isLocked) {
        return { ok: false, error: 'Wallet is locked' };
      }

      if (approved) {
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

    case 'licn_signMessage': {
      const requestId = createPendingRequest(payload, context);
      return {
        ok: true,
        pending: true,
        requestId
      };
    }

    case 'licn_signTransaction': {
      const restrictionPreflight = await previewRestrictionPreflightForPayload(payload, context);
      const requestId = createPendingRequest(payload, context, { restrictionPreflight });
      return {
        ok: true,
        pending: true,
        requestId
      };
    }

    case 'licn_sendTransaction': {
      const restrictionPreflight = await previewRestrictionPreflightForPayload(payload, context);
      const requestId = createPendingRequest(payload, context, { restrictionPreflight });
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
  const approved = await isOriginApproved(origin);
  const hasWallet = Boolean(context.hasWallet);
  const connected = approved && hasWallet;
  const chainId = getChainId(context);
  const activeAddress = connected ? (context.activeAddress || null) : null;

  return {
    connected,
    origin,
    chainId,
    network: context.network || 'local-testnet',
    activeAddress,
    accounts: activeAddress ? [activeAddress] : [],
    hasWallet,
    isLocked: Boolean(context.isLocked)
  };
}

export { getPendingRequest, consumeFinalizedResult, finalizePendingRequest };
