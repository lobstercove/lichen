import { LichenRPC, getTrustedRpcEndpoint } from './rpc-service.js';
import { decryptPrivateKey, isValidAddress, privateKeyToKeypair, signTransaction } from './crypto-service.js';

const SUPPORTED_CHAINS = ['solana', 'ethereum', 'bsc', 'bnb', 'neox', 'neo-x', 'neo_x'];
const SUPPORTED_ASSETS = ['usdc', 'usdt', 'sol', 'eth', 'bnb', 'gas', 'neo'];
const BRIDGE_AUTH_TTL_SECS = 24 * 60 * 60;
const BRIDGE_AUTH_DOMAIN_V2 = 'LICHEN_BRIDGE_ACCESS_V2';
const BRIDGE_AUTH_CREATE_ACTION = 'createBridgeDeposit';
const BRIDGE_CACHE_KEY = 'lichenBridgeDeposits';

let activeBridgeAuth = null;

function getTrustedBridgeRpc(network) {
  return new LichenRPC(getTrustedRpcEndpoint(network));
}

function buildBridgeAccessMessage(userId, issuedAt, expiresAt) {
  return `LICHEN_BRIDGE_ACCESS_V1\nuser_id=${userId}\nissued_at=${issuedAt}\nexpires_at=${expiresAt}\n`;
}

function bridgeAuthNonce() {
  const bytes = new Uint8Array(16);
  crypto.getRandomValues(bytes);
  return Array.from(bytes, (byte) => byte.toString(16).padStart(2, '0')).join('');
}

function buildBridgeAccessMessageV2(userId, chain, asset, issuedAt, expiresAt, nonce) {
  const canonicalChain = canonicalBridgeChain(chain);
  const normalizedAsset = String(asset || '').trim().toLowerCase();
  return `${BRIDGE_AUTH_DOMAIN_V2}\naction=${BRIDGE_AUTH_CREATE_ACTION}\nuser_id=${userId}\nchain=${canonicalChain}\nasset=${normalizedAsset}\nroute=${canonicalChain}:${normalizedAsset}\nissued_at=${issuedAt}\nexpires_at=${expiresAt}\nnonce=${nonce}\n`;
}

function bridgeAuthMatchesRoute(auth, chain = '', asset = '') {
  const canonicalChain = canonicalBridgeChain(chain);
  const normalizedAsset = String(asset || '').trim().toLowerCase();
  if (!canonicalChain && !normalizedAsset) return true;
  return auth?.version === 2
    && auth.chain === canonicalChain
    && auth.asset === normalizedAsset
    && auth.route === `${canonicalChain}:${normalizedAsset}`;
}

function hasValidBridgeAccessAuth(wallet, { chain = '', asset = '' } = {}) {
  if (!wallet?.address || !activeBridgeAuth) return false;
  const now = Math.floor(Date.now() / 1000);
  return activeBridgeAuth.user_id === wallet.address
    && activeBridgeAuth.expires_at > now + 30
    && bridgeAuthMatchesRoute(activeBridgeAuth, chain, asset);
}

export function hasBridgeAccessAuth(wallet, route = {}) {
  return hasValidBridgeAccessAuth(wallet, route);
}

function currentBridgeAuthPayload(wallet, { chain = '', asset = '' } = {}) {
  if (!hasValidBridgeAccessAuth(wallet, { chain, asset })) return null;
  const payload = {
    issued_at: activeBridgeAuth.issued_at,
    expires_at: activeBridgeAuth.expires_at,
    signature: activeBridgeAuth.signature
  };
  for (const key of ['version', 'domain', 'action', 'user_id', 'chain', 'asset', 'route', 'nonce']) {
    if (activeBridgeAuth[key] !== undefined && activeBridgeAuth[key] !== null) {
      payload[key] = activeBridgeAuth[key];
    }
  }
  return payload;
}

async function ensureBridgeAccessAuth(wallet, password, { forceRefresh = false, chain = '', asset = '' } = {}) {
  if (!forceRefresh && hasValidBridgeAccessAuth(wallet, { chain, asset })) {
    return currentBridgeAuthPayload(wallet, { chain, asset });
  }
  if (!wallet?.encryptedKey) {
    throw new Error('Bridge authorization requires an unlocked wallet');
  }
  if (typeof password !== 'string' || !password) {
    throw new Error('Wallet password required for bridge authorization');
  }

  let privateKeyHex = null;
  try {
    privateKeyHex = await decryptPrivateKey(wallet.encryptedKey, password);
    const keypair = await privateKeyToKeypair(privateKeyHex);
    if (keypair.address !== wallet.address) {
      throw new Error('Wallet key does not match the active address. Re-import this wallet from its seed phrase or private key, then try again.');
    }
    const issuedAt = Math.floor(Date.now() / 1000);
    const expiresAt = issuedAt + BRIDGE_AUTH_TTL_SECS;
    const canonicalChain = canonicalBridgeChain(chain);
    const normalizedAsset = String(asset || '').trim().toLowerCase();
    const useV2 = Boolean(canonicalChain && normalizedAsset);
    const nonce = useV2 ? bridgeAuthNonce() : '';
    const message = useV2
      ? buildBridgeAccessMessageV2(wallet.address, canonicalChain, normalizedAsset, issuedAt, expiresAt, nonce)
      : buildBridgeAccessMessage(wallet.address, issuedAt, expiresAt);
    const messageBytes = new TextEncoder().encode(message);
    const signature = await signTransaction(keypair.privateKey, messageBytes);

    activeBridgeAuth = {
      user_id: wallet.address,
      issued_at: issuedAt,
      expires_at: expiresAt,
      signature
    };
    if (useV2) {
      activeBridgeAuth.version = 2;
      activeBridgeAuth.domain = BRIDGE_AUTH_DOMAIN_V2;
      activeBridgeAuth.action = BRIDGE_AUTH_CREATE_ACTION;
      activeBridgeAuth.chain = canonicalChain;
      activeBridgeAuth.asset = normalizedAsset;
      activeBridgeAuth.route = `${canonicalChain}:${normalizedAsset}`;
      activeBridgeAuth.nonce = nonce;
    }

    return currentBridgeAuthPayload(wallet);
  } finally {
    if (typeof privateKeyHex === 'string' && privateKeyHex.length > 0) {
      privateKeyHex = '0'.repeat(privateKeyHex.length);
    }
  }
}

function bridgeDepositUserMessage(error) {
  const message = String(error?.message || error || '').trim();
  if (/Invalid bridge auth signature/i.test(message)) {
    return 'Bridge authorization did not match this wallet. Re-import the wallet from its seed phrase or private key, then try again.';
  }
  if (/missing CUSTODY_|Bridge service unavailable|Bridge service not configured|missing RPC URL for chain/i.test(message)) {
    return 'This bridge route is not live on testnet yet. The custody route must be enabled before a deposit address can be created.';
  }
  if (/rate_limited/i.test(message)) {
    return 'Too many bridge requests. Wait a few seconds, then request a new deposit address.';
  }
  return message || 'Failed to connect to bridge service';
}

function normalizeBridgeRecord(record = {}, fallback = {}) {
  const depositId = String(record.deposit_id || fallback.deposit_id || '').trim();
  if (!depositId) return null;

  return {
    deposit_id: depositId,
    address: String(record.address || fallback.address || '').trim(),
    status: String(record.status || fallback.status || 'issued').trim().toLowerCase(),
    user_id: String(record.user_id || fallback.user_id || '').trim(),
    chain: String(record.chain || fallback.chain || '').trim().toLowerCase(),
    asset: String(record.asset || fallback.asset || '').trim().toLowerCase(),
    network: String(record.network || fallback.network || '').trim(),
    updated_at: Number(record.updated_at || fallback.updated_at || Date.now()) || Date.now()
  };
}

function canonicalBridgeChain(chain) {
  const normalized = String(chain || '').trim().toLowerCase();
  if (normalized === 'bnb') return 'bsc';
  if (normalized === 'neo-x' || normalized === 'neo_x') return 'neox';
  return normalized;
}

function bridgeRouteIsPaused(status) {
  if (!status || typeof status !== 'object') return false;
  if (status.paused === true || status.route_paused === true || status.active === true || status.blocked === true) {
    return true;
  }
  return Array.isArray(status.active_restriction_ids) && status.active_restriction_ids.length > 0;
}

function routeRestrictionIds(status) {
  if (!status || typeof status !== 'object' || !Array.isArray(status.active_restriction_ids)) return '';
  return status.active_restriction_ids.map((id) => `#${id}`).join(', ');
}

export async function getBridgeRouteRestrictionStatus({ chain, asset, network }) {
  const canonicalChain = canonicalBridgeChain(chain);
  const normalizedAsset = String(asset || '').trim().toLowerCase();
  const rpc = getTrustedBridgeRpc(network);
  return rpc.call('getBridgeRouteRestrictionStatus', [{ chain: canonicalChain, asset: normalizedAsset }]);
}

async function assertBridgeRouteOpen({ chain, asset, network }) {
  let status;
  try {
    status = await getBridgeRouteRestrictionStatus({ chain, asset, network });
  } catch (error) {
    throw new Error(`Bridge route status unavailable for ${String(asset || '').toUpperCase()} on ${canonicalBridgeChain(chain)}`);
  }
  if (bridgeRouteIsPaused(status)) {
    const ids = routeRestrictionIds(status);
    throw new Error(`Bridge route paused for ${String(asset || '').toUpperCase()} on ${canonicalBridgeChain(chain)}${ids ? ` (${ids})` : ''}`);
  }
  return status;
}

async function loadBridgeCacheRecords() {
  try {
    const raw = await chrome.storage.local.get(BRIDGE_CACHE_KEY);
    return Array.isArray(raw?.[BRIDGE_CACHE_KEY]) ? raw[BRIDGE_CACHE_KEY] : [];
  } catch {
    return [];
  }
}

async function saveBridgeCacheRecords(records) {
  const normalized = records
    .map((record) => normalizeBridgeRecord(record))
    .filter(Boolean)
    .sort((left, right) => Number(right.updated_at || 0) - Number(left.updated_at || 0))
    .slice(0, 50);

  try {
    await chrome.storage.local.set({ [BRIDGE_CACHE_KEY]: normalized });
  } catch {
    // Ignore cache write failures in bridge UX.
  }
}

async function upsertBridgeCacheRecord(record, fallback = {}) {
  const normalized = normalizeBridgeRecord(record, fallback);
  if (!normalized) return;

  const existing = await loadBridgeCacheRecords();
  const filtered = existing.filter((entry) => entry?.deposit_id !== normalized.deposit_id);
  filtered.push(normalized);
  await saveBridgeCacheRecords(filtered);
}

export async function loadBridgeSnapshot(address, network) {
  if (!address) return null;

  const deposits = (await loadBridgeCacheRecords())
    .filter((entry) => entry?.user_id === address && entry?.network === network)
    .sort((left, right) => Number(right.updated_at || 0) - Number(left.updated_at || 0));
  const pending = deposits.filter((d) => {
    const s = String(d.status || '').toLowerCase();
    return s && s !== 'credited' && s !== 'completed' && s !== 'expired';
  }).length;

  return {
    totalDeposits: deposits.length,
    pending,
    raw: deposits.slice(0, 10)
  };
}

export async function requestBridgeDepositAddress({ wallet, password, chain, asset, network }) {
  if (!wallet?.address) {
    throw new Error('Missing user address');
  }
  if (!isValidAddress(wallet.address)) {
    throw new Error('Invalid user wallet address');
  }

  const normalizedChain = String(chain || '').trim().toLowerCase();
  const canonicalChain = canonicalBridgeChain(normalizedChain);
  const normalizedAsset = String(asset || '').trim().toLowerCase();
  if (!SUPPORTED_CHAINS.includes(normalizedChain)) {
    throw new Error('Unsupported bridge chain');
  }
  if (!SUPPORTED_ASSETS.includes(normalizedAsset)) {
    throw new Error('Unsupported bridge asset');
  }

  await assertBridgeRouteOpen({ chain: canonicalChain, asset: normalizedAsset, network });

  const auth = await ensureBridgeAccessAuth(wallet, password, {
    forceRefresh: false,
    chain: canonicalChain,
    asset: normalizedAsset
  });

  // Route through authenticated RPC bridge proxy — custody auth stays server-side.
  const rpc = getTrustedBridgeRpc(network);
  let result;
  try {
    result = await rpc.call('createBridgeDeposit', [{
      user_id: wallet.address,
      chain: canonicalChain,
      asset: normalizedAsset,
      auth
    }]);
  } catch (error) {
    throw new Error(bridgeDepositUserMessage(error));
  }

  await upsertBridgeCacheRecord(result, {
    deposit_id: result?.deposit_id,
    address: result?.address,
    status: result?.status || 'issued',
    user_id: wallet.address,
    chain: canonicalChain,
    asset: normalizedAsset,
    network,
    updated_at: Date.now()
  });

  return result;
}

export async function getBridgeDepositStatus({ depositId, wallet, network }) {
  if (!depositId) {
    throw new Error('Missing deposit id');
  }
  if (!wallet?.address) {
    throw new Error('Missing user address');
  }

  const auth = currentBridgeAuthPayload(wallet);
  if (!auth) {
    throw new Error('Bridge authorization expired. Re-open the bridge flow to continue status checks.');
  }

  // Route through authenticated RPC bridge proxy — custody auth stays server-side.
  const rpc = getTrustedBridgeRpc(network);
  const result = await rpc.call('getBridgeDeposit', [{
    deposit_id: depositId,
    user_id: wallet.address,
    auth
  }]);

  await upsertBridgeCacheRecord(result, {
    deposit_id: depositId,
    status: result?.status || 'issued',
    user_id: wallet.address,
    network,
    updated_at: Date.now()
  });

  return result;
}
