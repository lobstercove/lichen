/* full.js — Full-page wallet view for the LichenWallet extension.
   Replicates the website wallet UI using the extension's core modules. */

import { DEFAULT_NETWORK, loadState, saveState } from '../core/state-store.js';
import { getRpcEndpoint, LichenRPC } from '../core/rpc-service.js';
import { scheduleAutoLock, clearAutoLockAlarm } from '../core/lock-service.js';
import {
  decryptPrivateKey,
  encryptPrivateKey,
  generateId,
  generateMnemonic,
  isValidAddress,
  isValidMnemonic,
  mnemonicToKeypair,
  privateKeyToKeypair,
  bytesToHex,
  base58Encode,
  base58Decode,
  generateEVMAddress
} from '../core/crypto-service.js';
import { buildSignedNativeTransferTransaction, buildSignedSingleInstructionTransaction, encodeTransactionBase64, registerEvmAddress } from '../core/tx-service.js';
import { notify } from '../core/notification-service.js';
import {
  getBridgeDepositStatus,
  hasBridgeAccessAuth,
  preflightBridgeDepositRoute,
  requestBridgeDepositAddress
} from '../core/bridge-service.js';
import {
  loadIdentityDetails,
  registerIdentity,
  addIdentitySkill,
  updateIdentityAgentType,
  vouchForIdentity,
  setIdentityEndpoint,
  setIdentityAvailability,
  setIdentityRate,
  registerLichenName,
  renewLichenName,
  transferLichenName,
  releaseLichenName
} from '../core/identity-service.js';
import { stakeLicn, unstakeStLicn, claimMossStake, loadStakingSnapshot } from '../core/staking-service.js';
import { loadNftDetails } from '../core/nft-service.js';
import {
  assertRestrictionPreflightAllowed,
  extensionRestrictionStatusItems,
  loadExtensionRestrictionStatus,
  preflightNativeTransferRestrictions,
  restrictionPreflightSummary
} from '../core/restriction-service.js';
import {
  baseUnitsToDecimalString,
  parseDecimalBaseUnits,
  parsePositiveDecimalBaseUnits
} from '../core/amount-service.js';

const NFT_MARKETPLACE_URL = 'https://marketplace.lichen.network';
const LICN_LOGO_URL = 'https://lichen.network/assets/img/coins/128x128/licn.png';
const SHIELDED_NOTE_PAYLOAD_MAGIC_EXT = new TextEncoder().encode('LNP1');
const NOTE_ENCRYPTION_V1_PREFIX_EXT = 'a1:';

/* ──────────────────────────────────────────
   State
   ────────────────────────────────────────── */
let state = null;
let createdMnemonic = '';
let createdKeypair = null;
const LICN_USD_PRICE_CACHE_MS = 60 * 1000;
const LICN_USD_PRICE_STALE_MS = 5 * 60 * 1000;
let _licnUsdPriceCache = { value: 0.10, ts: 0, source: 'offline-fallback', fallback: true };
let extensionRestrictionStatusCache = {
  address: null,
  network: null,
  updatedAt: 0,
  status: null,
  inFlight: null
};

/* Confirm-challenge state for seed phrase verification */
let confirmWords = [];   // expected order
let selectedWords = [];  // user-selected
let poolWords = [];      // shuffled pool

/* ──────────────────────────────────────────
   Helpers
   ────────────────────────────────────────── */
function $(id) { return document.getElementById(id); }
function getActiveWallet() { return state.wallets.find(w => w.id === state.activeWalletId) || null; }
function maskAddr(a) { return (!a || a.length < 14) ? (a || '') : `${a.slice(0, 8)}…${a.slice(-6)}`; }
function decimals() { return Number(state?.settings?.decimals ?? 6); }

const EXT_BASE58_ALLOWED_RE = /^[1-9A-HJ-NP-Za-km-z]$/;

function extNumberAllowsNegative(input) {
  const minValue = Number(input.min || input.dataset.min);
  return input.dataset.allowNegative === 'true' || (Number.isFinite(minValue) && minValue < 0);
}

function extNumberAllowsDecimal(input) {
  if (input.dataset.integer === 'true') return false;
  const stepValue = String(input.step || input.dataset.step || '').trim().toLowerCase();
  if (!stepValue || stepValue === 'any') return true;
  const numericStep = Number(stepValue);
  return !Number.isFinite(numericStep) || !Number.isInteger(numericStep);
}

function sanitizeExtNumberValue(value, { allowNegative = false, allowDecimal = true } = {}) {
  let normalized = '';
  let sawDot = false;
  let sawSign = false;
  for (const char of String(value || '')) {
    if (char >= '0' && char <= '9') {
      normalized += char;
      continue;
    }
    if (char === '.' && allowDecimal && !sawDot) {
      normalized += char;
      sawDot = true;
      continue;
    }
    if (char === '-' && allowNegative && !sawSign && normalized.length === 0) {
      normalized += char;
      sawSign = true;
    }
  }
  if (normalized.startsWith('.')) return `0${normalized}`;
  if (normalized.startsWith('-.')) return normalized.replace('-.', '-0.');
  return normalized;
}

function sanitizeExtNumberInput(input, finalize = false) {
  const normalized = sanitizeExtNumberValue(input.value, {
    allowNegative: extNumberAllowsNegative(input),
    allowDecimal: extNumberAllowsDecimal(input),
  });
  if (!finalize) {
    if (normalized !== input.value) input.value = normalized;
    return;
  }
  if (!normalized || normalized === '-' || normalized === '.' || normalized === '-.') {
    input.value = '';
    return;
  }
  let numericValue = Number(normalized);
  if (!Number.isFinite(numericValue)) {
    input.value = '';
    return;
  }
  const minValue = Number(input.min || input.dataset.min);
  const maxValue = Number(input.max || input.dataset.max);
  if (Number.isFinite(minValue) && numericValue < minValue) numericValue = minValue;
  if (Number.isFinite(maxValue) && numericValue > maxValue) numericValue = maxValue;
  if (!extNumberAllowsDecimal(input)) numericValue = Math.trunc(numericValue);
  input.value = String(numericValue);
}

function sanitizeExtBase58(value) {
  return String(value || '').split('').filter((char) => EXT_BASE58_ALLOWED_RE.test(char)).join('');
}

function sanitizeExtHex(value) {
  return String(value || '').replace(/[^0-9a-fA-F]/g, '');
}

function applyExtensionInputGuards(root = document) {
  const scope = root || document;
  scope.querySelectorAll('input[data-input-kind="number"], input[data-wallet-numeric]').forEach((input) => {
    if (input.dataset.extNumericGuarded === '1') return;
    input.dataset.extNumericGuarded = '1';
    if (!input.getAttribute('inputmode')) input.setAttribute('inputmode', extNumberAllowsDecimal(input) ? 'decimal' : 'numeric');
    input.addEventListener('keydown', (event) => {
      if (event.ctrlKey || event.metaKey || event.altKey) return;
      if (event.key === 'e' || event.key === 'E' || event.key === '+') {
        event.preventDefault();
        return;
      }
      if (event.key === '-' && !extNumberAllowsNegative(input)) {
        event.preventDefault();
        return;
      }
      if (event.key === '.' && !extNumberAllowsDecimal(input)) event.preventDefault();
    });
    input.addEventListener('input', () => sanitizeExtNumberInput(input, false));
    input.addEventListener('blur', () => sanitizeExtNumberInput(input, true));
    input.addEventListener('paste', () => requestAnimationFrame(() => sanitizeExtNumberInput(input, false)));
  });

  scope.querySelectorAll('input[data-address-input="base58"], #sendTo, #shieldModalRecipient[data-address-input="base58"]').forEach((input) => {
    if (input.dataset.extAddressGuarded === '1') return;
    input.dataset.extAddressGuarded = '1';
    input.setAttribute('autocomplete', 'off');
    input.setAttribute('spellcheck', 'false');
    input.addEventListener('input', () => {
      const sanitized = sanitizeExtBase58(input.value);
      if (sanitized !== input.value) input.value = sanitized;
    });
    input.addEventListener('paste', () => requestAnimationFrame(() => {
      input.value = sanitizeExtBase58(input.value);
    }));
  });

  scope.querySelectorAll('input[data-hex-input], #shieldModalRecipient[data-hex-input]').forEach((input) => {
    if (input.dataset.extHexGuarded === '1') return;
    input.dataset.extHexGuarded = '1';
    input.setAttribute('autocomplete', 'off');
    input.setAttribute('spellcheck', 'false');
    input.addEventListener('input', () => {
      const sanitized = sanitizeExtHex(input.value);
      if (sanitized !== input.value) input.value = sanitized;
    });
    input.addEventListener('paste', () => requestAnimationFrame(() => {
      input.value = sanitizeExtHex(input.value);
    }));
  });
}

function rpc() {
  const network = state?.network?.selected || DEFAULT_NETWORK;
  const endpoint = getRpcEndpoint(network, state?.settings || {});
  return new LichenRPC(endpoint);
}

function normalizeChainSlotExt(value) {
  const slot = Number(value?.slot ?? value?.height ?? value?.current_slot ?? value?.currentSlot ?? value);
  return Number.isFinite(slot) && slot > 0 ? Math.floor(slot) : 0;
}

async function getCurrentChainSlotExt(client = rpc()) {
  try {
    const slot = normalizeChainSlotExt(await client.call('getSlot', []));
    if (slot > 0) return slot;
  } catch (_) { /* fallback */ }
  try {
    return normalizeChainSlotExt(await client.getLatestBlock());
  } catch (_) {
    return 0;
  }
}

function getQueueCurrentSlotExt(queue, fallbackSlot = 0) {
  return normalizeChainSlotExt(queue?.current_slot) || normalizeChainSlotExt(fallbackSlot);
}

function isQueueRequestClaimableExt(req, currentSlot) {
  if (typeof req?.claimable === 'boolean') return req.claimable;
  return currentSlot > 0 && Number(req?.claimable_at || 0) <= currentSlot;
}

function getQueueRequestRemainingSlotsExt(req, currentSlot) {
  const remaining = Number(req?.remaining_slots);
  if (Number.isFinite(remaining) && remaining >= 0) return Math.floor(remaining);
  const claimableAt = Number(req?.claimable_at || 0);
  return currentSlot > 0 && claimableAt > currentSlot ? Math.floor(claimableAt - currentSlot) : 0;
}

function sporesToLicn(value) {
  const raw = Number(value);
  return Number.isFinite(raw) ? raw / 1_000_000_000 : 0;
}

function formatNeoGasBaseUnits(value) {
  const raw = Number(value || 0);
  if (!Number.isFinite(raw) || raw <= 0) return '0';
  return (raw / 1_000_000_000).toLocaleString(undefined, { maximumFractionDigits: 9 });
}

function formatRewardMultiplierExt(multiplier) {
  const raw = String(multiplier ?? '1').trim();
  if (raw.endsWith('x')) return raw;
  const numeric = Number(raw);
  if (Number.isFinite(numeric)) {
    return `${numeric.toLocaleString(undefined, { maximumFractionDigits: 2 })}x`;
  }
  return `${raw || '1'}x`;
}

function formatMossStakeRewardLabel(_apyPercent, multiplier) {
  return `${formatRewardMultiplierExt(multiplier)} rewards`;
}

function formatLichenNameExt(name) {
  const bare = String(name || '').trim().replace(/(?:\.lichen)+$/i, '');
  return bare ? `${bare}.lichen` : '';
}

function bareLichenNameExt(name) {
  return String(name || '').trim().replace(/(?:\.lichen)+$/i, '').toLowerCase();
}

async function loadNeoGasRewardsSnapshot(address) {
  try {
    const client = rpc();
    const [stats, position] = await Promise.all([
      client.call('getNeoGasRewardsStats', []),
      client.call('getNeoGasRewardsPosition', [address])
    ]);
    return { stats, position };
  } catch {
    return null;
  }
}

function renderNeoGasRewardsAsset(snapshot) {
  const stats = snapshot?.stats;
  const position = snapshot?.position;
  if (!stats || !position) return '';

  const principal = Number(position.principal || 0);
  const claimable = Number(position.claimable || 0);
  const configured = stats.configured === true;
  if (!configured && principal <= 0 && claimable <= 0) return '';

  const status = stats.paused ? 'Paused' : (configured ? 'Active' : 'Pending');
  const disclosure = position.disclosure_current_accepted ? 'Accepted' : 'Required';
  return `
      <div class="asset-item">
        <div class="asset-icon" style="background:rgba(88,191,0,0.12);color:#58BF00;"><i class="fas fa-gift"></i></div>
        <div class="asset-info">
          <div class="asset-name">Neo GAS Rewards</div>
          <div class="asset-symbol">NEOGASRWD · ${escapeHtmlExt(status)}</div>
          <div style="font-size:0.75rem;color:var(--text-muted);margin-top:0.15rem;">wNEO ${formatNeoGasBaseUnits(principal)} · Disclosure ${escapeHtmlExt(disclosure)}</div>
        </div>
        <div class="asset-balance">
          <div class="asset-amount">${formatNeoGasBaseUnits(claimable)}</div>
          <div class="asset-value">Claimable wGAS</div>
        </div>
      </div>
    `;
}

function getFullBalanceSnapshot(result) {
  return {
    totalLicn: sporesToLicn(result?.spores ?? result?.balance ?? result?.total ?? result?.spendable ?? 0),
    spendableLicn: sporesToLicn(result?.spendable ?? result?.available ?? result?.spores ?? result?.balance ?? 0),
    stakedLicn: sporesToLicn(result?.staked ?? result?.staked_spores ?? 0),
    pendingRewardsLicn: sporesToLicn(result?.pending_rewards ?? result?.pendingRewards ?? 0),
    lockedLicn: sporesToLicn(result?.locked ?? result?.locked_spores ?? 0),
    mossStakedLicn: sporesToLicn(result?.moss_value ?? result?.mossValue ?? result?.moss_staked ?? result?.mossStaked ?? 0)
  };
}

function hexToBytesExt(hex) {
  const normalized = String(hex || '').replace(/^0x/i, '');
  if (!/^[0-9a-fA-F]{64}$/.test(normalized)) {
    throw new Error('Invalid decrypted wallet seed');
  }

  const bytes = new Uint8Array(normalized.length / 2);
  for (let index = 0; index < normalized.length; index += 2) {
    bytes[index / 2] = parseInt(normalized.slice(index, index + 2), 16);
  }
  return bytes;
}

function hexToBytesAnyExt(hex, expectedLength = null) {
  const normalized = String(hex || '').replace(/^0x/i, '');
  if (!/^[0-9a-fA-F]*$/.test(normalized) || normalized.length % 2 !== 0) {
    throw new Error('Invalid hex bytes');
  }
  const bytes = new Uint8Array(normalized.length / 2);
  for (let index = 0; index < normalized.length; index += 2) {
    bytes[index / 2] = parseInt(normalized.slice(index, index + 2), 16);
  }
  if (expectedLength !== null && bytes.length !== expectedLength) {
    throw new Error(`Expected ${expectedLength} bytes, got ${bytes.length}`);
  }
  return bytes;
}

function writeU64LeExt(target, offset, value) {
  new DataView(target.buffer, target.byteOffset, target.byteLength).setBigUint64(offset, BigInt(value), true);
}

function writeU32LeExt(target, offset, value) {
  new DataView(target.buffer, target.byteOffset, target.byteLength).setUint32(offset, Number(value), true);
}

function baseUnitBigIntExt(value) {
  if (typeof value === 'bigint') return value > 0n ? value : 0n;
  if (typeof value === 'number') {
    if (!Number.isSafeInteger(value) || value <= 0) return 0n;
    return BigInt(value);
  }
  const text = String(value ?? '0').trim();
  if (!/^\d+$/.test(text)) return 0n;
  return BigInt(text);
}

function parseLicnAmountSporesExt(value, label = 'Amount') {
  return parsePositiveDecimalBaseUnits(value, 9, label);
}

function parseExtensionIntegerRange(value, label, min, max, fallback = null) {
  const text = String(value ?? '').trim();
  if (!text && fallback !== null) return fallback;
  if (!/^\d+$/.test(text)) throw new Error(`${label} must be an integer between ${min} and ${max}`);
  const parsed = Number(text);
  if (!Number.isSafeInteger(parsed) || parsed < min || parsed > max) {
    throw new Error(`${label} must be an integer between ${min} and ${max}`);
  }
  return parsed;
}

function formatLicnBaseUnitsExactExt(value) {
  return baseUnitsToDecimalString(baseUnitBigIntExt(value), 9);
}

function formatLicnBaseUnitsFixedExt(value, digits = 4) {
  const [whole, fraction = ''] = formatLicnBaseUnitsExactExt(value).split('.');
  return `${whole}.${fraction.padEnd(digits, '0').slice(0, digits)}`;
}

function concatBytesExt(...chunks) {
  const total = chunks.reduce((sum, chunk) => sum + chunk.length, 0);
  const out = new Uint8Array(total);
  let offset = 0;
  for (const chunk of chunks) {
    out.set(chunk, offset);
    offset += chunk.length;
  }
  return out;
}

function buildExtShieldInstructionData(amountSpores, commitmentHex, proofBytes, encryptedNote, ephemeralPk) {
  if (!encryptedNote || !ephemeralPk) {
    throw new Error('Encrypted shielded note payload is required');
  }
  const header = new Uint8Array(41);
  header[0] = 23;
  writeU64LeExt(header, 1, amountSpores);
  header.set(hexToBytesAnyExt(commitmentHex, 32), 9);
  const notePayload = new TextEncoder().encode(JSON.stringify({
    commitment: commitmentHex,
    encrypted_note: encryptedNote,
    ephemeral_pk: ephemeralPk,
  }));
  const envelopeHeader = new Uint8Array(12);
  envelopeHeader.set(SHIELDED_NOTE_PAYLOAD_MAGIC_EXT, 0);
  writeU32LeExt(envelopeHeader, 4, proofBytes.length);
  writeU32LeExt(envelopeHeader, 8, notePayload.length);
  return concatBytesExt(header, envelopeHeader.subarray(0, 8), proofBytes, envelopeHeader.subarray(8), notePayload);
}

function buildExtUnshieldInstructionData(amountSpores, nullifierHex, merkleRootHex, recipientHashHex, proofBytes) {
  const header = new Uint8Array(105);
  header[0] = 24;
  writeU64LeExt(header, 1, amountSpores);
  header.set(hexToBytesAnyExt(nullifierHex, 32), 9);
  header.set(hexToBytesAnyExt(merkleRootHex, 32), 41);
  header.set(hexToBytesAnyExt(recipientHashHex, 32), 73);
  return concatBytesExt(header, proofBytes);
}

function buildExtTransferInstructionData(nullifiers, outputCommitments, merkleRootHex, proofBytes) {
  if (!Array.isArray(nullifiers) || nullifiers.length !== 2) throw new Error('Private transfer requires two input nullifiers');
  if (!Array.isArray(outputCommitments) || outputCommitments.length !== 2) throw new Error('Private transfer requires two output notes');

  const header = new Uint8Array(161);
  header[0] = 25;
  header.set(hexToBytesAnyExt(nullifiers[0], 32), 1);
  header.set(hexToBytesAnyExt(nullifiers[1], 32), 33);
  header.set(hexToBytesAnyExt(outputCommitments[0].commitment, 32), 65);
  header.set(hexToBytesAnyExt(outputCommitments[1].commitment, 32), 97);
  header.set(hexToBytesAnyExt(merkleRootHex, 32), 129);

  const notePayload = new TextEncoder().encode(JSON.stringify({
    outputs: outputCommitments.map((note) => ({
      commitment: note.commitment,
      encrypted_note: note.encrypted_note,
      ephemeral_pk: note.ephemeral_pk,
    })),
  }));
  const envelopeHeader = new Uint8Array(12);
  envelopeHeader.set(SHIELDED_NOTE_PAYLOAD_MAGIC_EXT, 0);
  writeU32LeExt(envelopeHeader, 4, proofBytes.length);
  writeU32LeExt(envelopeHeader, 8, notePayload.length);
  return concatBytesExt(header, envelopeHeader.subarray(0, 8), proofBytes, envelopeHeader.subarray(8), notePayload);
}

function zeroBytesExt(bytes) {
  if (bytes instanceof Uint8Array) {
    bytes.fill(0);
  }
}

function rpcEndpointToApiBase(endpoint) {
  try {
    const url = new URL(String(endpoint || '').trim());
    return `${url.origin}/api/v1`;
  } catch {
    return '';
  }
}

function normalizeLicnUsdQuote(cache = _licnUsdPriceCache, now = Date.now()) {
  const value = Number(cache?.value || 0);
  const timestamp = Number(cache?.ts || 0);
  const source = cache?.source || (timestamp > 0 ? 'oracle' : 'offline-fallback');
  return {
    value: Number.isFinite(value) && value > 0 ? value : 0.10,
    source,
    timestamp,
    stale: timestamp > 0 && now - timestamp > LICN_USD_PRICE_STALE_MS,
    fallback: cache?.fallback === true || source === 'offline-fallback',
  };
}

function licnUsdQuoteSuffix(quote) {
  if (quote?.fallback) return ' · offline estimate';
  if (quote?.stale) return ' · stale price';
  return '';
}

function licnUsdQuoteTitle(quote) {
  if (!quote) return 'USD valuation source unavailable';
  const source = quote.source === 'offline-fallback' ? 'offline fallback estimate' : quote.source;
  const updated = quote.timestamp > 0 ? new Date(quote.timestamp).toLocaleString() : 'not available';
  const stale = quote.stale ? ', stale' : '';
  return `USD valuation source: LICN ${source}, updated ${updated}${stale}`;
}

async function getLiveLicnUsdPrice() {
  const now = Date.now();
  if (now - _licnUsdPriceCache.ts < LICN_USD_PRICE_CACHE_MS && _licnUsdPriceCache.value > 0) {
    return normalizeLicnUsdQuote(_licnUsdPriceCache, now);
  }

  const endpoint = getRpcEndpoint(state?.network?.selected || DEFAULT_NETWORK, state?.settings || {});
  const apiBase = rpcEndpointToApiBase(endpoint);
  if (!apiBase) return normalizeLicnUsdQuote(_licnUsdPriceCache, now);

  try {
    const response = await fetch(`${apiBase}/oracle/prices`);
    if (!response.ok) throw new Error('oracle fetch failed');
    const data = await response.json();
    const feeds = Array.isArray(data?.feeds) ? data.feeds : [];
    const licnFeed = feeds.find((feed) => String(feed?.asset || '').toUpperCase() === 'LICN');
    const price = Number(licnFeed?.price || 0);
    if (Number.isFinite(price) && price > 0) {
      _licnUsdPriceCache = { value: price, ts: now, source: 'oracle', fallback: false };
      return normalizeLicnUsdQuote(_licnUsdPriceCache, now);
    }
  } catch {
    // Fall back to cached/default price.
  }

  return normalizeLicnUsdQuote(_licnUsdPriceCache, now);
}

function securePasswordPrompt(label = 'Wallet password (for signing):') {
  return new Promise((resolve) => {
    const overlay = document.createElement('div');
    overlay.style.cssText = 'position:fixed;top:0;left:0;width:100%;height:100%;background:rgba(0,0,0,0.6);display:flex;align-items:center;justify-content:center;z-index:99999;';
    overlay.innerHTML = `
      <div style="background:var(--bg,#1a1b26);border:1px solid var(--border,#333);border-radius:12px;padding:1rem;width:320px;max-width:92vw;box-sizing:border-box;">
        <p style="margin:0 0 0.75rem;font-size:0.85rem;color:var(--text,#e0e0e0);line-height:1.45;">${escapeHtmlExt(label)}</p>
        <input type="password" id="_fullSecPwInput" placeholder="Enter password" autocomplete="off"
          style="width:100%;padding:0.6rem;border-radius:8px;border:1px solid var(--border,#444);background:var(--card-bg,#24253a);color:var(--text,#e0e0e0);box-sizing:border-box;margin-bottom:0.75rem;">
        <div style="display:flex;gap:0.5rem;">
          <button id="_fullSecPwOk" style="flex:1;padding:0.5rem;border-radius:8px;border:none;background:var(--primary,#6C5CE7);color:#fff;cursor:pointer;">OK</button>
          <button id="_fullSecPwCancel" style="flex:1;padding:0.5rem;border-radius:8px;border:1px solid var(--border,#444);background:transparent;color:var(--text,#e0e0e0);cursor:pointer;">Cancel</button>
        </div>
      </div>`;
    document.body.appendChild(overlay);
    const input = overlay.querySelector('#_fullSecPwInput');
    input.focus();
    const finish = (value) => {
      overlay.remove();
      resolve(value);
    };
    overlay.querySelector('#_fullSecPwOk').addEventListener('click', () => finish(input.value || null));
    overlay.querySelector('#_fullSecPwCancel').addEventListener('click', () => finish(null));
    input.addEventListener('keydown', (event) => {
      if (event.key === 'Enter') finish(input.value || null);
      if (event.key === 'Escape') finish(null);
    });
  });
}

function showToast(msg, type = '') {
  const t = $('toast');
  if (!t) return;
  t.textContent = msg;
  t.className = 'toast show' + (type ? ` ${type}` : '');
  setTimeout(() => { t.classList.remove('show'); }, 3000);
}

function activeNetworkKey() {
  return state?.network?.selected || DEFAULT_NETWORK;
}

function setRestrictionElement(el, { kind = '', text = '' } = {}) {
  if (!el) return;
  if (!text) {
    el.hidden = true;
    el.className = 'extension-restriction-status';
    el.textContent = '';
    return;
  }
  el.hidden = false;
  el.className = `extension-restriction-status ${kind}`.trim();
  el.textContent = text;
}

function renderExtensionRestrictionStatus(status) {
  const el = $('extensionRestrictionStatus');
  const items = extensionRestrictionStatusItems(status);
  if (items.length > 0) {
    setRestrictionElement(el, {
      kind: 'blocked',
      text: `Consensus restriction active: ${items.join(' | ')}`
    });
    return;
  }
  if (Array.isArray(status?.criticalErrors) && status.criticalErrors.length > 0) {
    setRestrictionElement(el, {
      kind: 'warning',
      text: 'Restriction status unavailable from trusted RPC. Sending is blocked until preflight succeeds.'
    });
    return;
  }
  setRestrictionElement(el);
}

function renderSendRestrictionStatus(preflight = null) {
  const el = $('sendRestrictionStatus');
  if (preflight) {
    const summary = restrictionPreflightSummary(preflight);
    if (preflight.allowed === false) {
      setRestrictionElement(el, { kind: 'blocked', text: summary });
      return;
    }
    if (Array.isArray(preflight.warnings) && preflight.warnings.length > 0) {
      setRestrictionElement(el, { kind: 'warning', text: summary });
      return;
    }
    setRestrictionElement(el, { kind: 'passed', text: summary });
    return;
  }

  const status = extensionRestrictionStatusCache.status;
  const items = extensionRestrictionStatusItems(status)
    .filter((item) => !String(item).toLowerCase().includes('receive blocked'));
  if (items.length > 0) {
    setRestrictionElement(el, { kind: 'blocked', text: items.join(' | ') });
    return;
  }
  if (Array.isArray(status?.criticalErrors) && status.criticalErrors.length > 0) {
    setRestrictionElement(el, {
      kind: 'warning',
      text: 'Restriction status unavailable. Transfer preflight will verify before signing.'
    });
    return;
  }
  setRestrictionElement(el);
}

function renderExtensionAssetRestrictionBadges(status = extensionRestrictionStatusCache.status) {
  const badgeEl = document.querySelector('[data-asset-restriction-badges="LICN"]');
  if (!badgeEl) return;
  const items = extensionRestrictionStatusItems(status);
  if (!items.length) {
    badgeEl.innerHTML = '';
    return;
  }
  badgeEl.innerHTML = items.slice(0, 3).map((item) => (
    `<span class="extension-asset-restriction-badge">${escapeHtmlExt(item)}</span>`
  )).join('');
}

async function refreshExtensionRestrictionStatus({ force = false, updateSend = false, updateAssets = false } = {}) {
  const wallet = getActiveWallet();
  if (!wallet) {
    renderExtensionRestrictionStatus(null);
    renderSendRestrictionStatus(null);
    renderExtensionAssetRestrictionBadges(null);
    return null;
  }

  const network = activeNetworkKey();
  const now = Date.now();
  const fresh = extensionRestrictionStatusCache.address === wallet.address
    && extensionRestrictionStatusCache.network === network
    && extensionRestrictionStatusCache.status
    && now - extensionRestrictionStatusCache.updatedAt < 30_000;

  if (!force && fresh) {
    renderExtensionRestrictionStatus(extensionRestrictionStatusCache.status);
    if (updateSend) renderSendRestrictionStatus();
    if (updateAssets) renderExtensionAssetRestrictionBadges(extensionRestrictionStatusCache.status);
    return extensionRestrictionStatusCache.status;
  }

  if (extensionRestrictionStatusCache.inFlight
    && extensionRestrictionStatusCache.address === wallet.address
    && extensionRestrictionStatusCache.network === network) {
    return extensionRestrictionStatusCache.inFlight;
  }

  extensionRestrictionStatusCache.address = wallet.address;
  extensionRestrictionStatusCache.network = network;
  extensionRestrictionStatusCache.inFlight = loadExtensionRestrictionStatus({
    account: wallet.address,
    network
  }).then((status) => {
    extensionRestrictionStatusCache = {
      address: wallet.address,
      network,
      updatedAt: Date.now(),
      status,
      inFlight: null
    };
    renderExtensionRestrictionStatus(status);
    if (updateSend) renderSendRestrictionStatus();
    if (updateAssets) renderExtensionAssetRestrictionBadges(status);
    return status;
  }).catch((error) => {
    const status = {
      account: wallet.address,
      network,
      updatedAt: Date.now(),
      unavailable: true,
      criticalErrors: [error?.message || String(error)]
    };
    extensionRestrictionStatusCache = {
      address: wallet.address,
      network,
      updatedAt: Date.now(),
      status,
      inFlight: null
    };
    renderExtensionRestrictionStatus(status);
    if (updateSend) renderSendRestrictionStatus();
    if (updateAssets) renderExtensionAssetRestrictionBadges(status);
    return status;
  });
  return extensionRestrictionStatusCache.inFlight;
}

async function persist(next) {
  state = next;
  await saveState(next);
}

/* ──────────────────────────────────────────
   Screen management — identical to website
   ────────────────────────────────────────── */
const allScreens = () => document.querySelectorAll('.welcome-screen, .wallet-screen, .wallet-dashboard');

// Security: clear all sensitive input fields across all screens
function clearAllInputs() {
  document.querySelectorAll('input, textarea').forEach(el => {
    if (el.type !== 'hidden' && el.type !== 'checkbox' && el.type !== 'radio' && el.type !== 'file') {
      el.value = '';
    }
  });
  // Also clear file inputs separately
  document.querySelectorAll('input[type="file"]').forEach(el => { el.value = ''; });
}

function showScreen(id) {
  clearAllInputs();
  allScreens().forEach(el => { el.style.display = 'none'; });
  const target = $(id);
  if (target) target.style.display = target.classList.contains('wallet-dashboard') ? 'block' : 'flex';
}

/* ──────────────────────────────────────────
   Carousel
   ────────────────────────────────────────── */
function initCarousel() {
  const slides = document.querySelectorAll('.carousel-slide');
  const dots = document.querySelectorAll('.carousel-dot');
  if (!slides.length) return;

  let current = 0;
  let timer = null;

  function goTo(idx) {
    slides[current].classList.remove('active');
    dots[current].classList.remove('active');
    current = (idx + slides.length) % slides.length;
    slides[current].classList.add('active');
    dots[current].classList.add('active');
  }

  function startAuto() { timer = setInterval(() => goTo(current + 1), 4000); }
  function stopAuto() { clearInterval(timer); }

  dots.forEach(dot => {
    dot.addEventListener('click', () => { stopAuto(); goTo(Number(dot.dataset.slide)); startAuto(); });
  });

  const track = document.querySelector('.carousel-track');
  if (track) {
    track.addEventListener('mouseenter', stopAuto);
    track.addEventListener('mouseleave', startAuto);
  }
  startAuto();
}

/* ──────────────────────────────────────────
   Create Wallet Flow
   ────────────────────────────────────────── */
function setWizardStep(step) {
  document.querySelectorAll('.create-step').forEach(el => {
    const s = Number(el.dataset.step);
    el.classList.toggle('active', s === step);
  });
  document.querySelectorAll('.wizard-step-item').forEach(el => {
    const s = Number(el.dataset.step);
    el.classList.toggle('active', s === step);
    el.classList.toggle('completed', s < step);
  });
}

function shuffleCopy(arr) {
  const a = [...arr];
  for (let i = a.length - 1; i > 0; i--) {
    const j = Math.floor(Math.random() * (i + 1));
    [a[i], a[j]] = [a[j], a[i]];
  }
  return a;
}

function renderConfirmSlots() {
  const root = $('confirmSlotsGrid');
  if (!root) return;
  root.innerHTML = confirmWords.map((_, i) => {
    const w = selectedWords[i] || '';
    const filled = Boolean(w);
    const correct = filled && w === confirmWords[i];
    return `<button type="button" class="confirm-slot ${filled ? 'filled' : ''} ${correct ? 'correct' : ''}" data-idx="${i}">
      <span class="slot-number">${i + 1}.</span><span>${w}</span></button>`;
  }).join('');

  root.querySelectorAll('[data-idx]').forEach(btn => {
    btn.addEventListener('click', () => {
      const i = Number(btn.dataset.idx);
      if (!selectedWords[i]) return;
      selectedWords[i] = '';
      renderConfirmSlots();
      renderConfirmPool();
      checkConfirm();
    });
  });
}

function renderConfirmPool() {
  const root = $('confirmWordPool');
  if (!root) return;

  const usedCounts = selectedWords.reduce((acc, w) => { if (w) acc[w] = (acc[w] || 0) + 1; return acc; }, {});

  root.innerHTML = poolWords.map((word, i) => {
    const expected = confirmWords.filter(w => w === word).length;
    const used = usedCounts[word] || 0;
    return `<button type="button" class="confirm-word ${used >= expected ? 'used' : ''}" data-pool="${i}">${word}</button>`;
  }).join('');

  root.querySelectorAll('[data-pool]').forEach(btn => {
    btn.addEventListener('click', () => {
      if (btn.classList.contains('used')) return;
      const word = poolWords[Number(btn.dataset.pool)];
      const slot = selectedWords.findIndex(s => !s);
      if (slot === -1) return;
      selectedWords[slot] = word;
      renderConfirmSlots();
      renderConfirmPool();
      checkConfirm();
    });
  });
}

function checkConfirm() {
  const btn = $('finishCreateBtn');
  if (!btn) return;
  const ok = selectedWords.every((w, i) => w && w === confirmWords[i]);
  btn.disabled = !ok;
}

async function handleCreateStep2() {
  const pw = $('createPassword').value;
  const confirm = $('confirmPassword').value;
  if (!pw || pw.length < 8) { showToast('Password must be at least 8 characters', 'error'); return; }
  if (pw !== confirm) { showToast('Passwords do not match', 'error'); return; }

  createdMnemonic = await generateMnemonic();
  createdKeypair = await mnemonicToKeypair(createdMnemonic);

  // Render seed phrase grid
  const words = createdMnemonic.split(' ');
  $('seedPhraseDisplay').innerHTML = words.map((w, i) =>
    `<div class="seed-word"><span class="seed-word-number">${i + 1}</span>${w}</div>`
  ).join('');

  setWizardStep(2);
}

function handleCreateStep3() {
  const words = createdMnemonic.split(' ');
  confirmWords = words;
  selectedWords = Array(words.length).fill('');
  poolWords = shuffleCopy(words);
  renderConfirmSlots();
  renderConfirmPool();
  checkConfirm();
  setWizardStep(3);
}

async function handleFinishCreate() {
  const pw = $('createPassword').value;
  try {
    const encryptedKey = await encryptPrivateKey(createdKeypair.privateKey, pw);
    const encryptedMnemonic = await encryptPrivateKey(createdMnemonic, pw);

    const wallet = {
      id: generateId(),
      name: `Wallet ${state.wallets.length + 1}`,
      address: createdKeypair.address,
      publicKey: createdKeypair.publicKey,
      encryptedKey,
      encryptedMnemonic,
      createdAt: new Date().toISOString()
    };

    await persist({
      ...state,
      wallets: [...state.wallets, wallet],
      activeWalletId: wallet.id,
      isLocked: false
    });

    // Register EVM address on-chain in background (don't block)
    registerEvmAddress({ wallet, privateKeyHex: createdKeypair.privateKey, network: state.network?.selected, settings: state.settings }).catch(() => { });

    showToast('Wallet created successfully!', 'success');
    showDashboard();
  } catch (e) {
    showToast(`Create failed: ${e.message}`, 'error');
  }
}

/* ──────────────────────────────────────────
   Import Wallet
   ────────────────────────────────────────── */
function setupImportTabs() {
  document.querySelectorAll('.import-tab').forEach(tab => {
    tab.addEventListener('click', () => {
      document.querySelectorAll('.import-tab').forEach(t => t.classList.remove('active'));
      tab.classList.add('active');
      const method = tab.dataset.method;
      document.querySelectorAll('.import-method').forEach(m => {
        m.classList.toggle('active', m.dataset.method === method);
      });
    });
  });

  buildImportMnemonicGrid();
}

function buildImportMnemonicGrid() {
  const grid = $('importSeedGrid');
  if (!grid || grid.dataset.ready === '1') return;

  for (let i = 0; i < 24; i++) {
    const input = document.createElement('input');
    input.type = 'text';
    input.placeholder = `Word ${i + 1}`;
    input.className = 'form-input';
    input.dataset.wordIdx = String(i);
    if (i >= 12) input.style.display = 'none';
    grid.appendChild(input);
  }

  grid.addEventListener('paste', (e) => {
    const text = (e.clipboardData || window.clipboardData).getData('text').trim();
    const words = text.split(/\s+/).filter(Boolean);
    if (words.length < 2) return;

    e.preventDefault();
    const inputs = Array.from(grid.querySelectorAll('input'));
    if (words.length > 12) inputs.forEach(inp => { inp.style.display = ''; });
    words.slice(0, 24).forEach((word, idx) => {
      if (inputs[idx]) inputs[idx].value = word.toLowerCase();
    });
  });

  grid.dataset.ready = '1';
}

function getImportMnemonicFromGrid() {
  const words = Array.from(document.querySelectorAll('#importSeedGrid input'))
    .map(i => (i.value || '').trim().toLowerCase())
    .filter(Boolean);
  return words.join(' ');
}

function normalizeImportPrivateKeyHex(privateKey) {
  const raw = String(privateKey || '').trim();
  const compact = raw.replace(/\s+/g, '').replace(/^0x/i, '');
  if (/^[0-9a-fA-F]{64}$/.test(compact)) return compact;

  const candidates = (raw.match(/(?:0x)?[0-9a-fA-F]{64}/g) || [])
    .map(candidate => candidate.replace(/^0x/i, ''))
    .filter(candidate => candidate.length === 64);

  if (candidates.length === 1) return candidates[0];
  throw new Error('Private key must be exactly 64 hex characters (0-9, a-f)');
}

async function handleImportSeed() {
  const seed = getImportMnemonicFromGrid();
  const pw = $('importPasswordSeed').value;
  if (!isValidMnemonic(seed)) { showToast('Invalid 12-word seed phrase', 'error'); return; }
  if (!pw || pw.length < 8) { showToast('Password must be at least 8 characters', 'error'); return; }

  try {
    const kp = await mnemonicToKeypair(seed);
    const encryptedKey = await encryptPrivateKey(kp.privateKey, pw);
    const encryptedMnemonic = await encryptPrivateKey(seed, pw);

    const wallet = {
      id: generateId(),
      name: `Wallet ${state.wallets.length + 1}`,
      address: kp.address,
      publicKey: kp.publicKey,
      encryptedKey,
      encryptedMnemonic,
      createdAt: new Date().toISOString()
    };

    await persist({ ...state, wallets: [...state.wallets, wallet], activeWalletId: wallet.id, isLocked: false });
    registerEvmAddress({ wallet, privateKeyHex: kp.privateKey, network: state.network?.selected, settings: state.settings }).catch(() => { });
    showToast('Wallet imported!', 'success');
    showDashboard();
  } catch (e) {
    showToast(`Import failed: ${e.message}`, 'error');
  }
}

async function handleImportPrivKey() {
  let key;
  try {
    key = normalizeImportPrivateKeyHex($('importPrivKey').value);
  } catch (e) {
    showToast(e.message, 'error');
    return;
  }
  const pw = $('importPasswordPriv').value;
  if (!pw || pw.length < 8) { showToast('Password must be at least 8 characters', 'error'); return; }

  try {
    const kp = await privateKeyToKeypair(key);
    const encryptedKey = await encryptPrivateKey(kp.privateKey, pw);

    const wallet = {
      id: generateId(),
      name: `Wallet ${state.wallets.length + 1}`,
      address: kp.address,
      publicKey: kp.publicKey,
      encryptedKey,
      encryptedMnemonic: null,
      createdAt: new Date().toISOString()
    };

    await persist({ ...state, wallets: [...state.wallets, wallet], activeWalletId: wallet.id, isLocked: false });
    registerEvmAddress({ wallet, privateKeyHex: kp.privateKey, network: state.network?.selected, settings: state.settings }).catch(() => { });
    showToast('Wallet imported!', 'success');
    showDashboard();
  } catch (e) {
    showToast(`Import failed: ${e.message}`, 'error');
  }
}

async function handleImportJson() {
  const raw = $('importJsonFile').files?.[0];
  const pw = $('importPasswordJson').value;
  if (!raw) { showToast('Choose a JSON keystore file', 'error'); return; }
  if (!pw || pw.length < 8) { showToast('Password must be at least 8 characters', 'error'); return; }

  try {
    const text = await raw.text();
    const json = JSON.parse(text);

    let kp;
    if (json.encryptedSeed) {
      const seedHex = await decryptPrivateKey(json.encryptedSeed, pw);
      kp = await privateKeyToKeypair(seedHex);
    } else if (json.privateKey) {
      if (Array.isArray(json.privateKey)) {
        if (json.privateKey.length !== 32) {
          throw new Error('privateKey array must contain 32 bytes');
        }
        kp = await privateKeyToKeypair(bytesToHex(new Uint8Array(json.privateKey)));
      } else {
        kp = await privateKeyToKeypair(String(json.privateKey).replace(/^0x/, ''));
      }
    } else if (json.seed) {
      if (Array.isArray(json.seed)) {
        if (json.seed.length !== 32) {
          throw new Error('seed array must contain 32 bytes');
        }
        kp = await privateKeyToKeypair(bytesToHex(new Uint8Array(json.seed)));
      } else {
        kp = await privateKeyToKeypair(String(json.seed).replace(/^0x/, ''));
      }
    } else {
      throw new Error('Unsupported keystore format');
    }

    const encryptedKey = await encryptPrivateKey(kp.privateKey, pw);
    const wallet = {
      id: generateId(),
      name: json.name || `Wallet ${state.wallets.length + 1}`,
      address: kp.address,
      publicKey: kp.publicKey,
      encryptedKey,
      encryptedMnemonic: null,
      createdAt: new Date().toISOString()
    };

    await persist({ ...state, wallets: [...state.wallets, wallet], activeWalletId: wallet.id, isLocked: false });
    registerEvmAddress({ wallet, privateKeyHex: kp.privateKey, network: state.network?.selected, settings: state.settings }).catch(() => { });
    showToast('Wallet imported from JSON!', 'success');
    showDashboard();
  } catch (e) {
    showToast(`JSON import failed: ${e.message}`, 'error');
  }
}

/* ──────────────────────────────────────────
   Unlock / Lock / Logout
   ────────────────────────────────────────── */
async function handleUnlock() {
  const pw = $('unlockPassword').value;
  if (!pw) { showToast('Enter your password', 'error'); return; }

  const wallet = state.wallets[0];
  if (!wallet) { showToast('No wallet found', 'error'); return; }

  try {
    await decryptPrivateKey(wallet.encryptedKey, pw);
    clearAllInputs();
    await persist({ ...state, isLocked: false });
    showToast('Wallet unlocked!', 'success');
    showDashboard();
  } catch {
    showToast('Incorrect password', 'error');
  }
}

async function handleLock() {
  clearAllInputs();
  await persist({ ...state, isLocked: true });
  showScreen('unlockScreen');
}

async function handleLogout() {
  if (!confirm('This will remove all wallets from this extension. Make sure you have your seed phrase backed up!')) return;
  clearAllInputs();
  if (typeof clearAutoLockAlarm === 'function') await clearAutoLockAlarm();
  await chrome.storage.local.clear();
  state = { wallets: [], activeWalletId: null, isLocked: false, settings: { currency: 'USD', lockTimeout: 300000 }, network: { selected: DEFAULT_NETWORK } };
  showScreen('welcomeScreen');
  showToast('Logged out');
}

/* ──────────────────────────────────────────
   Dashboard
   ────────────────────────────────────────── */
async function showDashboard() {
  showScreen('walletDashboard');
  const wallet = getActiveWallet();
  if (!wallet) return;

  $('currentWalletName').textContent = wallet.name;

  // Populate wallet dropdown
  // AUDIT-FIX FE-7: Escape wallet names to prevent XSS
  const dropdown = $('walletDropdown');
  dropdown.innerHTML = state.wallets.map(w =>
    `<div class="wallet-dropdown-item ${w.id === state.activeWalletId ? 'active' : ''}" data-wid="${escapeHtmlExt(w.id)}">
      <i class="fas fa-wallet" style="margin-right:0.5rem;"></i> ${escapeHtmlExt(w.name)} <span style="color:var(--text-muted);margin-left:auto;font-size:0.78rem;">${maskAddr(w.address)}</span>
    </div>`
  ).join('') + `
    <div class="wallet-dropdown-item" data-wid="__create" style="color:var(--primary);"><i class="fas fa-plus" style="margin-right:0.5rem;"></i> Create New Wallet</div>
    <div class="wallet-dropdown-item" data-wid="__import" style="color:var(--primary);"><i class="fas fa-download" style="margin-right:0.5rem;"></i> Import Wallet</div>
  `;

  dropdown.querySelectorAll('[data-wid]').forEach(item => {
    item.addEventListener('click', async () => {
      const wid = item.dataset.wid;
      if (wid === '__create') { showScreen('createWalletScreen'); setWizardStep(1); return; }
      if (wid === '__import') { showScreen('importWalletScreen'); return; }
      await persist({ ...state, activeWalletId: wid });
      showDashboard();
    });
  });

  // Network selector in settings
  const ns = $('networkSelect');
  if (ns) ns.value = state.network?.selected || DEFAULT_NETWORK;

  // Dashboard tabs
  setupDashboardTabs();

  // Load data
  await refreshBalance();
  await loadAssets();
  await loadActivity();
  await loadNftsTab();
  const activeTab = document.querySelector('.dashboard-tab.active')?.dataset?.tab;
  if (activeTab === 'identity') await loadIdentityTab();
}

function setupDashboardTabs() {
  document.querySelectorAll('.dashboard-tab').forEach(tab => {
    tab.addEventListener('click', () => {
      document.querySelectorAll('.dashboard-tab').forEach(t => t.classList.remove('active'));
      tab.classList.add('active');
      const name = tab.dataset.tab;
      document.querySelectorAll('.tab-content').forEach(tc => {
        tc.classList.toggle('active', tc.dataset.tab === name);
      });
      if (name === 'activity') loadActivity();
      if (name === 'assets') loadAssets();
      if (name === 'nfts') loadNftsTab();
      if (name === 'identity') loadIdentityTab();
      if (name === 'staking') loadStakingTab();
      if (name === 'shield') loadShieldTab();
    });
  });
}

function safeImageUrlExt(url) {
  if (!url) return '';
  try {
    const parsed = new URL(String(url));
    if (parsed.protocol === 'https:' || parsed.protocol === 'http:' || parsed.protocol === 'ipfs:') return parsed.href;
    return '';
  } catch {
    return '';
  }
}

async function loadNftsTab() {
  const wallet = getActiveWallet();
  const nftCount = $('nftCount');
  const nftsGrid = $('nftsGrid');
  const nftsEmpty = $('nftsEmpty');
  if (!wallet || !nftCount || !nftsGrid || !nftsEmpty) return;

  nftCount.textContent = 'Loading…';
  nftsGrid.innerHTML = '';

  try {
    const network = state?.network?.selected || DEFAULT_NETWORK;
    const items = await loadNftDetails(wallet.address, network, 50);
    nftCount.textContent = `${items.length} NFT${items.length === 1 ? '' : 's'}`;

    if (!items.length) {
      nftsEmpty.style.display = 'block';
      nftsGrid.innerHTML = '';
      return;
    }

    nftsEmpty.style.display = 'none';
    nftsGrid.innerHTML = items.map((item) => {
      const safeName = escapeHtmlExt(item.name || 'Unnamed NFT');
      const safeMint = escapeHtmlExt(item.mint || 'unknown');
      const safeStandard = escapeHtmlExt(item.standard || 'Unknown');
      const safeImage = safeImageUrlExt(item.image || '');
      const safeAmount = escapeHtmlExt(String(item.amount || 1));

      return `
        <article class="nft-card" data-mint="${safeMint}">
          <div class="nft-card-image">${safeImage ? `<img src="${safeImage}" alt="${safeName}" style="width:100%;height:100%;object-fit:cover;" />` : '<span style="color:var(--text-muted);font-size:0.85rem;">No image</span>'}</div>
          <div class="nft-card-content">
            <div class="nft-card-title">${safeName}</div>
            <div class="nft-card-subtitle">${safeStandard} • ${safeAmount}</div>
            <div class="nft-card-mint">${safeMint}</div>
          </div>
        </article>
      `;
    }).join('');
  } catch (error) {
    nftCount.textContent = '0 NFTs';
    nftsGrid.innerHTML = '';
    nftsEmpty.style.display = 'block';
    showToast(`Failed to load NFTs: ${error?.message || error}`, 'error');
  }
}

async function refreshBalance() {
  const wallet = getActiveWallet();
  if (!wallet) return;

  try {
    const result = await rpc().getBalance(wallet.address);
    const balanceSnapshot = getFullBalanceSnapshot(result);
    const licnUsdQuote = await getLiveLicnUsdPrice();
    $('totalBalance').textContent = `${balanceSnapshot.totalLicn.toLocaleString(undefined, { minimumFractionDigits: 2, maximumFractionDigits: 9 })} LICN`;
    $('balanceUsd').textContent = `$${(balanceSnapshot.totalLicn * licnUsdQuote.value).toLocaleString(undefined, { minimumFractionDigits: 2, maximumFractionDigits: 6 })} USD${licnUsdQuoteSuffix(licnUsdQuote)}`;
    $('balanceUsd').title = licnUsdQuoteTitle(licnUsdQuote);

    const breakdownEl = $('balanceBreakdown');
    if (breakdownEl) {
      const shieldedSpores = extensionShieldedBalanceSpores();
      const stakingPosition = await rpc().call('getStakingPosition', [wallet.address]).catch(() => null);
      const stLicnBalance = Number(stakingPosition?.st_licn_amount || 0) / 1_000_000_000;
      const hasBreakdown = balanceSnapshot.stakedLicn > 0 || balanceSnapshot.lockedLicn > 0 || stLicnBalance > 0 || balanceSnapshot.pendingRewardsLicn > 0 || shieldedSpores > 0n;
      if (hasBreakdown) {
        const parts = [`<i class="fas fa-wallet" style="opacity:0.5;"></i> Spendable: <strong>${balanceSnapshot.spendableLicn.toLocaleString(undefined, { maximumFractionDigits: 4 })}</strong>`];
        if (balanceSnapshot.stakedLicn > 0) parts.push(`<i class="fas fa-lock" style="opacity:0.5;"></i> Staked: <strong>${balanceSnapshot.stakedLicn.toLocaleString(undefined, { maximumFractionDigits: 4 })}</strong>`);
        if (stLicnBalance > 0) parts.push(`<i class="fas fa-coins" style="opacity:0.5;"></i> Staking: <strong>${stLicnBalance.toLocaleString(undefined, { maximumFractionDigits: 4 })} stLICN</strong>`);
        if (shieldedSpores > 0n) parts.push(`<i class="fas fa-shield-alt" style="opacity:0.5;"></i> Shielded: <strong>${formatLicnBaseUnitsFixedExt(shieldedSpores)}</strong>`);
        if (balanceSnapshot.pendingRewardsLicn > 0) parts.push(`<i class="fas fa-gift" style="opacity:0.5;"></i> Rewards: <strong>${balanceSnapshot.pendingRewardsLicn.toLocaleString(undefined, { maximumFractionDigits: 4 })}</strong>`);
        if (balanceSnapshot.lockedLicn > 0) parts.push(`<i class="fas fa-hourglass" style="opacity:0.5;"></i> Locked: <strong>${balanceSnapshot.lockedLicn.toLocaleString(undefined, { maximumFractionDigits: 4 })}</strong>`);
        breakdownEl.innerHTML = parts.join(' &nbsp;·&nbsp; ');
        breakdownEl.style.display = 'block';
      } else {
        breakdownEl.innerHTML = '';
        breakdownEl.style.display = 'none';
      }
    }
    void refreshExtensionRestrictionStatus({ updateSend: true, updateAssets: true });
  } catch {
    $('totalBalance').textContent = '0.00 LICN';
    $('balanceUsd').textContent = '$0.00 USD';
    $('balanceUsd').title = 'USD valuation source unavailable';
    if ($('balanceBreakdown')) {
      $('balanceBreakdown').innerHTML = '';
      $('balanceBreakdown').style.display = 'none';
    }
  }
}

const AGENT_TYPES = [
  { value: 0, label: 'Unknown', desc: 'Unspecified or new identity' },
  { value: 1, label: 'Trading', desc: 'Market-making, arbitrage, DeFi strategies' },
  { value: 2, label: 'Development', desc: 'Smart contracts, tooling, protocol dev' },
  { value: 3, label: 'Analysis', desc: 'On-chain analytics, data feeds, research' },
  { value: 4, label: 'Creative', desc: 'Content creation, design, media' },
  { value: 5, label: 'Infrastructure', desc: 'Validators, RPCs, indexers, relayers' },
  { value: 6, label: 'Governance', desc: 'Voting, proposals, DAO operations' },
  { value: 7, label: 'Oracle', desc: 'External data feeds, price oracles' },
  { value: 8, label: 'Storage', desc: 'Data persistence, archival, backups' },
  { value: 9, label: 'General', desc: 'Multi-purpose or uncategorized agent' },
  { value: 10, label: 'Personal', desc: 'Human user — personal identity' }
];

const TRUST_TIERS = [
  { name: 'Newcomer', min: 0, color: '#6c7a89' },
  { name: 'Verified', min: 100, color: '#3498db' },
  { name: 'Trusted', min: 500, color: '#2ecc71' },
  { name: 'Established', min: 1000, color: '#f1c40f' },
  { name: 'Elite', min: 5000, color: '#e67e22' },
  { name: 'Legendary', min: 10000, color: '#e74c3c' }
];

const ACHIEVEMENT_DEFS = [
  // Identity (1-12)
  { id: 1, name: 'First Transaction', icon: 'fas fa-exchange-alt' },
  { id: 2, name: 'Governance Voter', icon: 'fas fa-vote-yea' },
  { id: 3, name: 'Program Builder', icon: 'fas fa-code' },
  { id: 4, name: 'Trusted Agent', icon: 'fas fa-shield-alt' },
  { id: 5, name: 'Veteran Agent', icon: 'fas fa-medal' },
  { id: 6, name: 'Legendary Agent', icon: 'fas fa-crown' },
  { id: 7, name: 'Well Endorsed', icon: 'fas fa-handshake' },
  { id: 8, name: 'Bootstrap Graduation', icon: 'fas fa-graduation-cap' },
  { id: 9, name: 'Name Registrar', icon: 'fas fa-at' },
  { id: 10, name: 'Skill Master', icon: 'fas fa-tools' },
  { id: 11, name: 'Social Butterfly', icon: 'fas fa-users' },
  { id: 12, name: 'First Name', icon: 'fas fa-id-card' },
  // DEX (13-21)
  { id: 13, name: 'First Trade', icon: 'fas fa-chart-line' },
  { id: 14, name: 'LP Provider', icon: 'fas fa-water' },
  { id: 15, name: 'LP Withdrawal', icon: 'fas fa-faucet' },
  { id: 16, name: 'DEX User', icon: 'fas fa-random' },
  { id: 17, name: 'Multi-hop Trader', icon: 'fas fa-route' },
  { id: 18, name: 'Margin Trader', icon: 'fas fa-chart-bar' },
  { id: 19, name: 'Position Closer', icon: 'fas fa-compress-alt' },
  { id: 20, name: 'Yield Farmer', icon: 'fas fa-seedling' },
  { id: 21, name: 'Analytics Explorer', icon: 'fas fa-chart-pie' },
  // Lending (31-38)
  { id: 31, name: 'First Lend', icon: 'fas fa-hand-holding-usd' },
  { id: 32, name: 'First Borrow', icon: 'fas fa-file-invoice-dollar' },
  { id: 33, name: 'Loan Repaid', icon: 'fas fa-check-circle' },
  { id: 34, name: 'Liquidator', icon: 'fas fa-gavel' },
  { id: 35, name: 'Withdrawal Expert', icon: 'fas fa-sign-out-alt' },
  { id: 36, name: 'Stablecoin Minter', icon: 'fas fa-coins' },
  { id: 37, name: 'Stablecoin Redeemer', icon: 'fas fa-undo' },
  { id: 38, name: 'Stable Sender', icon: 'fas fa-paper-plane' },
  // Staking (41-48)
  { id: 41, name: 'First Stake', icon: 'fas fa-layer-group' },
  { id: 42, name: 'Unstaked', icon: 'fas fa-unlock' },
  { id: 43, name: 'Liquid Staking Pioneer', icon: 'fas fa-fish' },
  { id: 44, name: 'Locked Staker', icon: 'fas fa-lock' },
  { id: 45, name: 'Diamond Hands', icon: 'fas fa-gem' },
  { id: 46, name: 'Whale Staker', icon: 'fas fa-whale' },
  { id: 47, name: 'Reward Harvester', icon: 'fas fa-gift' },
  { id: 48, name: 'stLICN Transferrer', icon: 'fas fa-share' },
  // Bridge (51-56)
  { id: 51, name: 'Bridge Pioneer', icon: 'fas fa-bridge' },
  { id: 52, name: 'Bridge Out', icon: 'fas fa-sign-out-alt' },
  { id: 53, name: 'Bridge User', icon: 'fas fa-exchange-alt' },
  { id: 54, name: 'Wrapper', icon: 'fas fa-box' },
  { id: 55, name: 'Unwrapper', icon: 'fas fa-box-open' },
  { id: 56, name: 'Cross-chain Trader', icon: 'fas fa-globe' },
  // Shield/Privacy (57-60)
  { id: 57, name: 'Privacy Pioneer', icon: 'fas fa-user-secret' },
  { id: 58, name: 'Unshielded', icon: 'fas fa-eye' },
  { id: 59, name: 'Shadow Sender', icon: 'fas fa-mask' },
  { id: 60, name: 'ZK Privacy User', icon: 'fas fa-user-shield' },
  // NFT (63-70)
  { id: 63, name: 'Collection Creator', icon: 'fas fa-palette' },
  { id: 64, name: 'First Mint', icon: 'fas fa-stamp' },
  { id: 65, name: 'NFT Trader', icon: 'fas fa-store' },
  { id: 66, name: 'First Listing', icon: 'fas fa-tag' },
  { id: 67, name: 'First Purchase', icon: 'fas fa-shopping-cart' },
  { id: 68, name: 'Bidder', icon: 'fas fa-gavel' },
  { id: 69, name: 'Deal Maker', icon: 'fas fa-handshake' },
  { id: 70, name: 'Punk Collector', icon: 'fas fa-robot' },
  // Governance (71-73)
  { id: 71, name: 'Proposal Creator', icon: 'fas fa-scroll' },
  { id: 72, name: 'First Vote', icon: 'fas fa-ballot-check' },
  { id: 73, name: 'Delegator', icon: 'fas fa-people-arrows' },
  // Oracle (81-82)
  { id: 81, name: 'Oracle Reporter', icon: 'fas fa-satellite-dish' },
  { id: 82, name: 'Oracle User', icon: 'fas fa-broadcast-tower' },
  // Storage (86-88)
  { id: 86, name: 'File Uploader', icon: 'fas fa-cloud-upload-alt' },
  { id: 87, name: 'Data Retriever', icon: 'fas fa-cloud-download-alt' },
  { id: 88, name: 'Storage User', icon: 'fas fa-database' },
  // Marketplace/Auction (91-93)
  { id: 91, name: 'Auctioneer', icon: 'fas fa-bullhorn' },
  { id: 92, name: 'Auction Bidder', icon: 'fas fa-hand-paper' },
  { id: 93, name: 'Auction Winner', icon: 'fas fa-trophy' },
  // Bounty (96-98)
  { id: 96, name: 'Bounty Poster', icon: 'fas fa-clipboard-list' },
  { id: 97, name: 'Bounty Hunter', icon: 'fas fa-crosshairs' },
  { id: 98, name: 'Bounty Judge', icon: 'fas fa-balance-scale' },
  // Prediction (101-104)
  { id: 101, name: 'Market Maker', icon: 'fas fa-chart-area' },
  { id: 102, name: 'First Prediction', icon: 'fas fa-dice' },
  { id: 103, name: 'Oracle Resolver', icon: 'fas fa-check-double' },
  { id: 104, name: 'Prediction Winner', icon: 'fas fa-star' },
  // General milestones (106-124)
  { id: 106, name: 'Big Spender', icon: 'fas fa-money-bill-wave' },
  { id: 107, name: 'Whale Transfer', icon: 'fas fa-whale' },
  { id: 108, name: 'EVM Connected', icon: 'fas fa-link' },
  { id: 109, name: 'Identity Created', icon: 'fas fa-id-badge' },
  { id: 110, name: 'Profile Customizer', icon: 'fas fa-paint-brush' },
  { id: 111, name: 'Voucher', icon: 'fas fa-thumbs-up' },
  { id: 112, name: 'Agent Creator', icon: 'fas fa-robot' },
  { id: 113, name: 'Compute Provider', icon: 'fas fa-server' },
  { id: 114, name: 'Compute Consumer', icon: 'fas fa-microchip' },
  { id: 115, name: 'Payment Creator', icon: 'fas fa-file-invoice' },
  { id: 116, name: 'First Payment', icon: 'fas fa-credit-card' },
  { id: 117, name: 'Subscription Creator', icon: 'fas fa-calendar-check' },
  { id: 118, name: 'Token Launcher', icon: 'fas fa-rocket' },
  { id: 119, name: 'Early Buyer', icon: 'fas fa-bolt' },
  { id: 120, name: 'Token Seller', icon: 'fas fa-cash-register' },
  { id: 121, name: 'Vault Depositor', icon: 'fas fa-piggy-bank' },
  { id: 122, name: 'Vault Withdrawer', icon: 'fas fa-wallet' },
  { id: 123, name: 'Token Contract User', icon: 'fas fa-coins' },
  { id: 124, name: 'Contract Interactor', icon: 'fas fa-cog' },
];

function getTrustTier(score) {
  for (let i = TRUST_TIERS.length - 1; i >= 0; i--) {
    if (score >= TRUST_TIERS[i].min) return TRUST_TIERS[i];
  }
  return TRUST_TIERS[0];
}

function getNextTier(score) {
  for (const t of TRUST_TIERS) {
    if (score < t.min) return t;
  }
  return null;
}

function getAgentTypeName(val) {
  const t = AGENT_TYPES.find(a => a.value === Number(val));
  return t ? t.label : 'Unknown';
}

function fmtAddr(addr, len = 8) {
  if (!addr || addr.length < 16) return addr || '—';
  return addr.slice(0, len) + '…' + addr.slice(-4);
}

/* ──────────────────────────────────────────
   Staking Tab
   ────────────────────────────────────────── */
async function loadStakingTab() {
  const wallet = getActiveWallet();
  const container = $('stakingValidatorInfo');
  if (!wallet || !container) return;
  container.style.display = 'block';

  const rpcClient = rpc();

  try {
    const [poolInfo, position, queue, balance] = await Promise.all([
      rpcClient.call('getMossStakePoolInfo').catch(() => null),
      rpcClient.call('getStakingPosition', [wallet.address]).catch(() => null),
      rpcClient.call('getUnstakingQueue', [wallet.address]).catch(() => ({ pending_requests: [] })),
      rpcClient.getBalance(wallet.address).catch(() => null),
    ]);
    const currentSlot = getQueueCurrentSlotExt(queue) || await getCurrentChainSlotExt(rpcClient);
    const hasCurrentSlot = currentSlot > 0;
    const feeSpores = 1_000_000n;
    const spendableSpores = balance
      ? baseUnitBigIntExt(balance?.spendable ?? balance?.available ?? balance?.spores ?? balance?.balance ?? 0)
      : null;
    const canPayClaimFee = spendableSpores === null || spendableSpores >= feeSpores;
    const claimFeeTitle = `Need ${formatLicnBaseUnitsExactExt(feeSpores)} LICN spendable for transaction fee`;

    const stLicn = Number(position?.st_licn_amount || 0) / 1e9;
    const value = Number(position?.current_value_licn || 0) / 1e9;
    const accruedRewards = Math.max(0, Number(position?.current_value_licn || 0) - Number(position?.licn_deposited || 0)) / 1e9;
    const totalStaked = Number(poolInfo?.total_licn_staked || 0) / 1e9;
    const lockTier = position?.lock_tier_name || 'Flexible';
    const multiplier = position?.reward_multiplier || 1.0;
    const lockUntil = Number(position?.lock_until || 0);

    // Determine if position is locked
    const isLocked = lockUntil > 0 && (!hasCurrentSlot || lockUntil > currentSlot);
    const remainingDays = isLocked && hasCurrentSlot ? Math.ceil((lockUntil - currentSlot) / 216000) : 0;

    const tierDefaults = [
      { name: 'Flexible', multiplier: 1 },
      { name: '30-Day Lock', multiplier: 1.6 },
      { name: '180-Day Lock', multiplier: 2.4 },
      { name: '365-Day Lock', multiplier: 3.6 },
    ];
    const tierColors = ['#94a3b8', '#60a5fa', '#a78bfa', '#f59e0b'];
    const poolTiers = poolInfo?.tiers || [];
    const displayTiers = tierDefaults.map((fallback, i) => ({
      name: poolTiers[i]?.name || fallback.name,
      multiplier: poolTiers[i]?.multiplier ?? fallback.multiplier,
      apyPercent: poolTiers[i]?.apy_percent,
    }));

    const lockBanner = isLocked
      ? `<div style="margin-top:1rem;padding:0.75rem 1rem;background:rgba(249,115,22,0.1);border:1px solid rgba(249,115,22,0.3);border-radius:8px;font-size:0.85rem;color:#f97316;">
           <i class="fas fa-lock"></i> Position locked (${lockTier}). ${hasCurrentSlot ? `~${remainingDays} days remaining.` : `Unlocks at slot ${lockUntil.toLocaleString()}.`}
         </div>`
      : '';

    container.innerHTML = `
      <div style="background:linear-gradient(135deg,rgba(59,130,246,0.1),rgba(37,99,235,0.1));padding:1.5rem;border-radius:12px;margin-bottom:1.5rem;">
        <h3 style="margin:0 0 0.5rem 0;display:flex;align-items:center;gap:0.5rem;">
          <i class="fas fa-water" style="color:#3b82f6;"></i> Liquid Staking
        </h3>
        <p style="margin:0;font-size:0.85rem;color:var(--text-muted);">
          Stake LICN to receive stLICN. Rewards accrue into the redeemable value. Flexible stays liquid; locked tiers are position-bound for boosted rewards.
        </p>
      </div>

      <div style="display:grid;grid-template-columns:repeat(3,1fr);gap:1rem;margin-bottom:1.5rem;">
        <div style="background:var(--card-bg);padding:1rem;border-radius:10px;border:1px solid var(--border);text-align:center;">
          <div style="font-size:0.75rem;color:var(--text-muted);margin-bottom:0.25rem;">Your stLICN</div>
          <div style="font-size:1.2rem;font-weight:700;color:var(--text);">${stLicn.toLocaleString(undefined, { maximumFractionDigits: 4 })}</div>
        </div>
        <div style="background:var(--card-bg);padding:1rem;border-radius:10px;border:1px solid var(--border);text-align:center;">
          <div style="font-size:0.75rem;color:var(--text-muted);margin-bottom:0.25rem;">Redeemable Value</div>
          <div style="font-size:1.2rem;font-weight:700;color:var(--text);">${value.toLocaleString(undefined, { maximumFractionDigits: 4 })} LICN</div>
        </div>
        <div style="background:var(--card-bg);padding:1rem;border-radius:10px;border:1px solid var(--border);text-align:center;">
          <div style="font-size:0.75rem;color:var(--text-muted);margin-bottom:0.25rem;">Accrued Rewards</div>
          <div title="Tier-weighted accrued rewards. This is already included in Redeemable Value." style="font-size:1.2rem;font-weight:700;color:#10b981;">${accruedRewards.toLocaleString(undefined, { maximumFractionDigits: 4 })} LICN</div>
        </div>
      </div>

      <div style="display:grid;grid-template-columns:repeat(3,1fr);gap:0.75rem;margin-bottom:1.5rem;">
        <div style="background:var(--card-bg);padding:0.75rem;border-radius:8px;border:1px solid var(--border);text-align:center;">
          <div style="font-size:0.7rem;color:var(--text-muted);">Your Tier</div>
          <div style="font-weight:600;color:#a78bfa;">${lockTier}</div>
        </div>
        <div style="background:var(--card-bg);padding:0.75rem;border-radius:8px;border:1px solid var(--border);text-align:center;">
          <div style="font-size:0.7rem;color:var(--text-muted);">Multiplier</div>
          <div style="font-weight:600;color:var(--text);">${multiplier}x</div>
        </div>
        <div style="background:var(--card-bg);padding:0.75rem;border-radius:8px;border:1px solid var(--border);text-align:center;">
          <div style="font-size:0.7rem;color:var(--text-muted);">Total Pool</div>
          <div style="font-weight:600;color:var(--text);">${totalStaked.toLocaleString(undefined, { maximumFractionDigits: 0 })} LICN</div>
        </div>
      </div>

      <div style="display:grid;grid-template-columns:repeat(4,1fr);gap:0.75rem;margin-bottom:1.5rem;" id="fullTiersGrid">
        ${displayTiers.map((tier, i) => {
      const isActive = lockTier === tier.name && stLicn > 0;
      const apyLabel = formatMossStakeRewardLabel(tier.apyPercent, tier.multiplier);
      return `<div style="background:var(--card-bg);padding:0.75rem;border-radius:8px;border:2px solid ${isActive ? tierColors[i] : 'var(--border)'};text-align:center;">
            <div style="font-size:0.8rem;font-weight:600;color:${tierColors[i]};">${tier.name}</div>
            <div style="font-size:0.72rem;color:var(--text-muted);">${apyLabel}</div>
            ${isActive ? '<div style="font-size:0.65rem;color:#10b981;margin-top:0.25rem;"><i class="fas fa-check-circle"></i> Active</div>' : ''}
          </div>`;
    }).join('')}
      </div>

      <div style="background:var(--card-bg);padding:1rem;border-radius:10px;border:1px solid var(--border);margin-bottom:1rem;font-size:0.85rem;color:var(--text-muted);">
        <i class="fas fa-info-circle" style="color:#3b82f6;"></i>
        <strong>Flexible:</strong> 7-day target cooldown, 1x rewards.
        <strong>Locked tiers</strong> earn boosted rewards and are position-bound for the chosen duration.
      </div>

      <div style="display:grid;grid-template-columns:1fr 1fr;gap:1rem;margin-bottom:1rem;">
        <button id="fullStakeBtn" class="btn btn-primary" style="width:100%;padding:1rem;font-size:0.9rem;">
          <i class="fas fa-arrow-down"></i> Stake LICN
        </button>
        <button id="fullUnstakeBtn" class="btn btn-secondary" style="width:100%;padding:1rem;font-size:0.9rem;${isLocked ? 'opacity:0.5;cursor:not-allowed;' : ''}">
          <i class="fas fa-arrow-up"></i> Unstake stLICN
        </button>
      </div>

      ${lockBanner}

      <div id="fullPendingUnstakes" style="margin-top:1.5rem;display:none;">
        <h4 style="margin-bottom:1rem;">Pending Unstakes (slot-based cooldown)</h4>
        <div id="fullUnstakesList"></div>
      </div>
    `;

    // Pending unstakes
    const pendingReqs = queue?.pending_requests || [];
    if (pendingReqs.length > 0) {
      $('fullPendingUnstakes').style.display = 'block';
      $('fullUnstakesList').innerHTML = pendingReqs.map(req => {
        const amt = (Number(req.licn_to_receive || req.amount || 0) / 1e9).toLocaleString(undefined, { maximumFractionDigits: 4 });
        const claimable = isQueueRequestClaimableExt(req, currentSlot);
        const remainingSlots = hasCurrentSlot ? getQueueRequestRemainingSlotsExt(req, currentSlot) : 0;
        const remainingDays = hasCurrentSlot ? (remainingSlots / 216000).toFixed(1) : null;
        return `<div style="padding:0.75rem;background:var(--card-bg);border-radius:8px;border:1px solid var(--border);margin-bottom:0.5rem;display:flex;justify-content:space-between;align-items:center;">
          <span style="font-weight:600;">${amt} LICN</span>
          ${claimable
            ? canPayClaimFee
              ? '<button class="btn btn-small fullClaimBtn" style="padding:0.3rem 0.8rem;font-size:0.8rem;background:#10b981;border:none;border-radius:6px;color:#fff;cursor:pointer;font-weight:600;"><i class="fas fa-check-circle"></i> Claim</button>'
              : `<button class="btn btn-small fullClaimBtn" disabled title="${claimFeeTitle}" style="padding:0.3rem 0.8rem;font-size:0.8rem;background:#64748b;border:none;border-radius:6px;color:#fff;cursor:not-allowed;font-weight:600;opacity:0.65;"><i class="fas fa-check-circle"></i> Claim</button>`
            : `<span style="color:var(--text-muted);font-size:0.8rem;"><i class="fas fa-clock"></i> ${remainingDays ? `~${remainingDays} days` : 'Waiting for chain slot'}</span>`
          }
        </div>`;
      }).join('');

      document.querySelectorAll('.fullClaimBtn').forEach(btn => {
        btn.addEventListener('click', () => handleFullClaim());
      });
    }

    // Stake button
    $('fullStakeBtn')?.addEventListener('click', () => showStakeModal());
    // Unstake button — disabled when locked
    $('fullUnstakeBtn')?.addEventListener('click', () => {
      if (isLocked) {
        alert(`Position is locked (${lockTier}). ~${remainingDays} days remaining until unlock.`);
        return;
      }
      showUnstakeModal();
    });
  } catch (err) {
    container.innerHTML = `<div style="padding:2rem;text-align:center;color:var(--text-muted);"><i class="fas fa-exclamation-circle"></i> Failed to load staking data: ${escapeHtmlExt(err.message)}</div>`;
  }
}

async function showStakeModal() {
  const wallet = getActiveWallet();
  if (!wallet) return;

  const overlay = document.createElement('div');
  overlay.className = 'modal-overlay';
  overlay.style.cssText = 'position:fixed;top:0;left:0;width:100%;height:100%;background:rgba(0,0,0,0.6);display:flex;align-items:center;justify-content:center;z-index:10000;';
  overlay.innerHTML = `
    <div style="background:var(--bg);border:1px solid var(--border);border-radius:16px;padding:2rem;width:420px;max-width:90vw;">
      <h3 style="margin:0 0 1rem;"><i class="fas fa-layer-group" style="color:#3b82f6;"></i> Stake to Liquid Staking</h3>
      <label style="font-size:0.85rem;font-weight:600;display:block;margin-bottom:0.25rem;">Amount (LICN)</label>
      <input type="text" id="stakeAmountInput" placeholder="0.00" inputmode="decimal" data-wallet-numeric="true" data-min="0" style="width:100%;padding:0.75rem;border-radius:8px;border:1px solid var(--border);background:var(--card-bg);color:var(--text);margin-bottom:1rem;box-sizing:border-box;">
      <label style="font-size:0.85rem;font-weight:600;display:block;margin-bottom:0.25rem;">Lock Tier</label>
      <select id="stakeTierSelect" style="width:100%;padding:0.75rem;border-radius:8px;border:1px solid var(--border);background:var(--card-bg);color:var(--text);margin-bottom:1rem;box-sizing:border-box;">
        <option value="0">Flexible — 7-day target cooldown, 1x rewards</option>
        <option value="1">30-Day Lock — 1.6x rewards</option>
        <option value="2">180-Day Lock — 2.4x rewards</option>
        <option value="3">365-Day Lock — 3.6x rewards</option>
      </select>
      <label style="font-size:0.85rem;font-weight:600;display:block;margin-bottom:0.25rem;">Wallet Password</label>
      <input type="password" id="stakePasswordInput" placeholder="Enter password" style="width:100%;padding:0.75rem;border-radius:8px;border:1px solid var(--border);background:var(--card-bg);color:var(--text);margin-bottom:1.25rem;box-sizing:border-box;">
      <div style="display:flex;gap:0.75rem;">
        <button id="stakeConfirmBtn" class="btn btn-primary" style="flex:1;padding:0.75rem;">Stake LICN</button>
        <button id="stakeCancelBtn" class="btn btn-secondary" style="flex:1;padding:0.75rem;">Cancel</button>
      </div>
      <div id="stakeModalStatus" style="margin-top:0.75rem;font-size:0.85rem;text-align:center;"></div>
    </div>
  `;
  document.body.appendChild(overlay);
  applyExtensionInputGuards(overlay);

  overlay.querySelector('#stakeCancelBtn').addEventListener('click', () => overlay.remove());
  overlay.querySelector('#stakeConfirmBtn').addEventListener('click', async () => {
    const amountInput = overlay.querySelector('#stakeAmountInput');
    const amountText = amountInput.value.trim();
    const tier = parseInt(overlay.querySelector('#stakeTierSelect').value, 10);
    const password = overlay.querySelector('#stakePasswordInput').value;
    const statusEl = overlay.querySelector('#stakeModalStatus');
    let amountSpores;
    try {
      amountSpores = parseLicnAmountSporesExt(amountText, 'Stake amount');
    } catch (error) {
      statusEl.textContent = error?.message || 'Enter a valid amount';
      return;
    }
    if (!password) { statusEl.textContent = 'Password required'; return; }
    // Balance guard: check spendable LICN
    try {
      const balResult = await rpc().getBalance(wallet.address);
      const spendable = baseUnitBigIntExt(balResult?.spendable || balResult?.spores || 0);
      const feeSpores = 1_000_000n;
      const maxStakable = spendable > feeSpores ? spendable - feeSpores : 0n;
      if (maxStakable <= 0n) { statusEl.textContent = 'Insufficient LICN balance'; return; }
      if (amountSpores > maxStakable) {
        const adjusted = formatLicnBaseUnitsExactExt(maxStakable);
        amountInput.value = adjusted;
        statusEl.textContent = `Adjusted to available: ${adjusted} LICN`;
        return;
      }
    } catch (e) { /* let RPC reject */ }
    try {
      statusEl.innerHTML = '<i class="fas fa-spinner fa-spin"></i> Staking...';
      await stakeLicn({ wallet, password, amountLicn: amountText, tier, network: state.network?.selected || DEFAULT_NETWORK });
      statusEl.innerHTML = '<span style="color:#10b981;">✓ Staked successfully!</span>';
      setTimeout(() => { overlay.remove(); loadStakingTab(); }, 1500);
    } catch (err) {
      statusEl.innerHTML = `<span style="color:#ef4444;">${escapeHtmlExt(err.message)}</span>`;
    }
  });
}

async function showUnstakeModal() {
  const wallet = getActiveWallet();
  if (!wallet) return;

  const overlay = document.createElement('div');
  overlay.className = 'modal-overlay';
  overlay.style.cssText = 'position:fixed;top:0;left:0;width:100%;height:100%;background:rgba(0,0,0,0.6);display:flex;align-items:center;justify-content:center;z-index:10000;';
  overlay.innerHTML = `
    <div style="background:var(--bg);border:1px solid var(--border);border-radius:16px;padding:2rem;width:420px;max-width:90vw;">
      <h3 style="margin:0 0 1rem;"><i class="fas fa-unlock-alt" style="color:#f59e0b;"></i> Unstake from Liquid Staking</h3>
      <p style="font-size:0.85rem;color:var(--text-muted);margin-bottom:1rem;">After requesting, there is a <strong>slot-based cooldown</strong> before you can claim your LICN. The cooldown targets 7 days at normal block pace and requires a claim transaction after maturity.</p>
      <label style="font-size:0.85rem;font-weight:600;display:block;margin-bottom:0.25rem;">Amount (stLICN)</label>
      <input type="text" id="unstakeAmountInput" placeholder="0.00" inputmode="decimal" data-wallet-numeric="true" data-min="0" style="width:100%;padding:0.75rem;border-radius:8px;border:1px solid var(--border);background:var(--card-bg);color:var(--text);margin-bottom:1rem;box-sizing:border-box;">
      <label style="font-size:0.85rem;font-weight:600;display:block;margin-bottom:0.25rem;">Wallet Password</label>
      <input type="password" id="unstakePasswordInput" placeholder="Enter password" style="width:100%;padding:0.75rem;border-radius:8px;border:1px solid var(--border);background:var(--card-bg);color:var(--text);margin-bottom:1.25rem;box-sizing:border-box;">
      <div style="display:flex;gap:0.75rem;">
        <button id="unstakeConfirmBtn" class="btn btn-primary" style="flex:1;padding:0.75rem;">Unstake</button>
        <button id="unstakeCancelBtn" class="btn btn-secondary" style="flex:1;padding:0.75rem;">Cancel</button>
      </div>
      <div id="unstakeModalStatus" style="margin-top:0.75rem;font-size:0.85rem;text-align:center;"></div>
    </div>
  `;
  document.body.appendChild(overlay);
  applyExtensionInputGuards(overlay);

  overlay.querySelector('#unstakeCancelBtn').addEventListener('click', () => overlay.remove());
  overlay.querySelector('#unstakeConfirmBtn').addEventListener('click', async () => {
    const amountInput = overlay.querySelector('#unstakeAmountInput');
    const amountText = amountInput.value.trim();
    const password = overlay.querySelector('#unstakePasswordInput').value;
    const statusEl = overlay.querySelector('#unstakeModalStatus');
    let amountSpores;
    try {
      amountSpores = parseLicnAmountSporesExt(amountText, 'Unstake amount');
    } catch (error) {
      statusEl.textContent = error?.message || 'Enter a valid amount';
      return;
    }
    if (!password) { statusEl.textContent = 'Password required'; return; }
    // Balance guard: check stLICN position
    try {
      const pos = await rpc().call('getStakingPosition', [wallet.address]);
      const stLicn = baseUnitBigIntExt(pos?.st_licn_amount || 0);
      if (stLicn <= 0n) { statusEl.textContent = 'No stLICN balance to unstake'; return; }
      if (amountSpores > stLicn) {
        const adjusted = formatLicnBaseUnitsExactExt(stLicn);
        amountInput.value = adjusted;
        statusEl.textContent = `Adjusted to stLICN balance: ${adjusted}`;
        return;
      }
    } catch (e) { /* let RPC reject */ }
    try {
      statusEl.innerHTML = '<i class="fas fa-spinner fa-spin"></i> Unstaking...';
      await unstakeStLicn({ wallet, password, amountLicn: amountText, network: state.network?.selected || DEFAULT_NETWORK });
      statusEl.innerHTML = '<span style="color:#10b981;">✓ Unstake initiated. Claim after the slot-based cooldown.</span>';
      setTimeout(() => { overlay.remove(); loadStakingTab(); }, 1500);
    } catch (err) {
      statusEl.innerHTML = `<span style="color:#ef4444;">${escapeHtmlExt(err.message)}</span>`;
    }
  });
}

async function handleFullClaim() {
  const wallet = getActiveWallet();
  if (!wallet) return;

  // Balance guard: verify there is a claimable unstake and enough for fee
  try {
    const queue = await rpc().call('getUnstakingQueue', [wallet.address]);
    const pending = queue?.pending_requests || [];
    const currentSlot = getQueueCurrentSlotExt(queue) || await getCurrentChainSlotExt();
    if (currentSlot <= 0) {
      alert('Unable to confirm current chain slot');
      return;
    }
    const claimable = pending.filter(r => isQueueRequestClaimableExt(r, currentSlot));
    if (claimable.length === 0) {
      alert('No matured unstakes to claim');
      return;
    }
  } catch (e) { /* let RPC reject */ }
  try {
    const balResult = await rpc().call('getBalance', [wallet.address]);
    const spendable = baseUnitBigIntExt(balResult?.spendable ?? balResult?.available ?? balResult?.spores ?? balResult?.balance ?? 0);
    const feeSpores = 1_000_000n;
    if (spendable < feeSpores) {
      alert(`Insufficient LICN for transaction fee (need ${formatLicnBaseUnitsExactExt(feeSpores)} LICN)`);
      return;
    }
  } catch (e) { /* let RPC reject */ }

  const password = prompt('Enter wallet password to claim unstake:');
  if (!password) return;
  try {
    await claimMossStake({ wallet, password, network: state.network?.selected || DEFAULT_NETWORK });
    alert('Claim successful!');
    loadStakingTab();
  } catch (err) {
    alert('Claim failed: ' + err.message);
  }
}

// ──────────────────────────────────────────
// Shield (ZK Privacy) Tab
// ──────────────────────────────────────────
let _shieldedState = { initialized: false, balance: '0', address: null, viewingKey: null, notes: [], poolStats: null };

function extensionNoteValueSpores(note) {
  return baseUnitBigIntExt(note?.value || 0);
}

function extensionShieldedBalanceSpores() {
  return baseUnitBigIntExt(_shieldedState.balance || 0);
}

function unspentExtensionShieldedNotes() {
  return (_shieldedState.notes || []).filter((note) => !note.spent && extensionNoteValueSpores(note) > 0n);
}

async function deriveExtensionShieldedStorageKey() {
  if (!_shieldedState.spendingKey || !_shieldedState.viewingKey) return null;
  const domain = new TextEncoder().encode('lichen-extension-shielded-storage-v1');
  const material = new Uint8Array(
    _shieldedState.spendingKey.length + _shieldedState.viewingKey.length + domain.length
  );
  material.set(_shieldedState.spendingKey, 0);
  material.set(_shieldedState.viewingKey, _shieldedState.spendingKey.length);
  material.set(domain, _shieldedState.spendingKey.length + _shieldedState.viewingKey.length);
  const digest = await crypto.subtle.digest('SHA-256', material);
  zeroBytesExt(material);
  return crypto.subtle.importKey('raw', new Uint8Array(digest), { name: 'AES-GCM' }, false, ['encrypt', 'decrypt']);
}

async function loadExtensionShieldedNotes(wallet) {
  try {
    const payload = wallet?.shieldedNotes;
    if (!payload) return;
    if (payload.version !== 1 || !payload.iv || !payload.ciphertext) return;
    const key = await deriveExtensionShieldedStorageKey();
    if (!key) return;
    const decrypted = await crypto.subtle.decrypt(
      { name: 'AES-GCM', iv: hexToBytesAnyExt(payload.iv, 12) },
      key,
      hexToBytesAnyExt(payload.ciphertext),
    );
    const parsed = JSON.parse(new TextDecoder().decode(decrypted));
    _shieldedState.notes = Array.isArray(parsed.notes) ? parsed.notes : [];
  } catch (error) {
    console.warn('Failed to load shielded extension notes:', error?.message || error);
  }
}

async function saveExtensionShieldedNotes(wallet) {
  if (!wallet || !state) return;
  const key = await deriveExtensionShieldedStorageKey();
  if (!key) return;

  const encoded = new TextEncoder().encode(JSON.stringify({ notes: _shieldedState.notes || [] }));
  const iv = crypto.getRandomValues(new Uint8Array(12));
  const encrypted = await crypto.subtle.encrypt({ name: 'AES-GCM', iv }, key, encoded);
  wallet.shieldedNotes = {
    version: 1,
    iv: bytesToHex(iv),
    ciphertext: bytesToHex(new Uint8Array(encrypted)),
  };
  await saveState(state);
}

function recalculateExtensionShieldedBalance() {
  _shieldedState.balance = (_shieldedState.notes || [])
    .filter(n => !n.spent)
    .reduce((sum, note) => sum + extensionNoteValueSpores(note), 0n)
    .toString();
}

async function upsertExtensionShieldedNote(wallet, note) {
  const commitment = String(note?.commitment || '').toLowerCase();
  if (!commitment) return;
  if (!Array.isArray(_shieldedState.notes)) _shieldedState.notes = [];
  const existing = _shieldedState.notes.find((n) => String(n.commitment || '').toLowerCase() === commitment);
  if (existing) {
    Object.assign(existing, note);
  } else {
    _shieldedState.notes.push(note);
  }
  recalculateExtensionShieldedBalance();
  await saveExtensionShieldedNotes(wallet);
}

async function fetchExtensionShieldedCommitments(client, from, limit = 1000) {
  const resp = await client.call('getShieldedCommitments', [{ from, limit }]);
  return Array.isArray(resp)
    ? resp
    : (Array.isArray(resp?.commitments) ? resp.commitments : []);
}

async function encryptExtensionShieldedNoteBytes(noteBytes, encKey) {
  const iv = crypto.getRandomValues(new Uint8Array(12));
  const aesKey = await crypto.subtle.importKey('raw', encKey, { name: 'AES-GCM' }, false, ['encrypt']);
  const ciphertext = await crypto.subtle.encrypt({ name: 'AES-GCM', iv }, aesKey, noteBytes);
  return `${NOTE_ENCRYPTION_V1_PREFIX_EXT}${bytesToHex(iv)}:${bytesToHex(new Uint8Array(ciphertext))}`;
}

async function tryDecryptExtensionShieldedNote(client, entry) {
  const encryptedNote = entry?.encrypted_note || entry?.encryptedNote;
  const ephemeralPk = entry?.ephemeral_pk || entry?.ephemeralPk;
  if (!_shieldedState.viewingKey || !encryptedNote || !ephemeralPk) return null;
  if (!String(encryptedNote).startsWith(NOTE_ENCRYPTION_V1_PREFIX_EXT)) return null;

  try {
    const keyMaterial = new Uint8Array([
      ...hexToBytesAnyExt(ephemeralPk, 32),
      ..._shieldedState.viewingKey,
    ]);
    const decKeyHash = await crypto.subtle.digest('SHA-256', keyMaterial);
    const aesKey = await crypto.subtle.importKey('raw', new Uint8Array(decKeyHash), { name: 'AES-GCM' }, false, ['decrypt']);
    const parts = String(encryptedNote).split(':');
    if (parts.length !== 3) return null;
    const iv = hexToBytesAnyExt(parts[1], 12);
    const ciphertext = hexToBytesAnyExt(parts[2]);
    const decrypted = await crypto.subtle.decrypt({ name: 'AES-GCM', iv }, aesKey, ciphertext);
    const plaintext = new Uint8Array(decrypted);
    if (plaintext.length < 104) return null;
    const value = new DataView(plaintext.buffer, plaintext.byteOffset + 32, 8).getBigUint64(0, true);
    if (value <= 0n) return null;
    const blinding = bytesToHex(plaintext.slice(40, 72));
    const serial = bytesToHex(plaintext.slice(72, 104));
    const commitmentResp = await client.call('computeShieldCommitment', [{ amount: value.toString(), blinding }]).catch(() => null);
    if (!commitmentResp?.commitment || String(commitmentResp.commitment).toLowerCase() !== String(entry.commitment || '').toLowerCase()) {
      return null;
    }
    return { value: value.toString(), blinding, serial };
  } catch {
    return null;
  }
}

async function computeExtensionShieldedNullifier(client, note) {
  if (note?.nullifier) return note.nullifier;
  if (!_shieldedState.spendingKey || !note?.serial) return null;
  const resp = await client.call('computeShieldNullifier', [{
    serial: note.serial,
    spending_key: bytesToHex(_shieldedState.spendingKey),
  }]);
  return resp?.nullifier || null;
}

async function syncExtensionShieldedNotesFromChain(wallet, client) {
  if (!_shieldedState.initialized || !_shieldedState.viewingKey) return false;
  const pool = await client.call('getShieldedPoolState', []).catch(() => _shieldedState.poolStats || null);
  const total = Number(pool?.commitment_count ?? pool?.commitmentCount ?? 0);
  if (!Number.isFinite(total) || total <= 0) return false;

  const pageSize = 1000;
  const startFrom = Math.max(0, total - 10_000);
  let changed = false;
  for (let from = startFrom; from < total; from += pageSize) {
    const entries = await fetchExtensionShieldedCommitments(client, from, pageSize).catch(() => []);
    if (!entries.length) break;
    for (const entry of entries) {
      const commitment = String(entry.commitment || '').toLowerCase();
      if (!commitment) continue;
      const existing = (_shieldedState.notes || []).find((note) => String(note.commitment || '').toLowerCase() === commitment);
      const entryIndex = Number(entry.index ?? entry.commitment_index ?? entry.commitmentIndex);
      if (existing) {
        if (Number.isFinite(entryIndex) && entryIndex >= 0 && existing.index !== entryIndex) {
          existing.index = entryIndex;
          existing.pendingIndex = false;
          existing.pendingConfirmation = false;
          existing.confirmed = true;
          changed = true;
        }
        continue;
      }
      const note = await tryDecryptExtensionShieldedNote(client, entry);
      if (!note) continue;
      _shieldedState.notes.push({
        index: Number.isFinite(entryIndex) && entryIndex >= 0 ? entryIndex : null,
        value: note.value,
        blinding: note.blinding,
        serial: note.serial,
        commitment,
        pendingIndex: !Number.isFinite(entryIndex),
        pendingConfirmation: false,
        confirmed: true,
        spent: false,
        recoveredAt: Date.now(),
      });
      changed = true;
    }
    if (entries.length < pageSize) break;
  }
  for (const note of _shieldedState.notes || []) {
    if (note?.spent) continue;
    const nullifier = await computeExtensionShieldedNullifier(client, note).catch(() => null);
    if (!nullifier) continue;
    const spent = await client.call('isNullifierSpent', [nullifier]).catch(() => null);
    if (spent?.spent) {
      note.spent = true;
      note.nullifier = nullifier;
      changed = true;
    } else if (!note.nullifier) {
      note.nullifier = nullifier;
      changed = true;
    }
  }
  if (changed) {
    recalculateExtensionShieldedBalance();
    await saveExtensionShieldedNotes(wallet);
  }
  return changed;
}

async function resolveExtensionShieldedCommitmentIndex(client, commitmentHex, preferredIndex = null) {
  const normalized = String(commitmentHex || '').toLowerCase();
  if (!normalized) return null;

  const preferred = Number(preferredIndex);
  if (Number.isFinite(preferred) && preferred >= 0) {
    const probe = await fetchExtensionShieldedCommitments(client, preferred, 1).catch(() => []);
    const match = probe.find((entry) => String(entry.commitment || '').toLowerCase() === normalized);
    const matchIndex = Number(match?.index ?? match?.commitment_index ?? match?.commitmentIndex);
    if (match && Number.isFinite(matchIndex) && matchIndex >= 0) return matchIndex;
  }

  const pool = await client.call('getShieldedPoolState', []).catch(() => _shieldedState.poolStats || null);
  const total = Number(pool?.commitment_count ?? pool?.commitmentCount ?? 0);
  if (!Number.isFinite(total) || total <= 0) return null;

  const pageSize = 1000;
  for (let from = Math.max(0, total - pageSize); from >= 0; from = Math.max(0, from - pageSize)) {
    const entries = await fetchExtensionShieldedCommitments(client, from, pageSize).catch(() => []);
    const match = entries.find((entry) => String(entry.commitment || '').toLowerCase() === normalized);
    const matchIndex = Number(match?.index ?? match?.commitment_index ?? match?.commitmentIndex);
    if (match && Number.isFinite(matchIndex) && matchIndex >= 0) return matchIndex;
    if (from === 0) break;
  }
  return null;
}

async function resolveExtensionNoteCommitmentIndex(client, note) {
  const resolved = await resolveExtensionShieldedCommitmentIndex(client, note?.commitment, note?.index);
  if (!Number.isFinite(resolved) || resolved < 0) {
    throw new Error('Shielded commitment is not indexed yet; sync the shielded pool and try again');
  }
  note.index = resolved;
  note.pendingIndex = false;
  return resolved;
}

function normalizeExtensionViewingKey(value) {
  return String(value || '').trim().replace(/^0x/i, '').toLowerCase();
}

function ownExtensionViewingKeyHex() {
  return _shieldedState.viewingKey ? bytesToHex(_shieldedState.viewingKey).toLowerCase() : '';
}

function isOwnExtensionViewingKey(value) {
  const normalized = normalizeExtensionViewingKey(value);
  return Boolean(normalized && ownExtensionViewingKeyHex() && normalized === ownExtensionViewingKeyHex());
}

function selectTwoExtensionInputNotes(unspentNotes, targetAmount) {
  if (!Array.isArray(unspentNotes) || unspentNotes.length < 2) return null;
  const target = baseUnitBigIntExt(targetAmount);
  if (target <= 0n) return null;
  let bestPair = null;
  let bestExcess = null;
  for (let i = 0; i < unspentNotes.length; i++) {
    for (let j = i + 1; j < unspentNotes.length; j++) {
      const total = extensionNoteValueSpores(unspentNotes[i]) + extensionNoteValueSpores(unspentNotes[j]);
      if (total < target) continue;
      const excess = total - target;
      if (bestExcess === null || excess < bestExcess) {
        bestPair = [unspentNotes[i], unspentNotes[j]];
        bestExcess = excess;
        if (excess === 0n) return bestPair;
      }
    }
  }
  return bestPair;
}

function extensionPrivateTransferPrereqMessage() {
  if (!_shieldedState.initialized) return 'Initialize shielded privacy first';
  const unspent = unspentExtensionShieldedNotes();
  if (extensionShieldedBalanceSpores() <= 0n || unspent.length === 0) {
    return 'Shield LICN before sending a private transfer';
  }
  if (unspent.length < 2) {
    return 'Private transfer requires two unspent shielded notes';
  }
  return '';
}

async function encryptExtensionNoteForRecipient(value, blinding, serial, recipientViewingKeyHex) {
  const recipientVK = hexToBytesAnyExt(recipientViewingKeyHex, 32);
  const ephemeralKey = crypto.getRandomValues(new Uint8Array(32));
  const encKeyMaterial = new Uint8Array([...ephemeralKey, ...recipientVK]);
  const encKeyHash = await crypto.subtle.digest('SHA-256', encKeyMaterial);
  const noteBytes = new Uint8Array(104);
  noteBytes.set(recipientVK.slice(0, 32), 0);
  new DataView(noteBytes.buffer).setBigUint64(32, BigInt(value), true);
  noteBytes.set(blinding, 40);
  noteBytes.set(serial, 72);
  return {
    encryptedNote: await encryptExtensionShieldedNoteBytes(noteBytes, new Uint8Array(encKeyHash)),
    ephemeralPk: bytesToHex(ephemeralKey),
  };
}

async function assertExtensionPublicFeeBalance(type) {
  const wallet = getActiveWallet();
  if (!wallet) return;
  const zkFees = { shield: 100_000, unshield: 150_000, transfer: 200_000 };
  const required = 1_000_000 + (zkFees[type] || 0);
  const balResult = await rpc().call('getBalance', [wallet.address]);
  const spendableSpores = baseUnitBigIntExt(balResult?.spendable ?? balResult?.spores ?? balResult?.balance ?? 0);
  const requiredSpores = BigInt(required);
  if (spendableSpores < requiredSpores) {
    throw new Error(`Insufficient public LICN for fee: need ${formatLicnBaseUnitsFixedExt(requiredSpores)} LICN`);
  }
}

async function signAndSubmitExtensionShieldedInstruction({ wallet, password, instructionDataBytes }) {
  const client = rpc();
  const privateKeyHex = await decryptPrivateKey(wallet.encryptedKey, password);
  const blockhash = await client.getRecentBlockhash();
  const tx = await buildSignedSingleInstructionTransaction({
    privateKeyHex,
    fromAddress: wallet.address,
    blockhash,
    instructionDataBytes,
  });
  return client.sendTransactionWithPreflight(encodeTransactionBase64(tx));
}

async function submitExtensionShield({ wallet, amountLicn, password, statusEl }) {
  const amountSpores = parseLicnAmountSporesExt(amountLicn, 'Shield amount');

  const client = rpc();
  if (!_shieldedState.initialized) {
    const ok = await ensureShieldedStateInitialized(wallet, password);
    if (!ok) throw new Error('Shielded wallet not initialized');
  }

  statusEl.textContent = 'Generating shield proof...';
  const blinding = crypto.getRandomValues(new Uint8Array(32));
  const serial = crypto.getRandomValues(new Uint8Array(32));
  const noteBytes = new Uint8Array(104);
  const ownerBytes = base58Decode(_shieldedState.address);
  noteBytes.set(ownerBytes.slice(0, 32), 0);
  new DataView(noteBytes.buffer).setBigUint64(32, amountSpores, true);
  noteBytes.set(blinding, 40);
  noteBytes.set(serial, 72);
  const ephemeralKey = crypto.getRandomValues(new Uint8Array(32));
  const encKeyMaterial = new Uint8Array([...ephemeralKey, ..._shieldedState.viewingKey]);
  const encKeyHash = await crypto.subtle.digest('SHA-256', encKeyMaterial);
  const encryptedNote = await encryptExtensionShieldedNoteBytes(noteBytes, new Uint8Array(encKeyHash));
  const proof = await client.call('generateShieldProof', [{
    amount: amountSpores.toString(),
    blinding: bytesToHex(blinding),
    commitment: null,
  }]);
  const commitmentIndex = Number(
    _shieldedState.poolStats?.commitmentCount
    ?? _shieldedState.poolStats?.commitment_count
    ?? _shieldedState.notes.length
    ?? 0
  );

  statusEl.textContent = 'Submitting signed transaction...';
  const signature = await signAndSubmitExtensionShieldedInstruction({
    wallet,
    password,
    instructionDataBytes: buildExtShieldInstructionData(
      amountSpores,
      proof.commitment,
      hexToBytesAnyExt(proof.proof),
      encryptedNote,
      bytesToHex(ephemeralKey),
    ),
  });

  const ownedNote = {
    index: null,
    value: amountSpores.toString(),
    blinding: bytesToHex(blinding),
    serial: bytesToHex(serial),
    commitment: proof.commitment,
    pendingIndex: true,
    pendingConfirmation: true,
    signature,
    spent: false,
    createdAt: Date.now(),
    broadcastAt: Date.now(),
  };
  await upsertExtensionShieldedNote(wallet, ownedNote);

  let resolvedIndex = null;
  try {
    resolvedIndex = await resolveExtensionShieldedCommitmentIndex(
      client,
      proof.commitment,
      commitmentIndex,
    );
  } catch (error) {
    console.warn('Shielded commitment index not resolved yet:', error?.message || error);
  }

  await upsertExtensionShieldedNote(wallet, {
    ...ownedNote,
    index: Number.isFinite(resolvedIndex) ? resolvedIndex : null,
    pendingIndex: !Number.isFinite(resolvedIndex),
    pendingConfirmation: false,
    confirmed: true,
    confirmedAt: Date.now(),
  });
  return signature;
}

async function submitExtensionUnshield({ wallet, amountLicn, password, recipient, statusEl }) {
  if (!_shieldedState.initialized) {
    const ok = await ensureShieldedStateInitialized(wallet, password);
    if (!ok) throw new Error('Shielded wallet not initialized');
  }
  if (recipient !== wallet.address) {
    throw new Error('Unshield currently requires the active wallet as recipient');
  }

  const amountSpores = parseLicnAmountSporesExt(amountLicn, 'Unshield amount');
  const note = (_shieldedState.notes || []).find((n) => !n.spent && extensionNoteValueSpores(n) === amountSpores);
  if (!note) throw new Error('Unshield currently requires a single note exactly matching the amount');

  const client = rpc();
  const pool = await client.call('getShieldedPoolState', []).catch(() => _shieldedState.poolStats);
  const noteIndex = await resolveExtensionNoteCommitmentIndex(client, note);
  const merklePath = await client.call('getShieldedMerklePath', [noteIndex]);
  const merkleRoot = merklePath?.root || merklePath?.merkleRoot || merklePath?.merkle_root || pool?.merkleRoot || pool?.merkle_root;
  if (!merkleRoot) throw new Error('Shielded Merkle root unavailable');
  _shieldedState.poolStats = { ...(_shieldedState.poolStats || {}), ...(pool || {}), merkleRoot };

  statusEl.textContent = 'Generating unshield proof...';
  const proof = await client.call('generateUnshieldProof', [{
    amount: amountSpores.toString(),
    merkle_root: merkleRoot,
    recipient,
    blinding: note.blinding,
    serial: note.serial,
    spending_key: bytesToHex(_shieldedState.spendingKey),
    merkle_path: merklePath?.siblings || [],
    path_bits: merklePath?.pathBits || merklePath?.path_bits || [],
  }]);

  statusEl.textContent = 'Submitting signed transaction...';
  const signature = await signAndSubmitExtensionShieldedInstruction({
    wallet,
    password,
    instructionDataBytes: buildExtUnshieldInstructionData(
      amountSpores,
      proof.nullifier,
      proof.merkle_root,
      proof.recipient_hash,
      hexToBytesAnyExt(proof.proof),
    ),
  });

  note.spent = true;
  note.nullifier = proof.nullifier;
  recalculateExtensionShieldedBalance();
  await saveExtensionShieldedNotes(wallet);
  return signature;
}

async function submitExtensionPrivateTransfer({ wallet, amountLicn, password, recipientViewingKey, statusEl }) {
  if (!_shieldedState.initialized) {
    const ok = await ensureShieldedStateInitialized(wallet, password);
    if (!ok) throw new Error('Shielded wallet not initialized');
  }

  const normalizedViewingKey = normalizeExtensionViewingKey(recipientViewingKey);
  if (!/^[0-9a-f]{64}$/.test(normalizedViewingKey)) {
    throw new Error('Enter a valid recipient viewing key');
  }
  if (isOwnExtensionViewingKey(normalizedViewingKey)) {
    throw new Error('Private transfers to your own viewing key are not allowed');
  }

  const amountSpores = parseLicnAmountSporesExt(amountLicn, 'Private transfer amount');
  const unspentNotes = unspentExtensionShieldedNotes();
  const inputNotes = selectTwoExtensionInputNotes(unspentNotes, amountSpores);
  if (!inputNotes) {
    throw new Error('Private transfer requires two unspent shielded notes with enough balance');
  }

  const client = rpc();
  await assertExtensionPublicFeeBalance('transfer');
  const inputTotal = inputNotes.reduce((sum, note) => sum + extensionNoteValueSpores(note), 0n);
  const changeAmount = inputTotal - amountSpores;

  statusEl.textContent = 'Generating private transfer proof...';
  const inputIndices = await Promise.all(inputNotes.map((note) => resolveExtensionNoteCommitmentIndex(client, note)));
  const merkleWitnesses = await Promise.all(inputIndices.map((index) => client.call('getShieldedMerklePath', [index])));
  const pool = await client.call('getShieldedPoolState', []).catch(() => _shieldedState.poolStats);
  const merkleRoot = merkleWitnesses[0]?.root
    || merkleWitnesses[0]?.merkleRoot
    || merkleWitnesses[0]?.merkle_root
    || pool?.merkleRoot
    || pool?.merkle_root;
  if (!merkleRoot) throw new Error('Shielded Merkle root unavailable');

  const recipientBlinding = crypto.getRandomValues(new Uint8Array(32));
  const recipientSerial = crypto.getRandomValues(new Uint8Array(32));
  const changeBlinding = crypto.getRandomValues(new Uint8Array(32));
  const changeSerial = crypto.getRandomValues(new Uint8Array(32));

  const proof = await client.call('generateTransferProof', [{
    merkle_root: merkleRoot,
    inputs: inputNotes.map((note, index) => ({
      amount: extensionNoteValueSpores(note).toString(),
      blinding: note.blinding,
      serial: note.serial,
      spending_key: bytesToHex(_shieldedState.spendingKey),
      merkle_path: merkleWitnesses[index]?.siblings || [],
      path_bits: merkleWitnesses[index]?.pathBits || merkleWitnesses[index]?.path_bits || [],
    })),
    outputs: [
      { amount: amountSpores.toString(), blinding: bytesToHex(recipientBlinding) },
      { amount: changeAmount.toString(), blinding: bytesToHex(changeBlinding) },
    ],
  }]);

  const recipientEnc = await encryptExtensionNoteForRecipient(
    amountSpores,
    recipientBlinding,
    recipientSerial,
    normalizedViewingKey,
  );
  const changeEnc = await encryptExtensionNoteForRecipient(
    changeAmount,
    changeBlinding,
    changeSerial,
    bytesToHex(_shieldedState.viewingKey),
  );
  const outputCommitments = [
    {
      commitment: proof.commitment_c,
      encrypted_note: recipientEnc.encryptedNote,
      ephemeral_pk: recipientEnc.ephemeralPk,
    },
    {
      commitment: proof.commitment_d,
      encrypted_note: changeEnc.encryptedNote,
      ephemeral_pk: changeEnc.ephemeralPk,
    },
  ];

  statusEl.textContent = 'Submitting signed transaction...';
  const signature = await signAndSubmitExtensionShieldedInstruction({
    wallet,
    password,
    instructionDataBytes: buildExtTransferInstructionData(
      [proof.nullifier_a, proof.nullifier_b],
      outputCommitments,
      merkleRoot,
      hexToBytesAnyExt(proof.proof),
    ),
  });

  inputNotes.forEach((note, index) => {
    note.spent = true;
    note.nullifier = index === 0 ? proof.nullifier_a : proof.nullifier_b;
  });
  if (changeAmount > 0n) {
    await upsertExtensionShieldedNote(wallet, {
      index: null,
      value: changeAmount.toString(),
      blinding: bytesToHex(changeBlinding),
      serial: bytesToHex(changeSerial),
      commitment: proof.commitment_d,
      pendingIndex: true,
      pendingConfirmation: true,
      signature,
      spent: false,
      createdAt: Date.now(),
      broadcastAt: Date.now(),
    });
  } else {
    recalculateExtensionShieldedBalance();
    await saveExtensionShieldedNotes(wallet);
  }

  setTimeout(() => syncExtensionShieldedNotesFromChain(wallet, client).catch(() => {}), 1500);
  setTimeout(() => syncExtensionShieldedNotesFromChain(wallet, client).catch(() => {}), 5000);
  return signature;
}

async function deriveShieldedSeedForWallet(wallet, password) {
  if (!wallet?.encryptedKey) return null;

  let decryptedSeedHex = null;
  try {
    if (wallet.encryptedMnemonic) {
      try {
        const mnemonic = await decryptPrivateKey(wallet.encryptedMnemonic, password);
        if (mnemonic && isValidMnemonic(mnemonic)) {
          const keypair = await mnemonicToKeypair(mnemonic);
          decryptedSeedHex = keypair.privateKey;
          zeroBytesExt(keypair.seed);
        }
      } catch {
        // Fall back to the encrypted private key path.
      }
    }

    if (!decryptedSeedHex) {
      decryptedSeedHex = await decryptPrivateKey(wallet.encryptedKey, password);
    }

    const domain = new TextEncoder().encode('lichen-shielded-spending-seed-v1');
    const seedBytes = hexToBytesExt(decryptedSeedHex);
    const keyMaterial = new Uint8Array(seedBytes.length + domain.length);
    keyMaterial.set(seedBytes, 0);
    keyMaterial.set(domain, seedBytes.length);

    const digest = await crypto.subtle.digest('SHA-256', keyMaterial);
    const shieldSeed = new Uint8Array(digest);

    zeroBytesExt(seedBytes);
    zeroBytesExt(keyMaterial);
    return shieldSeed;
  } finally {
    decryptedSeedHex = null;
  }
}

async function ensureShieldedStateInitialized(wallet, providedPassword = null) {
  if (_shieldedState.initialized && _shieldedState.address && _shieldedState.viewingKey) {
    return true;
  }

  const password = providedPassword || await securePasswordPrompt('Enter your wallet password to initialize shielded privacy.');
  if (!password) {
    showToast('Shielded initialization cancelled', 'info');
    return false;
  }

  let shieldSeed = null;
  let spendingKey = null;
  try {
    shieldSeed = await deriveShieldedSeedForWallet(wallet, password);
    if (!shieldSeed) return false;

    const encoder = new TextEncoder();
    spendingKey = new Uint8Array(await crypto.subtle.digest('SHA-256', new Uint8Array([...shieldSeed, ...encoder.encode('lichen-shielded-spending-key-v1')])));
    const viewingKey = new Uint8Array(await crypto.subtle.digest('SHA-256', new Uint8Array([...spendingKey, ...encoder.encode('lichen-viewing-key-v1')])));
    const addressDigest = await crypto.subtle.digest('SHA-256', viewingKey);
    const shieldedAddress = base58Encode(new Uint8Array(addressDigest).slice(0, 32));

    _shieldedState = {
      ..._shieldedState,
      initialized: true,
      address: shieldedAddress,
      spendingKey: new Uint8Array(spendingKey),
      viewingKey: new Uint8Array(viewingKey),
    };
    await loadExtensionShieldedNotes(wallet);
    showToast('Shielded privacy ready', 'success');
    return true;
  } catch (error) {
    showToast(`Shielded initialization failed: ${error?.message || error}`, 'error');
    return false;
  } finally {
    zeroBytesExt(shieldSeed);
    zeroBytesExt(spendingKey);
  }
}

async function loadShieldTab() {
  const wallet = getActiveWallet();
  const container = $('shieldContent');
  if (!wallet || !container) return;

  const rpcClient = rpc();

  if (!_shieldedState.initialized) {
    await ensureShieldedStateInitialized(wallet);
  }

  // Fetch pool stats + shielded balance
  let poolStats = null;
  try {
    const res = await rpcClient.call('getShieldedPoolState', []).catch(() => rpcClient.call('getShieldedPoolStats', []));
    poolStats = res || null;
  } catch (_) { }

  await syncExtensionShieldedNotesFromChain(wallet, rpcClient);

  let shieldedBalance = 0n;
  let ownedNotes = [];
  let shieldedAddress = _shieldedState.address || 'Initialize shielded wallet to derive';
  ownedNotes = Array.isArray(_shieldedState.notes) ? _shieldedState.notes : [];
  let resolvedAnyNoteIndex = false;
  for (const note of ownedNotes) {
    if (note?.spent || !note?.commitment) continue;
    const hasIndex = Number.isFinite(Number(note.index)) && Number(note.index) >= 0;
    if (hasIndex && !note.pendingIndex) continue;
    try {
      const resolved = await resolveExtensionShieldedCommitmentIndex(rpcClient, note.commitment, note.index);
      if (Number.isFinite(resolved) && resolved >= 0) {
        note.index = resolved;
        note.pendingIndex = false;
        note.pendingConfirmation = false;
        note.confirmed = true;
        resolvedAnyNoteIndex = true;
      }
    } catch (_) { /* leave pending; unshield will re-check */ }
  }
  if (resolvedAnyNoteIndex) {
    await saveExtensionShieldedNotes(wallet);
  }
  shieldedBalance = ownedNotes.filter(n => !n.spent).reduce((s, n) => s + extensionNoteValueSpores(n), 0n);

  _shieldedState = { ..._shieldedState, balance: shieldedBalance.toString(), address: shieldedAddress, notes: ownedNotes, poolStats };
  void refreshBalance();

  const balLicn = formatLicnBaseUnitsFixedExt(shieldedBalance);
  const poolLicn = poolStats ? formatLicnBaseUnitsFixedExt(poolStats.pool_balance || 0, 2) : '—';
  const commitCount = poolStats ? (poolStats.commitment_count || poolStats.commitmentCount || 0).toLocaleString() : '—';
  const unspent = ownedNotes.filter(n => !n.spent);
  const transferPrereq = extensionPrivateTransferPrereqMessage();

  const notesHtml = unspent.length > 0
    ? unspent.map(n => {
      const label = n.pendingConfirmation ? 'Submitted' : (n.pendingIndex ? 'Indexing' : 'Unspent');
      const icon = n.pendingConfirmation || n.pendingIndex ? 'fas fa-clock' : 'fas fa-check-circle';
      return `
        <div style="padding:0.75rem;background:var(--card-bg);border-radius:8px;border:1px solid var(--border);margin-bottom:0.5rem;display:flex;justify-content:space-between;align-items:center;">
          <div>
            <div style="font-weight:600;"><i class="fas fa-lock" style="color:#10b981;margin-right:0.25rem;"></i>${formatLicnBaseUnitsFixedExt(n.value)} LICN</div>
            <div style="font-size:0.7rem;color:var(--text-muted);">Note #${n.index ?? '?'} &bull; ${(n.commitment || '').slice(0, 12)}...</div>
          </div>
          <span style="font-size:0.7rem;background:rgba(16,185,129,0.1);color:#10b981;padding:0.2rem 0.5rem;border-radius:4px;"><i class="${icon}"></i> ${label}</span>
        </div>`;
    }).join('')
    : `<div style="text-align:center;padding:1.5rem;color:var(--text-muted);">
        <i class="fas fa-shield-alt" style="font-size:1.5rem;opacity:0.4;display:block;margin-bottom:0.5rem;"></i>
        <p style="margin:0 0 0.25rem;">No shielded notes yet</p>
        <p style="margin:0;font-size:0.8rem;">Shield LICN to create your first private note</p>
      </div>`;

  container.innerHTML = `
    <div style="background:linear-gradient(135deg,rgba(16,185,129,0.1),rgba(5,150,105,0.08));padding:1.5rem;border-radius:12px;margin-bottom:1.5rem;border:1px solid rgba(16,185,129,0.12);">
      <h3 style="margin:0 0 0.5rem 0;display:flex;align-items:center;gap:0.5rem;">
        <i class="fas fa-user-shield" style="color:#10b981;"></i> Shielded Privacy
        <span style="font-size:0.65rem;background:rgba(16,185,129,0.15);color:#10b981;padding:0.15rem 0.5rem;border-radius:4px;font-weight:600;">Plonky3 STARK</span>
      </h3>
      <p style="margin:0;font-size:0.85rem;color:var(--text-muted);">Shield LICN with transparent STARK proofs. Notes keep amounts and transfer links private while preserving auditable execution.</p>
    </div>

    <div style="background:var(--card-bg);padding:1.25rem;border-radius:12px;border:1px solid var(--border);margin-bottom:1.25rem;">
      <div style="display:flex;justify-content:space-between;align-items:center;margin-bottom:1rem;">
        <div>
          <div style="font-size:0.75rem;color:var(--text-muted);">Shielded Balance</div>
          <div style="font-size:1.4rem;font-weight:700;color:var(--text);">${balLicn} LICN</div>
          <div style="font-size:0.7rem;color:var(--text-muted);">${shieldedBalance.toLocaleString()} spores</div>
        </div>
        <div style="display:flex;gap:0.5rem;">
          <button class="btn btn-small btn-primary" id="extShieldBtn"><i class="fas fa-arrow-down"></i> Shield</button>
          <button class="btn btn-small btn-secondary" id="extUnshieldBtn" ${unspent.length === 0 ? 'disabled title="No shielded balance"' : ''}><i class="fas fa-arrow-up"></i> Unshield</button>
        </div>
      </div>
      <button class="btn btn-primary ${transferPrereq ? 'is-disabled' : ''}" id="extPrivateTransferBtn" style="width:100%;padding:0.75rem;opacity:${transferPrereq ? '0.62' : '1'};" aria-disabled="${transferPrereq ? 'true' : 'false'}" title="${escapeHtmlExt(transferPrereq)}">
        <i class="fas fa-paper-plane"></i> Private Transfer
      </button>
      <div style="min-height:1rem;margin-top:0.45rem;text-align:center;font-size:0.72rem;color:var(--text-muted);">${escapeHtmlExt(transferPrereq)}</div>
    </div>

    <div style="background:var(--card-bg);padding:1rem;border-radius:12px;border:1px solid var(--border);margin-bottom:1.25rem;">
      <h4 style="margin:0 0 0.75rem;font-size:0.9rem;"><i class="fas fa-key" style="color:var(--text-muted);"></i> Shielded Keys</h4>
      <div style="margin-bottom:0.5rem;">
        <div style="font-size:0.7rem;color:var(--text-muted);margin-bottom:0.15rem;">Shielded Address</div>
        <div style="display:flex;align-items:center;gap:0.5rem;">
          <code style="font-size:0.75rem;word-break:break-all;flex:1;" id="extShieldedAddr">${escapeHtmlExt(String(shieldedAddress))}</code>
          <button class="btn-icon" id="extCopyShieldAddr" title="Copy"><i class="fas fa-copy"></i></button>
        </div>
      </div>
      <div>
        <div style="font-size:0.7rem;color:var(--text-muted);margin-bottom:0.15rem;">Viewing Key</div>
        <div style="display:flex;align-items:center;gap:0.5rem;">
          <code style="font-size:0.75rem;flex:1;word-break:break-all;">${_shieldedState.viewingKey ? escapeHtmlExt(bytesToHex(_shieldedState.viewingKey)) : 'Initialize shielded wallet to reveal'}</code>
          <button class="btn-icon" id="extCopyViewKey" title="Copy"><i class="fas fa-eye"></i></button>
        </div>
      </div>
      <div style="margin-top:0.75rem;padding:0.6rem;background:rgba(59,130,246,0.06);border-radius:8px;font-size:0.75rem;color:var(--text-muted);">
        <i class="fas fa-info-circle" style="color:#3b82f6;"></i>
        Your spending key never leaves this device. Viewing key enables auditors to see your shielded activity without spending.
      </div>
    </div>

    <div style="display:grid;grid-template-columns:1fr 1fr;gap:0.75rem;margin-bottom:1.25rem;">
      <div style="background:var(--card-bg);padding:0.75rem;border-radius:8px;border:1px solid var(--border);text-align:center;">
        <div style="font-size:0.7rem;color:var(--text-muted);">Total Shielded</div>
        <div style="font-weight:600;color:var(--text);">${poolLicn} LICN</div>
      </div>
      <div style="background:var(--card-bg);padding:0.75rem;border-radius:8px;border:1px solid var(--border);text-align:center;">
        <div style="font-size:0.7rem;color:var(--text-muted);">Commitments</div>
        <div style="font-weight:600;color:var(--text);">${commitCount}</div>
      </div>
    </div>

    <div style="background:var(--card-bg);padding:1rem;border-radius:12px;border:1px solid var(--border);">
      <div style="display:flex;justify-content:space-between;align-items:center;margin-bottom:0.75rem;">
        <h4 style="margin:0;font-size:0.9rem;"><i class="fas fa-file-invoice" style="color:var(--text-muted);"></i> Shielded Notes</h4>
        <span style="font-size:0.75rem;color:var(--text-muted);">${unspent.length} unspent / ${ownedNotes.length} total</span>
      </div>
      ${notesHtml}
    </div>
  `;

  // Wire buttons
  $('extShieldBtn')?.addEventListener('click', () => showShieldModal('shield'));
  $('extUnshieldBtn')?.addEventListener('click', () => showShieldModal('unshield'));
  $('extPrivateTransferBtn')?.addEventListener('click', () => {
    const message = extensionPrivateTransferPrereqMessage();
    if (message) { showToast(message, 'info'); return; }
    showShieldModal('transfer');
  });
  $('extCopyShieldAddr')?.addEventListener('click', () => {
    if (_shieldedState.address) { navigator.clipboard.writeText(_shieldedState.address); showToast('Shielded address copied', 'success'); }
  });
  $('extCopyViewKey')?.addEventListener('click', () => {
    if (_shieldedState.viewingKey) {
      navigator.clipboard.writeText(bytesToHex(_shieldedState.viewingKey));
      showToast('Viewing key copied', 'success');
      return;
    }
    showToast('Viewing key is unavailable until shielded privacy is initialized', 'info');
  });
}

function showShieldModal(type) {
  if (type === 'transfer') {
    const prereq = extensionPrivateTransferPrereqMessage();
    if (prereq) { showToast(prereq, 'info'); return; }
  }
  const titles = { shield: 'Shield LICN', unshield: 'Unshield LICN', transfer: 'Private Transfer' };
  const icons = { shield: 'fa-arrow-down', unshield: 'fa-arrow-up', transfer: 'fa-paper-plane' };

  const activeWalletForModal = getActiveWallet();
  const extraField = type === 'unshield'
    ? `<label style="font-size:0.85rem;font-weight:600;display:block;margin-bottom:0.25rem;">Recipient Address</label>
       <input type="text" id="shieldModalRecipient" value="${escapeHtmlExt(activeWalletForModal?.address || '')}" placeholder="Active wallet address" readonly aria-readonly="true" data-address-input="base58" title="Unshield returns to the active wallet address" style="width:100%;padding:0.75rem;border-radius:8px;border:1px solid var(--border);background:var(--card-bg);color:var(--text);margin-bottom:1rem;box-sizing:border-box;">`
    : type === 'transfer'
      ? `<label style="font-size:0.85rem;font-weight:600;display:block;margin-bottom:0.25rem;">Recipient Viewing Key</label>
       <input type="text" id="shieldModalRecipient" placeholder="64-char hex viewing key" data-hex-input="true" maxlength="64" style="width:100%;padding:0.75rem;border-radius:8px;border:1px solid var(--border);background:var(--card-bg);color:var(--text);margin-bottom:0.35rem;box-sizing:border-box;">
       <div id="shieldModalValidation" style="font-size:0.75rem;color:var(--text-muted);margin-bottom:1rem;"></div>`
      : '';

  const overlay = document.createElement('div');
  overlay.className = 'modal-overlay';
  overlay.style.cssText = 'position:fixed;top:0;left:0;width:100%;height:100%;background:rgba(0,0,0,0.6);display:flex;align-items:center;justify-content:center;z-index:10000;';
  overlay.innerHTML = `
    <div style="background:var(--bg);border:1px solid var(--border);border-radius:16px;padding:2rem;width:420px;max-width:90vw;">
      <h3 style="margin:0 0 1rem;"><i class="fas ${icons[type]}" style="color:#10b981;"></i> ${titles[type]}</h3>
      <label style="font-size:0.85rem;font-weight:600;display:block;margin-bottom:0.25rem;">Amount (LICN)</label>
      <input type="text" id="shieldModalAmount" placeholder="0.00" inputmode="decimal" data-wallet-numeric="true" data-min="0" style="width:100%;padding:0.75rem;border-radius:8px;border:1px solid var(--border);background:var(--card-bg);color:var(--text);margin-bottom:1rem;box-sizing:border-box;">
      ${extraField}
      <label style="font-size:0.85rem;font-weight:600;display:block;margin-bottom:0.25rem;">Wallet Password</label>
      <input type="password" id="shieldModalPassword" placeholder="Enter password" style="width:100%;padding:0.75rem;border-radius:8px;border:1px solid var(--border);background:var(--card-bg);color:var(--text);margin-bottom:1.25rem;box-sizing:border-box;">
      <div style="display:flex;gap:0.75rem;">
        <button id="shieldModalConfirm" class="btn btn-primary" style="flex:1;padding:0.75rem;">${titles[type]}</button>
        <button id="shieldModalCancel" class="btn btn-secondary" style="flex:1;padding:0.75rem;">Cancel</button>
      </div>
      <div id="shieldModalStatus" style="margin-top:0.75rem;font-size:0.85rem;text-align:center;"></div>
    </div>
  `;
  document.body.appendChild(overlay);
  applyExtensionInputGuards(overlay);

  const confirmBtn = overlay.querySelector('#shieldModalConfirm');
  const amountInput = overlay.querySelector('#shieldModalAmount');
  const passwordInput = overlay.querySelector('#shieldModalPassword');
  const recipientInput = overlay.querySelector('#shieldModalRecipient');
  const validationEl = overlay.querySelector('#shieldModalValidation');
  const statusLine = overlay.querySelector('#shieldModalStatus');
  const modalValidationMessage = () => {
    const amountText = amountInput?.value || '';
    let amountSpores;
    try {
      amountSpores = parseLicnAmountSporesExt(amountText, `${titles[type]} amount`);
    } catch (error) {
      return amountText.trim() ? (error?.message || 'Enter a valid amount') : 'Enter a valid amount';
    }
    if (!passwordInput?.value) return 'Password required';
    if (type !== 'shield' && !recipientInput?.value) return 'Recipient required';
    if (type === 'unshield') {
      const shieldedBal = extensionShieldedBalanceSpores();
      if (shieldedBal <= 0n) return 'No shielded balance available';
      if (amountSpores > shieldedBal) return `Max available: ${formatLicnBaseUnitsFixedExt(shieldedBal)} LICN`;
    }
    if (type === 'transfer') {
      const recipient = normalizeExtensionViewingKey(recipientInput?.value || '');
      const shieldedBal = extensionShieldedBalanceSpores();
      if (!/^[0-9a-f]{64}$/.test(recipient)) return 'Enter a valid recipient viewing key';
      if (isOwnExtensionViewingKey(recipient)) return 'Private transfers to your own viewing key are not allowed';
      if (shieldedBal <= 0n) return 'No shielded balance available';
      if (amountSpores > shieldedBal) return `Max available: ${formatLicnBaseUnitsFixedExt(shieldedBal)} LICN`;
      const inputNotes = selectTwoExtensionInputNotes(
        unspentExtensionShieldedNotes(),
        amountSpores,
      );
      if (!inputNotes) return 'Private transfer requires two unspent shielded notes with enough balance';
    }
    return '';
  };
  const refreshModalValidation = () => {
    const message = modalValidationMessage();
    if (confirmBtn) {
      confirmBtn.disabled = Boolean(message);
      confirmBtn.title = message;
    }
    if (type === 'transfer' && validationEl) validationEl.textContent = message;
    if (type !== 'transfer' && statusLine && !statusLine.dataset.busy) statusLine.textContent = message;
  };
  [amountInput, passwordInput, recipientInput].forEach((input) => input?.addEventListener('input', refreshModalValidation));
  refreshModalValidation();

  overlay.querySelector('#shieldModalCancel').addEventListener('click', () => overlay.remove());
  overlay.querySelector('#shieldModalConfirm').addEventListener('click', async () => {
    const amountText = overlay.querySelector('#shieldModalAmount').value.trim();
    const password = overlay.querySelector('#shieldModalPassword').value;
    const wallet = getActiveWallet();
    const recipient = type === 'unshield'
      ? (wallet?.address || '')
      : normalizeExtensionViewingKey(overlay.querySelector('#shieldModalRecipient')?.value || '');
    const statusEl = overlay.querySelector('#shieldModalStatus');

    let amountSpores;
    try {
      amountSpores = parseLicnAmountSporesExt(amountText, `${titles[type]} amount`);
    } catch (error) {
      statusEl.textContent = error?.message || 'Enter a valid amount';
      return;
    }
    if (!password) { statusEl.textContent = 'Password required'; return; }
    if (type !== 'shield' && !recipient) { statusEl.textContent = 'Recipient required'; return; }
    if (type === 'transfer') {
      if (!/^[0-9a-f]{64}$/.test(recipient)) { statusEl.textContent = 'Enter a valid recipient viewing key'; return; }
      if (isOwnExtensionViewingKey(recipient)) { statusEl.textContent = 'Private transfers to your own viewing key are not allowed'; return; }
    }

    // Balance guard
    try {
      if (type === 'shield') {
        const balResult = await rpc().call('getBalance', [wallet.address]);
        const spendable = baseUnitBigIntExt(balResult?.spendable || balResult?.balance || 0);
        const feeSpores = 1_000_000n;
        const maxShieldable = spendable > feeSpores ? spendable - feeSpores : 0n;
        if (maxShieldable <= 0n) { statusEl.textContent = 'Insufficient LICN balance to shield'; return; }
        if (amountSpores > maxShieldable) { statusEl.textContent = `Max shieldable: ${formatLicnBaseUnitsFixedExt(maxShieldable)} LICN`; return; }
      } else {
        // unshield/transfer: check shielded balance
        const shieldedBal = extensionShieldedBalanceSpores();
        if (shieldedBal <= 0n) { statusEl.textContent = 'No shielded balance available'; return; }
        if (amountSpores > shieldedBal) { statusEl.textContent = `Max available: ${formatLicnBaseUnitsFixedExt(shieldedBal)} LICN`; return; }
        if (type === 'transfer') {
          const inputNotes = selectTwoExtensionInputNotes(
            unspentExtensionShieldedNotes(),
            amountSpores,
          );
          if (!inputNotes) {
            statusEl.textContent = 'Private transfer requires two unspent shielded notes with enough balance';
            return;
          }
          await assertExtensionPublicFeeBalance('transfer');
        }
      }
    } catch (e) {
      statusEl.textContent = e?.message || 'Balance check failed';
      return;
    }

    statusEl.dataset.busy = 'true';
    statusEl.innerHTML = '<i class="fas fa-spinner fa-spin"></i> Submitting...';
    try {
      if (!wallet) throw new Error('No active wallet');
      const signature = type === 'shield'
        ? await submitExtensionShield({ wallet, amountLicn: amountText, password, statusEl })
        : type === 'transfer'
          ? await submitExtensionPrivateTransfer({ wallet, amountLicn: amountText, password, recipientViewingKey: recipient, statusEl })
          : await submitExtensionUnshield({ wallet, amountLicn: amountText, password, recipient, statusEl });
      statusEl.innerHTML = '<i class="fas fa-check-circle" style="color:#10b981;"></i> Submitted ' + escapeHtmlExt(String(signature).slice(0, 16)) + '...';
      showToast(type === 'shield' ? 'Shield transaction submitted' : type === 'transfer' ? 'Private transfer submitted' : 'Unshield transaction submitted', 'success');
      setTimeout(() => {
        overlay.remove();
        loadShieldTab();
      }, 900);
    } catch (err) {
      delete statusEl.dataset.busy;
      statusEl.innerHTML = '<i class="fas fa-exclamation-circle" style="color:#ef4444;"></i> ' + escapeHtmlExt(err.message);
    }
  });
}

async function loadIdentityTab() {
  const wallet = getActiveWallet();
  const container = $('identityContent');
  if (!wallet || !container) return;

  container.innerHTML = '<div class="empty-state"><i class="fas fa-spinner fa-spin"></i> Loading LichenID...</div>';

  try {
    const data = await loadIdentityDetails(wallet.address, state.network?.selected);

    if (!data) {
      // No identity — show onboarding with Register step
      container.innerHTML = `
        <div class="id-onboard" style="display:flex;flex-direction:column;gap:0.5rem;padding:1rem;">
          <div class="id-onboard-step" id="idRegisterStep" style="display:flex;align-items:center;gap:1rem;padding:1rem;background:var(--bg-card);border:1px solid var(--primary);border-radius:12px;cursor:pointer;transition:background 0.2s;">
            <div style="width:36px;height:36px;border-radius:50%;background:var(--primary);color:#fff;display:flex;align-items:center;justify-content:center;flex-shrink:0;"><i class="fas fa-fingerprint"></i></div>
            <div style="flex:1;">
              <div style="font-weight:600;">Register Your LichenID</div>
              <div style="font-size:0.82rem;color:var(--text-muted);">Create your on-chain identity — choose a display name and agent type. Free — only the 0.0001 LICN tx fee.</div>
            </div>
            <i class="fas fa-chevron-right" style="color:var(--primary);"></i>
          </div>
          <div style="text-align:center;padding:0.5rem 0;">
            <button class="btn btn-small btn-secondary" id="idRefreshBtn" style="font-size:0.78rem;"><i class="fas fa-sync-alt"></i> Refresh</button>
            <div style="font-size:0.72rem;color:var(--text-muted);margin-top:0.35rem;">Already registered? Hit refresh — it may take a block to confirm.</div>
          </div>
        </div>
      `;

      $('idRegisterStep')?.addEventListener('click', () => showIdentityRegisterModal());
      $('idRefreshBtn')?.addEventListener('click', () => loadIdentityTab());
      return;
    }

    // Has identity — render full profile
    const rep = data.reputation;
    const tier = getTrustTier(rep);
    const nextTier = getNextTier(rep);
    const repPct = Math.min(100, (rep / 10000) * 100);
    const agentType = getAgentTypeName(data.agentType);
    const displayName = data.name || 'Unnamed';
    const lichenNameDisplay = formatLichenNameExt(data.lichenName);
    // Avoid "name name.lichen" duplicate when display name matches licn name
    const lichenBase = bareLichenNameExt(data.lichenName);
    const rawDisplayLower = bareLichenNameExt(data.name);
    const showDisplayName = !lichenNameDisplay || rawDisplayLower !== lichenBase;
    const isActive = data.active;
    const skills = data.skills;
    const achievements = data.achievements;
    const vouchesReceived = data.vouchesReceived;
    const vouchesGiven = data.vouchesGiven;
    const achievedIds = new Set(achievements.map(a => Number(a.id)).filter(Boolean));

    const nextInfo = nextTier
      ? `<span style="font-size:0.75rem;color:var(--text-muted);">Next: <strong>${nextTier.name}</strong> at ${nextTier.min.toLocaleString()}</span>`
      : '<span style="font-size:0.75rem;color:var(--text-muted);"><strong>Max tier reached</strong></span>';

    const tierStepsHtml = TRUST_TIERS.map(t => {
      const active = rep >= t.min;
      return `<span style="display:inline-block;padding:0.15rem 0.5rem;border-radius:6px;font-size:0.7rem;${active ? `background:${t.color}18;color:${t.color};border:1px solid ${t.color}33;` : 'background:var(--bg-tertiary);color:var(--text-muted);border:1px solid transparent;'}">${t.name}</span>`;
    }).join(' ');

    const skillsHtml = skills.length > 0
      ? skills.slice(0, 8).map(s => {
        const name = escapeHtmlExt(String(s.name || s.skill || 'Unnamed'));
        const prof = Number(s.proficiency || s.level || 0);
        const level = Math.max(0, Math.min(5, Math.round(prof / 20) || prof));
        const pct = (level / 5) * 100;
        return `<div style="display:flex;align-items:center;gap:0.5rem;margin-bottom:0.35rem;font-size:0.85rem;">
            <span style="min-width:80px;">${name}</span>
            <div style="flex:1;height:4px;background:var(--bg-tertiary);border-radius:2px;overflow:hidden;"><div style="height:100%;width:${pct}%;background:var(--primary);border-radius:2px;"></div></div>
            <span style="color:var(--text-muted);font-size:0.75rem;">${level}/5</span>
          </div>`;
      }).join('')
      : '<div style="color:var(--text-muted);font-size:0.82rem;">No skills yet</div>';

    const vouchChips = vouchesReceived.length > 0
      ? vouchesReceived.slice(0, 12).map(v => {
        const label = escapeHtmlExt(v.voucher_name ? formatLichenNameExt(v.voucher_name) : fmtAddr(v.voucher, 8));
        return `<span style="display:inline-block;padding:0.2rem 0.6rem;background:var(--bg-tertiary);border-radius:6px;font-size:0.75rem;margin:0.15rem;">${label}</span>`;
      }).join('')
      : '<span style="color:var(--text-muted);font-size:0.82rem;">None yet</span>';

    const allAchievements = ACHIEVEMENT_DEFS.map(def => {
      const earned = achievedIds.has(def.id);
      return `<span style="display:inline-block;padding:0.25rem 0.6rem;border-radius:6px;font-size:0.75rem;margin:0.15rem;${earned ? 'background:var(--primary)18;color:var(--primary);border:1px solid var(--primary)33;' : 'background:var(--bg-tertiary);color:var(--text-muted);opacity:0.5;'}"><i class="${escapeHtmlExt(def.icon)}"></i> ${escapeHtmlExt(def.name)}</span>`;
    }).join('');

    container.innerHTML = `
      <!-- Profile Strip -->
      <div style="display:flex;align-items:center;gap:1rem;padding:1.25rem;border-bottom:1px solid var(--border);">
        <div style="width:48px;height:48px;border-radius:50%;background:${tier.color}18;border:2px solid ${tier.color};display:flex;align-items:center;justify-content:center;">
          <i class="fas fa-fingerprint" style="color:${tier.color};font-size:1.25rem;"></i>
        </div>
        <div style="flex:1;">
          <div style="font-weight:700;font-size:1.1rem;">${showDisplayName ? escapeHtmlExt(displayName) : ''}${lichenNameDisplay ? ` <span style="color:var(--primary);">${escapeHtmlExt(lichenNameDisplay)}</span>` : (showDisplayName ? '' : escapeHtmlExt(displayName))}</div>
          <div style="display:flex;align-items:center;gap:0.5rem;flex-wrap:wrap;margin-top:0.25rem;">
            <span style="display:inline-block;padding:0.15rem 0.5rem;border-radius:6px;font-size:0.72rem;background:${tier.color}18;color:${tier.color};border:1px solid ${tier.color}33;">${tier.name}</span>
            <span style="display:inline-block;padding:0.15rem 0.5rem;border-radius:6px;font-size:0.72rem;background:var(--bg-tertiary);">${agentType}</span>
            ${isActive ? '<span style="display:inline-block;padding:0.15rem 0.5rem;border-radius:6px;font-size:0.72rem;background:rgba(74,222,128,0.1);color:#4ade80;"><i class="fas fa-circle" style="font-size:0.35em;vertical-align:middle;"></i> Active</span>' : ''}
            <span style="font-size:0.75rem;color:var(--text-muted);">${rep.toLocaleString()} rep</span>
          </div>
        </div>
        <button class="btn btn-small btn-secondary" id="idEditProfileBtn" title="Edit Profile"><i class="fas fa-pen"></i></button>
      </div>

      <!-- Grid: Reputation + Name -->
      <div style="display:grid;grid-template-columns:1fr 1fr;gap:1rem;padding:1rem;">
        <!-- Reputation -->
        <div style="background:var(--bg-card);border:1px solid var(--border);border-radius:12px;padding:1rem;">
          <div style="font-weight:600;font-size:0.85rem;margin-bottom:0.75rem;"><i class="fas fa-chart-line"></i> Reputation</div>
          <div style="display:flex;align-items:baseline;gap:0.5rem;">
            <span style="font-size:1.5rem;font-weight:700;">${rep.toLocaleString()}</span>
            <span style="color:var(--text-muted);font-size:0.82rem;">/ 10,000</span>
          </div>
          <div style="margin-top:0.5rem;height:6px;background:var(--bg-tertiary);border-radius:3px;overflow:hidden;">
            <div style="height:100%;width:${repPct}%;background:${tier.color};border-radius:3px;"></div>
          </div>
          <div style="margin-top:0.75rem;display:flex;flex-wrap:wrap;gap:0.25rem;">${tierStepsHtml}</div>
          ${nextInfo}
        </div>

        <!-- .lichen Name -->
        <div style="background:var(--bg-card);border:1px solid var(--border);border-radius:12px;padding:1rem;">
          <div style="display:flex;justify-content:space-between;align-items:center;margin-bottom:0.75rem;">
            <span style="font-weight:600;font-size:0.85rem;"><i class="fas fa-at"></i> .lichen Name</span>
          </div>
          ${data.lichenName ? `
            <div style="font-size:1.25rem;font-weight:700;">${escapeHtmlExt(formatLichenNameExt(data.lichenName))}</div>
            <div style="display:flex;gap:0.5rem;margin-top:0.75rem;flex-wrap:wrap;">
              <button class="btn btn-small btn-secondary" id="idRenewNameBtn"><i class="fas fa-redo"></i> Renew</button>
              <button class="btn btn-small btn-secondary" id="idTransferNameBtn"><i class="fas fa-exchange-alt"></i> Transfer</button>
              <button class="btn btn-small btn-danger" id="idReleaseNameBtn" style="font-size:0.75rem;"><i class="fas fa-trash-alt"></i> Release</button>
            </div>
          ` : `
            <div style="color:var(--text-muted);font-size:0.82rem;margin-bottom:0.5rem;">No name registered</div>
            <small style="color:var(--text-muted);">5+ chars from 20 LICN/yr</small>
            <div style="margin-top:0.75rem;text-align:center;">
              <button class="btn btn-small btn-primary" id="idRegisterNameBtn"><i class="fas fa-plus"></i> Register</button>
            </div>
          `}
        </div>

        <!-- Skills -->
        <div style="background:var(--bg-card);border:1px solid var(--border);border-radius:12px;padding:1rem;">
          <div style="display:flex;justify-content:space-between;align-items:center;margin-bottom:0.75rem;">
            <span style="font-weight:600;font-size:0.85rem;"><i class="fas fa-tools"></i> Skills</span>
            <button class="btn btn-small btn-secondary" id="idAddSkillBtn" style="font-size:0.72rem;"><i class="fas fa-plus"></i> Add</button>
          </div>
          ${skillsHtml}
        </div>

        <!-- Vouches -->
        <div style="background:var(--bg-card);border:1px solid var(--border);border-radius:12px;padding:1rem;">
          <div style="display:flex;justify-content:space-between;align-items:center;margin-bottom:0.75rem;">
            <span style="font-weight:600;font-size:0.85rem;"><i class="fas fa-handshake"></i> Vouches</span>
            <button class="btn btn-small btn-secondary" id="idVouchBtn" style="font-size:0.72rem;"><i class="fas fa-plus"></i> Vouch</button>
          </div>
          <div style="display:flex;gap:1rem;margin-bottom:0.5rem;font-size:0.82rem;">
            <span><strong>${vouchesReceived.length}</strong> received</span>
            <span><strong>${vouchesGiven.length}</strong> given</span>
          </div>
          <div style="display:flex;flex-wrap:wrap;">${vouchChips}</div>
        </div>
      </div>

      <!-- Achievements (full width) -->
      <div style="padding:0 1rem 1rem;">
        <div style="background:var(--bg-card);border:1px solid var(--border);border-radius:12px;padding:1rem;">
          <div style="display:flex;justify-content:space-between;align-items:center;margin-bottom:0.75rem;">
            <span style="font-weight:600;font-size:0.85rem;"><i class="fas fa-award"></i> Achievements</span>
            <span style="font-size:0.75rem;color:var(--text-muted);">${achievements.length}/${ACHIEVEMENT_DEFS.length}</span>
          </div>
          <div style="display:flex;flex-wrap:wrap;">${allAchievements}</div>
        </div>
      </div>

      <!-- Agent Service (full width) -->
      <div style="padding:0 1rem 1rem;">
        <div style="background:var(--bg-card);border:1px solid var(--border);border-radius:12px;padding:1rem;">
          <div style="display:flex;justify-content:space-between;align-items:center;margin-bottom:0.75rem;">
            <span style="font-weight:600;font-size:0.85rem;"><i class="fas fa-satellite-dish"></i> Agent Service</span>
            <button class="btn btn-small btn-secondary" id="idConfigAgentBtn" style="font-size:0.72rem;"><i class="fas fa-cog"></i> Configure</button>
          </div>
          <div style="display:grid;grid-template-columns:1fr 1fr 1fr;gap:0.75rem;font-size:0.82rem;">
            <div><span style="color:var(--text-muted);display:block;font-size:0.72rem;">Endpoint</span><span style="font-family:monospace;">${escapeHtmlExt(data.endpoint) || '<em style="opacity:0.4;">Not set</em>'}</span></div>
            <div><span style="color:var(--text-muted);display:block;font-size:0.72rem;">Status</span>${data.availability === 'online' ? '<span style="color:#4ade80;">Online</span>' : '<span style="color:var(--text-muted);">Offline</span>'}</div>
            <div><span style="color:var(--text-muted);display:block;font-size:0.72rem;">Rate</span>${data.rate.toLocaleString(undefined, { maximumFractionDigits: 9 })} LICN/req</div>
          </div>
        </div>
      </div>
    `;

    // Wire action buttons
    $('idEditProfileBtn')?.addEventListener('click', () => showIdentityEditProfileModal(data.agentType));
    $('idAddSkillBtn')?.addEventListener('click', () => showIdentityAddSkillModal());
    $('idVouchBtn')?.addEventListener('click', () => showIdentityVouchModal());
    $('idRegisterNameBtn')?.addEventListener('click', () => showIdentityRegisterNameModal());
    $('idRenewNameBtn')?.addEventListener('click', () => showIdentityRenewNameModal(data.lichenName));
    $('idTransferNameBtn')?.addEventListener('click', () => showIdentityTransferNameModal(data.lichenName));
    $('idReleaseNameBtn')?.addEventListener('click', () => showIdentityReleaseNameModal(data.lichenName));
    $('idConfigAgentBtn')?.addEventListener('click', () => showIdentityAgentConfigModal(data));

  } catch (e) {
    container.innerHTML = `<div class="empty-state"><p>Failed to load identity: ${escapeHtmlExt(e.message)}</p></div>`;
  }
}

/* ── Identity Action Modals ── */

function showIdentityPrompt(title, fields, onSubmit, onRender) {
  // Create a simple modal for identity actions
  const overlay = document.createElement('div');
  overlay.className = 'modal show';
  overlay.style.cssText = 'position:fixed;inset:0;background:rgba(0,0,0,0.6);display:flex;align-items:center;justify-content:center;z-index:1000;';

  const card = document.createElement('div');
  card.style.cssText = 'background:var(--bg-secondary);border:1px solid var(--border);border-radius:16px;padding:1.5rem;max-width:420px;width:90%;max-height:85vh;overflow-y:auto;';

  let fieldsHtml = fields.map(f => {
    if (f.type === 'select') {
      const opts = f.options.map(o => `<option value="${o.value}"${o.selected ? ' selected' : ''}>${o.label}</option>`).join('');
      return `<div class="form-group" style="margin-bottom:0.75rem;"><label style="font-size:0.82rem;color:var(--text-muted);display:block;margin-bottom:0.25rem;">${f.label}</label><select id="idModal_${f.id}" class="form-input" style="width:100%;">${opts}</select></div>`;
    }
    if (f.type === 'info') {
      return `<div style="font-size:0.82rem;color:var(--text-muted);margin-bottom:0.75rem;padding:0.5rem;background:var(--bg-tertiary);border-radius:8px;">${f.html}</div>`;
    }
    const isNumber = f.type === 'number';
    const integerAttr = isNumber && f.step !== undefined && Number(f.step) === 1 ? ' data-integer="true"' : '';
    const minAttr = f.min !== undefined ? (isNumber ? ` data-min="${f.min}"` : ` min="${f.min}"`) : '';
    const maxAttr = f.max !== undefined ? (isNumber ? ` data-max="${f.max}"` : ` max="${f.max}"`) : '';
    const stepAttr = f.step !== undefined ? (isNumber ? ` data-step="${f.step}"` : ` step="${f.step}"`) : '';
    const inputKind = isNumber ? ' data-input-kind="number" data-wallet-numeric="true"' : '';
    const addressAttr = /address|recipient|vouchee/i.test(`${f.id} ${f.label || ''}`) ? ' data-address-input="base58"' : '';
    const inputMode = isNumber ? ` inputmode="${integerAttr ? 'numeric' : 'decimal'}"` : '';
    const inputType = isNumber ? 'text' : (f.type || 'text');
    return `<div class="form-group" style="margin-bottom:0.75rem;"><label style="font-size:0.82rem;color:var(--text-muted);display:block;margin-bottom:0.25rem;">${f.label}</label><input type="${inputType}" id="idModal_${f.id}" class="form-input" placeholder="${f.placeholder || ''}" value="${f.value || ''}"${minAttr}${maxAttr}${stepAttr}${inputKind}${integerAttr}${addressAttr}${inputMode} style="width:100%;"></div>`;
  }).join('');

  card.innerHTML = `
    <h3 style="margin-bottom:1rem;"><i class="fas fa-fingerprint" style="color:var(--primary);margin-right:0.5rem;"></i>${title}</h3>
    ${fieldsHtml}
    <div style="display:flex;gap:0.75rem;margin-top:1rem;">
      <button class="btn btn-secondary" id="idModalCancel" style="flex:1;">Cancel</button>
      <button class="btn btn-primary" id="idModalConfirm" style="flex:1;">Confirm</button>
    </div>
  `;

  overlay.appendChild(card);
  document.body.appendChild(overlay);
  applyExtensionInputGuards(card);

  // Call onRender callback for dynamic behavior (e.g. cost previews)
  if (typeof onRender === 'function') {
    try { onRender(card); } catch (_) { }
  }

  overlay.addEventListener('click', e => { if (e.target === overlay) { overlay.remove(); } });
  card.querySelector('#idModalCancel').addEventListener('click', () => overlay.remove());
  card.querySelector('#idModalConfirm').addEventListener('click', async () => {
    const values = {};
    fields.forEach(f => {
      if (f.type === 'info') return;
      const el = document.getElementById(`idModal_${f.id}`);
      if (el) values[f.id] = el.value;
    });

    const confirmBtn = card.querySelector('#idModalConfirm');
    confirmBtn.disabled = true;
    confirmBtn.innerHTML = '<i class="fas fa-spinner fa-spin"></i> Processing...';

    try {
      await onSubmit(values);
      overlay.remove();
      showToast('Success!', 'success');
      // Retry loading with delay — tx may need 1-3 blocks to be indexed
      const container = $('identityContent');
      if (container) container.innerHTML = '<div class="empty-state"><i class="fas fa-spinner fa-spin"></i> Updating...</div>';
      for (let attempt = 0; attempt < 6; attempt++) {
        await new Promise(r => setTimeout(r, 1500));
        await loadIdentityTab();
        break;
      }
    } catch (err) {
      showToast(err.message, 'error');
      confirmBtn.disabled = false;
      confirmBtn.innerHTML = 'Confirm';
    }
  });
}

async function showIdentityRegisterModal() {
  const wallet = getActiveWallet();
  if (!wallet) return;

  showIdentityPrompt('Register LichenID', [
    { type: 'info', html: 'Create your on-chain identity. Choose a display name and agent type.<br><small>Free — only the 0.0001 LICN tx fee applies.</small>' },
    { id: 'displayName', label: 'Display Name', type: 'text', placeholder: 'e.g. CryptoBuilder' },
    { id: 'agentType', label: 'Agent Type', type: 'select', options: AGENT_TYPES.map(t => ({ value: t.value, label: `${t.label} — ${t.desc}` })) },
    { id: 'password', label: 'Wallet Password', type: 'password', placeholder: 'Sign transaction' }
  ], async (values) => {
    await registerIdentity({
      wallet, password: values.password, network: state.network?.selected,
      displayName: values.displayName, agentType: values.agentType
    });
  });
}

async function showIdentityEditProfileModal(currentAgentType) {
  const wallet = getActiveWallet();
  if (!wallet) return;

  showIdentityPrompt('Update Agent Type', [
    { id: 'agentType', label: 'Agent Type', type: 'select', options: AGENT_TYPES.map(t => ({ value: t.value, label: `${t.label} — ${t.desc}`, selected: t.value === Number(currentAgentType) })) },
    { id: 'password', label: 'Wallet Password', type: 'password', placeholder: 'Sign transaction' }
  ], async (values) => {
    await updateIdentityAgentType({
      wallet, password: values.password, network: state.network?.selected,
      agentType: values.agentType
    });
  });
}

async function showIdentityAddSkillModal() {
  const wallet = getActiveWallet();
  if (!wallet) return;

  showIdentityPrompt('Add Skill', [
    { id: 'skillName', label: 'Skill Name', type: 'text', placeholder: 'e.g. Rust, Trading, Security' },
    { id: 'proficiency', label: 'Proficiency (1-100)', type: 'number', placeholder: '50', min: 1, max: 100, step: 1 },
    { id: 'password', label: 'Wallet Password', type: 'password', placeholder: 'Sign transaction' }
  ], async (values) => {
    await addIdentitySkill({
      wallet, password: values.password, network: state.network?.selected,
      skillName: values.skillName, proficiency: values.proficiency
    });
  });
}

async function showIdentityVouchModal() {
  const wallet = getActiveWallet();
  if (!wallet) return;

  showIdentityPrompt('Vouch for Identity', [
    { type: 'info', html: 'Vouch for another LichenID holder. Both parties must have registered identities.' },
    { id: 'vouchee', label: 'Address to Vouch For', type: 'text', placeholder: 'Base58 address' },
    { id: 'password', label: 'Wallet Password', type: 'password', placeholder: 'Sign transaction' }
  ], async (values) => {
    await vouchForIdentity({
      wallet, password: values.password, network: state.network?.selected,
      vouchee: values.vouchee
    });
  });
}

async function showIdentityRegisterNameModal() {
  const wallet = getActiveWallet();
  if (!wallet) return;

  showIdentityPrompt('Register .lichen Name', [
    { type: 'info', html: '<div style="display:flex;flex-direction:column;gap:0.25rem;"><div><strong>5+ chars</strong> — 20 LICN/year</div><div style="opacity:0.6;"><strong>4 chars</strong> — 100 LICN/year (auction only)</div><div style="opacity:0.6;"><strong>3 chars</strong> — 500 LICN/year (auction only)</div></div><small>Names: lowercase, 5-32 chars (a-z, 0-9, hyphens). Duration: 1-10 years.</small><div id="extNameCostPreview" style="margin-top:0.5rem;padding:0.4rem 0.6rem;background:var(--bg-card);border-radius:6px;font-size:0.82rem;display:none;"><span style="opacity:0.7;">Total cost:</span> <strong id="extNameCostValue">—</strong></div>' },
    { id: 'name', label: 'Name (without .lichen)', type: 'text', placeholder: 'myname (5+ characters)' },
    { id: 'duration', label: 'Duration (years)', type: 'number', placeholder: '1', value: '1', min: 1, max: 10, step: 1 },
    { id: 'password', label: 'Wallet Password', type: 'password', placeholder: 'Sign transaction' }
  ], async (values) => {
    await registerLichenName({
      wallet, password: values.password, network: state.network?.selected,
      name: values.name, durationYears: values.duration
    });
  }, (card) => {
    const nameInput = card.querySelector('#idModal_name');
    const durationInput = card.querySelector('#idModal_duration');
    const preview = card.querySelector('#extNameCostPreview');
    const costValue = card.querySelector('#extNameCostValue');
    // Enforce lowercase as user types
    if (nameInput) {
      nameInput.style.textTransform = 'lowercase';
      nameInput.addEventListener('input', () => {
        const pos = nameInput.selectionStart;
        nameInput.value = nameInput.value.toLowerCase();
        nameInput.setSelectionRange(pos, pos);
      });
    }
    const updateCost = () => {
      const n = bareLichenNameExt(nameInput?.value || '');
      let d = 1;
      try {
        d = parseExtensionIntegerRange(durationInput?.value, 'Duration', 1, 10, 1);
      } catch (_) { }
      if (n.length >= 5) {
        const costPerYear = n.length <= 3 ? 500 : n.length === 4 ? 100 : 20;
        const total = costPerYear * d;
        if (costValue) costValue.textContent = `${total} LICN (${costPerYear} LICN × ${d} yr)`;
        if (preview) preview.style.display = 'block';
      } else {
        if (preview) preview.style.display = 'none';
      }
    };
    if (nameInput) nameInput.addEventListener('input', updateCost);
    if (durationInput) durationInput.addEventListener('input', updateCost);
  });
}

async function showIdentityRenewNameModal(currentName) {
  const wallet = getActiveWallet();
  if (!wallet) return;
  const name = bareLichenNameExt(currentName);

  showIdentityPrompt(`Renew ${name}.lichen`, [
    { id: 'years', label: 'Additional Years', type: 'number', placeholder: '1', value: '1', min: 1, max: 10, step: 1 },
    { id: 'password', label: 'Wallet Password', type: 'password', placeholder: 'Sign transaction' }
  ], async (values) => {
    await renewLichenName({
      wallet, password: values.password, network: state.network?.selected,
      name, additionalYears: values.years
    });
  });
}

async function showIdentityTransferNameModal(currentName) {
  const wallet = getActiveWallet();
  if (!wallet) return;
  const name = bareLichenNameExt(currentName);

  showIdentityPrompt(`Transfer ${name}.lichen`, [
    { type: 'info', html: 'Transfer ownership to another address. <strong>This is irreversible.</strong>' },
    { id: 'recipient', label: 'Recipient Address', type: 'text', placeholder: 'Base58 address' },
    { id: 'password', label: 'Wallet Password', type: 'password', placeholder: 'Sign transaction' }
  ], async (values) => {
    await transferLichenName({
      wallet, password: values.password, network: state.network?.selected,
      name, recipient: values.recipient
    });
  });
}

async function showIdentityReleaseNameModal(currentName) {
  const wallet = getActiveWallet();
  if (!wallet) return;
  const name = bareLichenNameExt(currentName);

  if (!confirm(`Release ${name}.lichen? This is permanent and cannot be undone.`)) return;

  showIdentityPrompt(`Confirm Release: ${name}.lichen`, [
    { type: 'info', html: `You are about to permanently release <strong>${name}.lichen</strong>. It can be re-registered by anyone.` },
    { id: 'password', label: 'Wallet Password', type: 'password', placeholder: 'Sign transaction' }
  ], async (values) => {
    await releaseLichenName({
      wallet, password: values.password, network: state.network?.selected,
      name
    });
  });
}

async function showIdentityAgentConfigModal(data) {
  const wallet = getActiveWallet();
  if (!wallet) return;

  showIdentityPrompt('Agent Service Configuration', [
    { type: 'info', html: 'Configure how other agents discover and interact with your identity.' },
    { id: 'endpoint', label: 'Service Endpoint URL', type: 'text', placeholder: 'https://api.example.com/agent', value: data.endpoint || '' },
    { id: 'rate', label: 'Rate (LICN per request)', type: 'number', placeholder: '0.001', value: String(data.rate || 0) },
    {
      id: 'availability', label: 'Availability', type: 'select', options: [
        { value: 'online', label: 'Online', selected: data.availability === 'online' },
        { value: 'offline', label: 'Offline', selected: data.availability !== 'online' }
      ]
    },
    { id: 'password', label: 'Wallet Password', type: 'password', placeholder: 'Sign transaction' }
  ], async (values) => {
    const tasks = [];
    if (values.endpoint !== (data.endpoint || '')) {
      tasks.push(() => setIdentityEndpoint({ wallet, password: values.password, network: state.network?.selected, endpoint: values.endpoint }));
    }
    const newRateSpores = parseDecimalBaseUnits(values.rate || '0', 9, 'Rate');
    const oldRateSpores = parseDecimalBaseUnits(String(data.rate || 0), 9, 'Rate');
    if (newRateSpores !== oldRateSpores) {
      tasks.push(() => setIdentityRate({ wallet, password: values.password, network: state.network?.selected, rateLicn: values.rate }));
    }
    const newOnline = values.availability === 'online';
    const oldOnline = data.availability === 'online';
    if (newOnline !== oldOnline) {
      tasks.push(() => setIdentityAvailability({ wallet, password: values.password, network: state.network?.selected, online: newOnline }));
    }
    if (tasks.length === 0) throw new Error('No changes to save');
    for (const task of tasks) await task();
  });
}

async function loadAssets() {
  const wallet = getActiveWallet();
  const list = $('assetsList');
  if (!wallet || !list) return;

  list.innerHTML = '<div class="empty-state"><span class="spinner"></span></div>';

  try {
    const [result, neoGasRewards] = await Promise.all([
      rpc().getBalance(wallet.address),
      loadNeoGasRewardsSnapshot(wallet.address)
    ]);
    const raw = Number(result?.spores || result?.spendable || 0);
    const licn = raw / 1_000_000_000;
    const d = decimals();

    list.innerHTML = `
      <div class="asset-item">
        <div class="asset-icon asset-icon-lichen" style="background:rgba(0, 201, 219,0.12);color:var(--primary);">
          <img src="${escapeHtmlExt(LICN_LOGO_URL)}" alt="LICN" style="width:32px;height:32px;border-radius:50%;object-fit:cover;">
        </div>
        <div class="asset-info">
          <div class="asset-name">LICN</div>
          <div class="asset-symbol">Lichen Native Token</div>
          <div class="extension-asset-restriction-badges" data-asset-restriction-badges="LICN"></div>
        </div>
        <div class="asset-balance">
          <div class="asset-amount">${licn.toLocaleString(undefined, { minimumFractionDigits: 2, maximumFractionDigits: 9 })}</div>
          <div class="asset-value">$${(licn * 0.10).toLocaleString(undefined, { minimumFractionDigits: 2, maximumFractionDigits: 6 })}</div>
        </div>
      </div>
      ${renderNeoGasRewardsAsset(neoGasRewards)}
    `;
    renderExtensionAssetRestrictionBadges();
  } catch {
    list.innerHTML = '<div class="empty-state"><p>Failed to load assets</p></div>';
  }
}

let _activityBeforeSlot = null;
let _activityHasMore = true;
const ACTIVITY_PER_PAGE = 20;

function getActivityCursor(result, txs, previousCursor) {
  const rpcNextBeforeSlot = Number(result?.next_before_slot);
  if (Number.isFinite(rpcNextBeforeSlot) && rpcNextBeforeSlot > 0) return rpcNextBeforeSlot;
  const last = txs[txs.length - 1] || {};
  const lastSlot = Number(last.slot || last.block_height || last.block);
  if (Number.isFinite(lastSlot) && lastSlot > 0 && lastSlot !== previousCursor) return lastSlot;
  return null;
}

async function loadActivity(reset = true) {
  const wallet = getActiveWallet();
  const list = $('activityList');
  if (!wallet || !list) return;

  if (reset) {
    _activityBeforeSlot = null;
    _activityHasMore = true;
    list.innerHTML = '<div class="empty-state"><span class="spinner"></span></div>';
  } else {
    const prevBtn = list.querySelector('.activity-load-more');
    if (prevBtn) prevBtn.remove();
  }

  try {
    const requestBeforeSlot = _activityBeforeSlot;
    const opts = { limit: ACTIVITY_PER_PAGE };
    if (requestBeforeSlot) opts.before_slot = requestBeforeSlot;
    const result = await rpc().getTransactionsByAddress(wallet.address, opts);
    const txs = result?.transactions || (Array.isArray(result) ? result : []);
    const rpcHasMore = typeof result?.has_more === 'boolean' ? result.has_more : txs.length >= ACTIVITY_PER_PAGE;
    if (rpcHasMore) {
      const nextCursor = getActivityCursor(result, txs, requestBeforeSlot);
      _activityHasMore = !!nextCursor;
      _activityBeforeSlot = nextCursor;
    } else {
      _activityHasMore = false;
      _activityBeforeSlot = null;
    }

    if (!txs.length && reset) {
      list.innerHTML = '<div class="empty-state"><span class="empty-icon"><i class="fas fa-history"></i></span><p>No recent activity</p></div>';
      return;
    }
    if (!txs.length) return;

    const explorerBase = '../explorer/transaction.html?sig=';
    const html = txs.map(tx => {
      const sig = tx.signature || tx.hash || 'unknown';
      const shortSig = `${String(sig).slice(0, 8)}…${String(sig).slice(-4)}`;
      const isSend = (tx.from === wallet.address);

      // 14 type mappings — aligned with wallet website
      const typeMap = {
        'Transfer': isSend ? 'Sent' : 'Received',
        'Airdrop': 'Airdrop',
        'Stake': 'Staked',
        'Unstake': 'Unstaked',
        'ClaimUnstake': 'Claimed Unstake',
        'RegisterEvmAddress': 'EVM Registration',
        'Contract': 'Contract Call',
        'ContractCall': 'Contract Call',
        'CreateCollection': 'Created Collection',
        'MintNFT': 'Minted NFT',
        'TransferNFT': isSend ? 'Sent NFT' : 'Received NFT',
        'Reward': 'Reward',
        'GenesisTransfer': 'Genesis Transfer',
        'GenesisMint': 'Genesis Mint',
        'MossStakeDeposit': 'Staked (Liquid Staking)',
        'MossStakeUnstake': 'Unstake Requested',
        'MossStakeClaim': 'Claimed Unstake',
        'MossStakeTransfer': 'stLICN Transfer',
        'DeployContract': 'Deploy Contract',
        'SetContractABI': 'Set Contract ABI',
        'FaucetAirdrop': 'Faucet Airdrop',
        'RegisterSymbol': 'Register Symbol',
        'CreateAccount': 'Create Account',
        'GrantRepay': 'Grant Repay',
        'Shield': 'Shielded',
        'Unshield': 'Unshielded',
        'ShieldedTransfer': 'Private Transfer',
      };
      const type = typeMap[tx.type] || (isSend ? 'Sent' : 'Received');

      // Icons & colors — aligned with wallet website
      let icon = isSend ? 'fa-arrow-up' : 'fa-arrow-down';
      let color = isSend ? '#00C9DB' : '#4ade80';
      let sign = isSend ? '-' : '+';

      if (tx.type === 'Shield') {
        icon = 'fa-shield-alt'; color = '#a78bfa'; sign = '-';
      } else if (tx.type === 'Unshield') {
        icon = 'fa-unlock'; color = '#4ade80'; sign = '+';
      } else if (tx.type === 'ShieldedTransfer') {
        icon = 'fa-user-shield'; color = '#a78bfa'; sign = '';
      } else if (tx.type === 'Stake' || tx.type === 'Unstake' || tx.type === 'ClaimUnstake' || tx.type === 'MossStakeDeposit' || tx.type === 'MossStakeUnstake' || tx.type === 'MossStakeClaim' || tx.type === 'MossStakeTransfer') {
        icon = 'fa-coins'; color = '#a78bfa';
        if (tx.type === 'MossStakeDeposit' || tx.type === 'MossStakeUnstake' || tx.type === 'Stake') {
          sign = '-';
        } else if (tx.type === 'MossStakeClaim') {
          sign = '+';
        }
      } else if (tx.type === 'RegisterEvmAddress' || tx.type === 'RegisterSymbol' || tx.type === 'SetContractABI') {
        icon = 'fa-link'; color = '#94a3b8';
      } else if (tx.type === 'Contract' || tx.type === 'ContractCall' || tx.type === 'DeployContract') {
        icon = 'fa-file-code'; color = '#f59e0b';
      } else if (tx.type === 'Reward' || tx.type === 'GenesisTransfer' || tx.type === 'GenesisMint' || tx.type === 'GrantRepay') {
        icon = 'fa-gift'; color = '#4ade80'; sign = '+';
      } else if (tx.type === 'Airdrop' || tx.type === 'FaucetAirdrop') {
        icon = 'fa-parachute-box'; color = '#60a5fa'; sign = '+';
      } else if (tx.type === 'CreateAccount') {
        icon = 'fa-user-plus'; color = '#94a3b8';
      }

      const isMossStakePoolTx = tx.type === 'MossStakeDeposit'
        || tx.type === 'MossStakeUnstake'
        || tx.type === 'MossStakeClaim';
      const address = (tx.type === 'Shield' || tx.type === 'Unshield' || tx.type === 'ShieldedTransfer')
        ? 'Shielded Pool'
        : isMossStakePoolTx
          ? 'MossStake Pool'
        : (isSend ? (tx.to || '') : (tx.from || ''));
      const displayAddr = address && address.length > 20 ? address.slice(0, 8) + '…' + address.slice(-4) : (address || '');
      const amountVal = tx.amount_spores ? tx.amount_spores : (tx.amount || 0);
      const amt = (Number(amountVal) / 1_000_000_000).toLocaleString(undefined, { maximumFractionDigits: 4 });
      const ts = tx.timestamp ? new Date(tx.timestamp * 1000).toLocaleString() : '';
      const explorerLink = sig !== 'unknown' ? `${explorerBase}${sig}` : '#';

      // Fee display: show actual fee amount for 0-amount contract calls / EVM registration
      const isZeroAmount = Number(amountVal) === 0;
      const isFeeOnly = tx.type === 'RegisterEvmAddress'
        || ((tx.type === 'Contract' || tx.type === 'ContractCall') && isZeroAmount);
      const feeSpores = tx.fee_spores || tx.fee || 0;
      const feeAmt = (Number(feeSpores) / 1_000_000_000).toLocaleString(undefined, { maximumFractionDigits: 4 });
      const amountUnit = tx.type === 'MossStakeUnstake' || tx.type === 'MossStakeTransfer'
        ? 'stLICN'
        : 'LICN';
      const amountStr = isFeeOnly ? `${feeAmt} LICN` : `${sign}${amt} ${amountUnit}`;
      const feeTag = isFeeOnly ? '<span style="display:inline-block;margin-left:0.3rem;padding:0.05rem 0.35rem;border-radius:4px;font-size:0.6rem;background:rgba(245,158,11,0.15);color:#f59e0b;font-weight:600;vertical-align:middle;">FEE</span>' : '';

      return `
        <a href="${explorerLink}" target="_blank" class="activity-item" style="text-decoration:none;color:inherit;display:flex;">
          <div class="activity-icon" style="background:${color}22;color:${color};">
            <i class="fas ${icon}"></i>
          </div>
          <div class="activity-details" style="flex:1;min-width:0;">
            <div class="activity-type">${type}${displayAddr ? `<span class="activity-addr" style="margin-left:0.5rem;font-size:0.75rem;opacity:0.5;">${displayAddr}</span>` : ''}</div>
            <div class="activity-date" style="font-size:0.75rem;opacity:0.5;">${shortSig}</div>
          </div>
          <div style="text-align:right;flex-shrink:0;">
            <div class="activity-amount" style="font-weight:600;color:${color};">${amountStr}${feeTag}</div>
            <div style="font-size:0.7rem;opacity:0.5;">${ts}</div>
          </div>
        </a>`;
    }).join('');

    if (reset) {
      list.innerHTML = html;
    } else {
      list.insertAdjacentHTML('beforeend', html);
    }

    if (_activityHasMore) {
      const loadMoreDiv = document.createElement('div');
      loadMoreDiv.className = 'activity-load-more';
      loadMoreDiv.style.cssText = 'text-align:center;padding:1rem;';
      const loadMoreBtn = document.createElement('button');
      loadMoreBtn.className = 'btn btn-small btn-secondary';
      loadMoreBtn.style.cssText = 'padding:0.5rem 1.5rem;font-size:0.85rem;';
      loadMoreBtn.textContent = 'Load More';
      loadMoreBtn.addEventListener('click', () => loadActivity(false));
      loadMoreDiv.appendChild(loadMoreBtn);
      list.appendChild(loadMoreDiv);
    }
  } catch {
    if (reset) list.innerHTML = '<div class="empty-state"><p>Failed to load activity</p></div>';
  }
}

/* ──────────────────────────────────────────
   Send Modal
   ────────────────────────────────────────── */
function openModal(id) { $(id)?.classList.add('show'); }
function closeModal(id) {
  $(id)?.classList.remove('show');
  if (id === 'sendModal') {
    const to = $('sendTo'); if (to) to.value = '';
    const amt = $('sendAmount'); if (amt) amt.value = '';
    const pw = $('sendPassword'); if (pw) pw.value = '';
  }
}

async function handleSend() {
  const wallet = getActiveWallet();
  if (!wallet) return;

  const to = $('sendTo').value.trim();
  const amountInput = $('sendAmount');
  const amountText = amountInput.value.trim();
  const pw = $('sendPassword').value;
  const selectedToken = $('sendToken')?.value || 'LICN';
  let amountSpores;

  if (!isValidAddress(to)) { showToast('Invalid recipient address', 'error'); return; }
  if (to === wallet.address) { showToast('Sending to your own wallet is not allowed', 'error'); return; }
  try {
    amountSpores = parseLicnAmountSporesExt(amountText, 'Transfer amount');
  } catch (error) {
    showToast(error?.message || 'Enter a valid amount', 'error');
    return;
  }
  if (!pw) { showToast('Password required to sign', 'error'); return; }
  if (selectedToken !== 'LICN') { showToast('Extension send supports LICN transfers only', 'error'); return; }

  try {
    const balResult = await rpc().getBalance(wallet.address);
    const spendable = baseUnitBigIntExt(balResult?.spendable || balResult?.spores || 0);
    const feeSpores = 1_000_000n;
    const maxSendable = spendable > feeSpores ? spendable - feeSpores : 0n;
    if (maxSendable <= 0n) {
      showToast('Insufficient LICN balance (not enough to cover fee)', 'error');
      return;
    }
    if (amountSpores > maxSendable) {
      const adjusted = formatLicnBaseUnitsExactExt(maxSendable);
      amountInput.value = adjusted;
      showToast(`Amount adjusted to available balance: ${adjusted} LICN`, 'error');
      return;
    }

    const restrictionPreflight = await preflightNativeTransferRestrictions({
      fromAddress: wallet.address,
      toAddress: to,
      amountLicn: amountText,
      network: activeNetworkKey()
    });
    renderSendRestrictionStatus(restrictionPreflight);
    assertRestrictionPreflightAllowed(restrictionPreflight);

    const privKey = await decryptPrivateKey(wallet.encryptedKey, pw);
    const blockhash = await rpc().getRecentBlockhash();

    const tx = await buildSignedNativeTransferTransaction({
      privateKeyHex: privKey,
      fromAddress: wallet.address,
      toAddress: to,
      amountLicn: amountText,
      blockhash
    });

    const encoded = encodeTransactionBase64(tx);
    await rpc().sendTransactionWithPreflight(encoded);

    showToast('Transaction sent!', 'success');
    closeModal('sendModal');
    $('sendTo').value = '';
    $('sendAmount').value = '';
    $('sendPassword').value = '';
    renderSendRestrictionStatus(null);
    await refreshBalance();
    await loadActivity();
  } catch (e) {
    showToast(`Send failed: ${e.message}`, 'error');
  }
}

/* ──────────────────────────────────────────
   Export functions
   ────────────────────────────────────────── */
async function promptPassword(label) {
  return new Promise(resolve => {
    const pw = prompt(label || 'Enter your wallet password:');
    resolve(pw);
  });
}

async function handleExportPrivKey() {
  const wallet = getActiveWallet();
  if (!wallet) return;
  const pw = await promptPassword('Enter wallet password to export private key:');
  if (!pw) return;
  try {
    const key = await decryptPrivateKey(wallet.encryptedKey, pw);
    await navigator.clipboard.writeText(key);
    showToast('Private key copied to clipboard', 'success');
  } catch (e) { showToast(`Export failed: ${e.message}`, 'error'); }
}

async function handleExportJson() {
  const wallet = getActiveWallet();
  if (!wallet) return;
  const pw = await promptPassword('Enter wallet password to export JSON:');
  if (!pw) return;
  try {
    const privHex = await decryptPrivateKey(wallet.encryptedKey, pw);
    const encryptedSeed = await encryptPrivateKey(privHex, pw);

    const keystore = {
      version: '3.0',
      name: wallet.name,
      address: wallet.address,
      keyType: 'ML-DSA-65',
      publicKey: {
        scheme_version: 1,
        bytes: wallet.publicKey,
      },
      encryptedSeed,
      created: wallet.createdAt,
      exported: new Date().toISOString(),
      encryption: 'AES-256-GCM-PBKDF2'
    };

    const blob = new Blob([JSON.stringify(keystore, null, 2)], { type: 'application/json' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = `lichen-wallet-keystore-${wallet.name}-${Date.now()}.json`;
    a.click();
    URL.revokeObjectURL(url);
    showToast('Keystore JSON downloaded', 'success');
  } catch (e) { showToast(`Export failed: ${e.message}`, 'error'); }
}

async function handleExportSeed() {
  const wallet = getActiveWallet();
  if (!wallet || !wallet.encryptedMnemonic) { showToast('No seed phrase stored', 'error'); return; }
  const pw = await promptPassword('Enter wallet password to view seed phrase:');
  if (!pw) return;
  try {
    const mnemonic = await decryptPrivateKey(wallet.encryptedMnemonic, pw);
    alert(`Your seed phrase:\n\n${mnemonic}\n\nKeep this safe and secret!`);
  } catch (e) { showToast(`Export failed: ${e.message}`, 'error'); }
}

/* ──────────────────────────────────────────
   Receive Modal — tab switching & addresses
   ────────────────────────────────────────── */
function openReceiveModal(initialTab = 'receive') {
  const wallet = getActiveWallet();
  if (wallet) {
    const addrEl = $('walletAddress');
    if (addrEl) addrEl.value = wallet.address;
    const evmEl = $('walletAddressEVM');
    if (evmEl) evmEl.value = wallet.evmAddress || generateEVMAddress(wallet.address) || '';
  }
  switchReceiveTab(initialTab);
  openModal('receiveModal');
}

function switchReceiveTab(tab) {
  document.querySelectorAll('.receive-tab').forEach(t => t.classList.toggle('active', t.dataset.tab === tab));
  const receiveContent = $('receiveTabContent');
  const depositContent = $('depositTabContent');
  if (receiveContent) receiveContent.style.display = (tab === 'receive') ? 'block' : 'none';
  if (depositContent) depositContent.style.display = (tab === 'deposit') ? 'block' : 'none';
  if (receiveContent) receiveContent.classList.toggle('active', tab === 'receive');
  if (depositContent) depositContent.classList.toggle('active', tab === 'deposit');
}

/* ──────────────────────────────────────────
   Bridge Deposit — routed through RPC proxy
   ────────────────────────────────────────── */
const BRIDGE_CHAINS_EXT = {
  solana: { label: 'Solana', assets: ['sol', 'usdc', 'usdt'] },
  ethereum: { label: 'Ethereum', assets: ['eth', 'usdc', 'usdt'] },
  bsc: { label: 'BNB Chain', assets: ['bnb', 'usdc', 'usdt'] },
  neox: { label: 'Neo X', detail: 'Chain ID 47763 · GAS and whole-lot NEO deposits.', assets: ['gas', 'neo'] },
  bitcoin: { label: 'Bitcoin', detail: 'Native SegWit BTC deposits.', assets: ['btc'] }
};
let extDepositPollTimer = null;
let extActiveDepositId = null;
const EXT_DEPOSIT_MAX_POLL = 24 * 60 * 60 * 1000; // 24h
const EXT_DEPOSIT_MAX_ERRORS = 20;
let extDepositTimeout = null;

function clearExtDepositPolling() {
  if (extDepositPollTimer) { clearInterval(extDepositPollTimer); extDepositPollTimer = null; }
  if (extDepositTimeout) { clearTimeout(extDepositTimeout); extDepositTimeout = null; }
}

function escapeHtmlExt(str) {
  if (!str) return '';
  return String(str).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;').replace(/'/g, '&#x27;');
}

function renderExtQrCode(container, text, size = 180) {
  if (!container) return;
  container.innerHTML = '';
  const QR = window.QRCode || globalThis.QRCode;
  if (!QR) {
    container.innerHTML = `
      <div style="padding:1rem;border:1px dashed var(--border);border-radius:12px;text-align:center;color:var(--text-muted);">
        <div style="font-size:1.4rem;margin-bottom:0.35rem;"><i class="fas fa-qrcode"></i></div>
        <div style="font-size:0.82rem;">QR unavailable</div>
      </div>`;
    return;
  }
  try {
    new QR(container, {
      text,
      width: size,
      height: size,
      colorDark: '#1a1a2e',
      colorLight: '#ffffff',
      correctLevel: QR.CorrectLevel.M,
    });
  } catch {
    container.innerHTML = `
      <div style="padding:1rem;border:1px dashed var(--border);border-radius:12px;text-align:center;color:var(--text-muted);">
        <div style="font-size:1.4rem;"><i class="fas fa-qrcode"></i></div>
      </div>`;
  }
}

async function startExtensionDeposit(chain) {
  const wallet = getActiveWallet();
  if (!wallet) { showToast('No active wallet', 'error'); return; }
  if (!isValidAddress(wallet.address)) { showToast('Invalid wallet address', 'error'); return; }

  const chainLabels = { solana: 'Solana', ethereum: 'Ethereum', bsc: 'BNB Chain', neox: 'Neo X', bitcoin: 'Bitcoin' };
  const chainLabel = chainLabels[chain] || chain;
  const chainAssets = (BRIDGE_CHAINS_EXT[chain] || { assets: ['usdc', 'usdt'] }).assets;
  const chainDetail = BRIDGE_CHAINS_EXT[chain]?.detail || '';

  // Show asset picker inline in depositTabContent
  const container = $('depositTabContent');
  if (!container) return;

  const tokenButtons = chainAssets.map(a =>
    `<button class="btn btn-secondary" data-bridge-asset="${a}" style="margin:0.25rem;padding:0.5rem 1.25rem;">${a.toUpperCase()}</button>`
  ).join(' ');

    container.innerHTML = `
    <p style="text-align:center;color:var(--text-secondary);margin-bottom:0.75rem;font-size:0.95rem;">
      Select a token to deposit from <strong>${escapeHtmlExt(chainLabel)}</strong>:
    </p>
    ${chainDetail ? `<p style="text-align:center;color:var(--text-muted);margin-top:-0.45rem;margin-bottom:0.75rem;font-size:0.82rem;">${escapeHtmlExt(chainDetail)}</p>` : ''}
    <p style="text-align:center;color:var(--text-muted);margin-top:-0.25rem;margin-bottom:0.75rem;font-size:0.82rem;">
      Only send the selected asset on this chain. Unfunded addresses are reserved for 24 hours.
    </p>
    <div style="display:flex;gap:0.5rem;justify-content:center;margin-bottom:1rem;">${tokenButtons}</div>
    <div id="extDepositResult" style="display:none;"></div>
    <button class="btn btn-secondary btn-small" id="extDepositBack" style="margin-top:0.75rem;">← Back</button>
  `;

  // Back button restores original deposit tab
  container.querySelector('#extDepositBack')?.addEventListener('click', () => restoreDepositTab(container));

  // Asset buttons
  container.querySelectorAll('[data-bridge-asset]').forEach(btn => {
    btn.addEventListener('click', () => executeExtensionDeposit(chain, btn.dataset.bridgeAsset, chainLabel, container));
  });
}

async function executeExtensionDeposit(chain, asset, chainLabel, container) {
  const wallet = getActiveWallet();
  if (!wallet) return;
  const network = state?.network?.selected || DEFAULT_NETWORK;

  const resultEl = container.querySelector('#extDepositResult');
  if (resultEl) { resultEl.style.display = 'block'; resultEl.innerHTML = '<p style="text-align:center;"><i class="fas fa-spinner fa-spin"></i> Requesting deposit address...</p>'; }

  // Hide asset buttons
  container.querySelectorAll('[data-bridge-asset]').forEach(b => b.style.display = 'none');

  try {
    await preflightBridgeDepositRoute({ chain, asset, network });

    const hasCachedBridgeAuth = hasBridgeAccessAuth(wallet, { chain, asset });
    if (hasCachedBridgeAuth && resultEl) {
      resultEl.innerHTML = '<p style="text-align:center;"><i class="fas fa-key"></i> Using active bridge authorization...</p>';
    }

    let password = '';
    if (!hasCachedBridgeAuth) {
      password = await promptPassword('Enter wallet password to sign bridge access authorization:');
      if (!password) {
        if (resultEl) resultEl.innerHTML = '<p style="color:#EF476F;text-align:center;">Bridge authorization cancelled.</p>';
        container.querySelectorAll('[data-bridge-asset]').forEach(b => b.style.display = '');
        return;
      }
    }

    const response = await requestBridgeDepositAddress({
      wallet,
      password,
      chain,
      asset,
      network
    });

    extActiveDepositId = response.deposit_id;
    const safeAddr = escapeHtmlExt(response.address);
    const safeId = escapeHtmlExt(response.deposit_id);
    const safeAsset = escapeHtmlExt(asset.toUpperCase());

    if (resultEl) {
      resultEl.innerHTML = `
        <div style="background:rgba(0, 201, 219,0.06);border-radius:8px;padding:1rem;text-align:left;">
          <div style="margin-bottom:0.75rem;text-align:center;"><strong>Send ${safeAsset} on ${escapeHtmlExt(chainLabel)}</strong></div>
          <div class="qr-code" id="extDepositQrCode"></div>
          <div class="address-display">
            <input id="extDepositAddr" class="form-input" readonly value="${safeAddr}" style="font-family:'JetBrains Mono',monospace;font-size:0.82rem;">
            <button class="btn-circle" id="extCopyDepositAddr" title="Copy deposit address"><i class="fas fa-copy"></i></button>
          </div>
          <div id="extCopyHint" style="text-align:center;font-size:0.8rem;color:var(--text-muted);margin-bottom:0.5rem;">Copy or scan this address</div>
          <div style="font-size:0.8rem;color:var(--text-muted);margin-bottom:0.5rem;">Deposit ID: ${safeId}</div>
          <div style="font-size:0.8rem;color:var(--text-muted);margin-bottom:0.5rem;">Reserved for 24 hours while unfunded. If it expires, request a new address.</div>
          <div id="extDepositStatus" style="font-size:0.85rem;"><i class="fas fa-clock" style="color:var(--text-muted);"></i> Waiting for deposit...</div>
        </div>
      `;
      renderExtQrCode(container.querySelector('#extDepositQrCode'), response.address, 180);
      const copyAddress = () => {
        navigator.clipboard.writeText(response.address)
          .then(() => {
            const hint = container.querySelector('#extCopyHint');
            if (hint) {
              hint.textContent = 'Copied!';
              setTimeout(() => { hint.textContent = 'Copy or scan this address'; }, 1500);
            }
            showToast('Deposit address copied!', 'success');
          })
          .catch(() => showToast('Copy failed', 'error'));
      };
      container.querySelector('#extDepositAddr')?.addEventListener('click', copyAddress);
      container.querySelector('#extCopyDepositAddr')?.addEventListener('click', copyAddress);
    }

    // Start polling
    clearExtDepositPolling();
    let consecutiveErrors = 0;
    const pollInterval = 5000;

    extDepositTimeout = setTimeout(() => {
      clearExtDepositPolling();
      const statusEl = container.querySelector('#extDepositStatus');
      if (statusEl) statusEl.innerHTML = '<i class="fas fa-times-circle" style="color:#EF476F;"></i> Polling timed out. Check deposit status manually.';
    }, EXT_DEPOSIT_MAX_POLL);

    extDepositPollTimer = setInterval(async () => {
      if (!extActiveDepositId) return;
      try {
        const statusResult = await getBridgeDepositStatus({
          depositId: extActiveDepositId,
          wallet,
          network
        });
        const statusValue = String(statusResult.status || 'issued').toLowerCase();
        const statusEl = container.querySelector('#extDepositStatus');
        const statusMap = {
          issued: '<i class="fas fa-clock" style="color:var(--text-muted);"></i> Waiting for deposit...',
          pending: '<i class="fas fa-spinner fa-spin" style="color:#FFD166;"></i> Deposit detected, confirming...',
          confirmed: '<i class="fas fa-check-circle" style="color:#06D6A0;"></i> Confirmed! Sweeping to treasury...',
          swept: '<i class="fas fa-exchange-alt" style="color:#06D6A0;"></i> Swept! Minting wrapped tokens...',
          credited: '<i class="fas fa-check-double" style="color:#06D6A0;"></i> Credited to your wallet!',
          expired: '<i class="fas fa-times-circle" style="color:#EF476F;"></i> Deposit expired.'
        };
        if (statusEl) statusEl.innerHTML = statusMap[statusValue] || statusMap['issued'];
        consecutiveErrors = 0;
        if (statusValue === 'credited' || statusValue === 'expired') {
          clearExtDepositPolling();
          if (statusValue === 'credited') showToast('Bridge deposit credited!', 'success');
        }
      } catch (error) {
        if (String(error?.message || '').includes('Bridge authorization expired')) {
          clearExtDepositPolling();
          const statusEl = container.querySelector('#extDepositStatus');
          if (statusEl) statusEl.innerHTML = '<i class="fas fa-lock" style="color:#EF476F;"></i> Bridge authorization expired. Restart the bridge flow.';
          return;
        }
        consecutiveErrors++;
        if (consecutiveErrors >= EXT_DEPOSIT_MAX_ERRORS) clearExtDepositPolling();
      }
    }, pollInterval);

  } catch (error) {
    if (resultEl) resultEl.innerHTML = `<p style="color:#EF476F;text-align:center;">Bridge request failed: ${escapeHtmlExt(error?.message || error)}</p>`;
    container.querySelectorAll('[data-bridge-asset]').forEach(b => b.style.display = '');
  }
}

function restoreDepositTab(container) {
  clearExtDepositPolling();
  extActiveDepositId = null;
  container.innerHTML = `
    <p style="text-align:center;color:var(--text-secondary);margin-bottom:1.25rem;font-size:0.95rem;">Deposit assets to your Lichen wallet via bridge</p>
    <div class="deposit-options">
      <div class="deposit-card" id="depositSOL">
        <div class="deposit-card-icon" style="background:rgba(153,69,255,0.12);color:#9945FF;"><i class="fas fa-sun"></i></div>
        <div class="deposit-card-info"><strong>Bridge from Solana</strong><span>SOL, USDC, USDT</span></div>
        <i class="fas fa-chevron-right" style="color:var(--text-muted);"></i>
      </div>
      <div class="deposit-card" id="depositETH">
        <div class="deposit-card-icon" style="background:rgba(98,126,234,0.12);color:#627EEA;"><i class="fab fa-ethereum"></i></div>
        <div class="deposit-card-info"><strong>Bridge from Ethereum</strong><span>ETH, USDC, USDT</span></div>
        <i class="fas fa-chevron-right" style="color:var(--text-muted);"></i>
      </div>
      <div class="deposit-card" id="depositBNB">
        <div class="deposit-card-icon" style="background:rgba(243,186,47,0.12);color:#F3BA2F;"><i class="fas fa-coins"></i></div>
        <div class="deposit-card-info"><strong>Bridge from BNB Chain</strong><span>BNB, USDC, USDT</span></div>
        <i class="fas fa-chevron-right" style="color:var(--text-muted);"></i>
      </div>
      <div class="deposit-card" id="depositNEOX">
        <div class="deposit-card-icon" style="background:rgba(0,229,153,0.12);color:#00E599;"><i class="fas fa-cubes"></i></div>
        <div class="deposit-card-info"><strong>Bridge from Neo X</strong><span>GAS · NEO</span></div>
        <i class="fas fa-chevron-right" style="color:var(--text-muted);"></i>
      </div>
      <div class="deposit-card" id="depositBTC">
        <div class="deposit-card-icon" style="background:rgba(247,147,26,0.12);color:#F7931A;"><i class="fab fa-bitcoin"></i></div>
        <div class="deposit-card-info"><strong>Bridge from Bitcoin</strong><span>BTC</span></div>
        <i class="fas fa-chevron-right" style="color:var(--text-muted);"></i>
      </div>
      <div class="deposit-card disabled">
        <div class="deposit-card-icon" style="background:rgba(0, 201, 219,0.12);color:var(--primary);"><i class="fas fa-credit-card"></i></div>
        <div class="deposit-card-info"><strong>Buy with Fiat</strong><span>Coming with mainnet launch</span></div>
        <span class="label-badge">Soon</span>
      </div>
    </div>
    <div style="text-align:center;margin-top:1.5rem;padding:0.75rem;background:rgba(0, 201, 219,0.08);border-radius:8px;font-size:0.85rem;color:var(--text-secondary);">
      <i class="fas fa-shield-alt" style="color:var(--primary);"></i> Bridge contracts are audited. Deposits typically confirm in 2-5 minutes.
    </div>
  `;
  // Re-wire click handlers
  container.querySelector('#depositSOL')?.addEventListener('click', () => startExtensionDeposit('solana'));
  container.querySelector('#depositETH')?.addEventListener('click', () => startExtensionDeposit('ethereum'));
  container.querySelector('#depositBNB')?.addEventListener('click', () => startExtensionDeposit('bsc'));
  container.querySelector('#depositNEOX')?.addEventListener('click', () => startExtensionDeposit('neox'));
  container.querySelector('#depositBTC')?.addEventListener('click', () => startExtensionDeposit('bitcoin'));
}

/* ──────────────────────────────────────────
   Send — available balance display
   ────────────────────────────────────────── */
async function populateSendTokenDropdown() {
  const select = $('sendToken');
  if (!select) return;
  const wallet = getActiveWallet();
  if (!wallet) return;
  const seen = new Set(['LICN']);
  const createOption = (value, label = value) => {
    const option = document.createElement('option');
    option.value = value;
    option.textContent = label;
    return option;
  };
  select.replaceChildren(createOption('LICN'));
  try {
    const accountsResult = await rpc().call('getTokenAccounts', [wallet.address]);
    const accounts = Array.isArray(accountsResult)
      ? accountsResult
      : Array.isArray(accountsResult?.accounts)
        ? accountsResult.accounts
        : [];
    if (Array.isArray(accounts)) {
      for (const acct of accounts) {
        const sym = String(acct.symbol || acct.token_symbol || '').trim();
        const bal = Number(acct.balance || acct.amount || 0);
        if (/^[A-Za-z0-9._-]{1,24}$/.test(sym) && bal > 0 && !seen.has(sym)) {
          seen.add(sym);
          select.appendChild(createOption(sym));
        }
      }
    }
  } catch { /* fallback: only LICN */ }
  // Add stLICN if user has a staking position
  try {
    const pos = await rpc().call('getStakingPosition', [wallet.address]);
    if (pos && pos.st_licn_amount > 0 && !seen.has('stLICN')) {
      seen.add('stLICN');
      select.appendChild(createOption('stLICN'));
    }
  } catch { /* no staking position */ }
}

async function updateSendAvailableBalance() {
  const el = $('sendAvailableBalance');
  if (!el) return;
  const wallet = getActiveWallet();
  if (!wallet) { el.textContent = ''; return; }
  try {
    const result = await rpc().getBalance(wallet.address);
    const raw = Number(result?.spendable || result?.spores || 0) / 1_000_000_000;
    el.textContent = `Available: ${raw.toLocaleString(undefined, { maximumFractionDigits: decimals() })} LICN`;
  } catch { el.textContent = ''; }
}

/* ──────────────────────────────────────────
   Settings — additional handlers
   ────────────────────────────────────────── */
async function handleAutoLockChange() {
  const mins = Number($('autoLockTimer')?.value || 15);
  const ms = mins * 60 * 1000;
  await persist({ ...state, settings: { ...state.settings, lockTimeout: ms } });
  if (ms > 0) scheduleAutoLock(ms); else clearAutoLockAlarm();
  showToast(`Auto-lock set to ${mins ? mins + ' minutes' : 'never'}`, 'success');
}

async function handleCurrencyChange() {
  const val = $('currencyDisplay')?.value || 'USD';
  await persist({ ...state, settings: { ...state.settings, currency: val } });
  showToast(`Currency set to ${val}`, 'success');
}

async function handleDecimalsChange() {
  const val = Number($('decimalPlaces')?.value || 6);
  await persist({ ...state, settings: { ...state.settings, decimals: val } });
  showToast(`Displaying ${val} decimals`, 'success');
  await refreshBalance();
  await loadAssets();
}

async function handleChangePassword() {
  const wallet = getActiveWallet();
  if (!wallet) return;
  const oldPw = prompt('Enter your current password:');
  if (!oldPw) return;
  try {
    const privKey = await decryptPrivateKey(wallet.encryptedKey, oldPw);
    const newPw = prompt('Enter new password (min 8 chars):');
    if (!newPw || newPw.length < 8) { showToast('New password must be 8+ characters', 'error'); return; }
    const newPw2 = prompt('Confirm new password:');
    if (newPw !== newPw2) { showToast('Passwords do not match', 'error'); return; }

    const newEncKey = await encryptPrivateKey(privKey, newPw);
    let newEncMnemonic = wallet.encryptedMnemonic;
    if (newEncMnemonic) {
      const mnemonic = await decryptPrivateKey(wallet.encryptedMnemonic, oldPw);
      newEncMnemonic = await encryptPrivateKey(mnemonic, newPw);
    }

    const updatedWallets = state.wallets.map(w =>
      w.id === wallet.id ? { ...w, encryptedKey: newEncKey, encryptedMnemonic: newEncMnemonic } : w
    );
    await persist({ ...state, wallets: updatedWallets });
    showToast('Password changed successfully!', 'success');
  } catch (e) { showToast(`Failed: ${e.message}`, 'error'); }
}

async function handleRenameWallet() {
  const wallet = getActiveWallet();
  if (!wallet) return;
  const newName = prompt('Enter new wallet name:', wallet.name);
  if (!newName || newName.trim() === wallet.name) return;
  const updatedWallets = state.wallets.map(w =>
    w.id === wallet.id ? { ...w, name: newName.trim() } : w
  );
  await persist({ ...state, wallets: updatedWallets });
  showToast('Wallet renamed', 'success');
  showDashboard();
}

async function handleClearHistory() {
  if (!confirm('Clear all cached transaction history?')) return;
  showToast('Transaction history cleared', 'success');
  const list = $('activityList');
  if (list) list.innerHTML = '<div class="empty-state"><span class="empty-icon"><i class="fas fa-history"></i></span><p>No recent activity</p></div>';
}

async function handleDeleteWallet() {
  const wallet = getActiveWallet();
  if (!wallet) return;
  if (!confirm(`Delete "${wallet.name}"? This cannot be undone. Make sure you have your recovery phrase!`)) return;
  const remaining = state.wallets.filter(w => w.id !== wallet.id);
  const nextActive = remaining.length > 0 ? remaining[0].id : null;
  await persist({ ...state, wallets: remaining, activeWalletId: nextActive });
  if (remaining.length === 0) {
    showScreen('welcomeScreen');
  } else {
    showDashboard();
  }
  showToast('Wallet deleted', 'success');
}

/* ──────────────────────────────────────────
   Settings
   ────────────────────────────────────────── */
async function handleSaveNetwork() {
  const ns = $('networkSelect');
  if (!ns) return;
  const mainnetRPC = $('mainnetRPC')?.value?.trim() || '';
  const testnetRPC = $('testnetRPC')?.value?.trim() || '';
  await persist({
    ...state,
    network: { ...state.network, selected: ns.value },
    settings: { ...state.settings, mainnetRPC, testnetRPC }
  });
  closeModal('settingsModal');
  showToast(`Network transport saved for ${ns.value}. Bridge and contract metadata stay pinned to trusted endpoints.`, 'success');
  await refreshBalance();
  await loadAssets();
}

/* ──────────────────────────────────────────
   Wire all events
   ────────────────────────────────────────── */
function wireEvents() {
  // Welcome
  $('btnCreateWallet')?.addEventListener('click', () => { showScreen('createWalletScreen'); setWizardStep(1); });
  $('btnImportWallet')?.addEventListener('click', () => { showScreen('importWalletScreen'); });

  // Create flow
  $('createStep2Btn')?.addEventListener('click', handleCreateStep2);
  $('createStep3Btn')?.addEventListener('click', handleCreateStep3);
  $('finishCreateBtn')?.addEventListener('click', handleFinishCreate);
  $('copySeedBtn')?.addEventListener('click', () => {
    navigator.clipboard.writeText(createdMnemonic);
    showToast('Seed phrase copied!', 'success');
  });
  $('backFromCreate')?.addEventListener('click', e => {
    e.preventDefault();
    const step = document.querySelector('.create-step.active');
    const current = step ? Number(step.dataset.step) : 1;
    if (current > 1) { setWizardStep(current - 1); } else { showScreen('welcomeScreen'); }
  });

  // Import flow
  setupImportTabs();
  $('importSeedBtn')?.addEventListener('click', handleImportSeed);
  $('importPrivBtn')?.addEventListener('click', handleImportPrivKey);
  $('importJsonBtn')?.addEventListener('click', handleImportJson);
  $('chooseFileBtn')?.addEventListener('click', () => $('importJsonFile')?.click());
  $('importJsonFile')?.addEventListener('change', () => {
    const f = $('importJsonFile').files?.[0];
    $('fileName').textContent = f ? f.name : '';
  });
  $('backFromImport')?.addEventListener('click', e => { e.preventDefault(); showScreen('welcomeScreen'); });

  // Unlock / Lock / Logout
  $('unlockSubmit')?.addEventListener('click', handleUnlock);
  $('unlockPassword')?.addEventListener('keydown', e => { if (e.key === 'Enter') handleUnlock(); });
  $('logoutBtn')?.addEventListener('click', handleLogout);
  $('navLockBtn')?.addEventListener('click', handleLock);
  $('navLogoutBtn')?.addEventListener('click', handleLogout);

  // Dashboard
  $('refreshBalanceBtn')?.addEventListener('click', async () => { await refreshBalance(); await loadAssets(); });
  $('refreshNftsBtn')?.addEventListener('click', async () => {
    await loadNftsTab();
    showToast('NFTs refreshed', 'success');
  });
  $('browseMarketplaceBtn')?.addEventListener('click', (e) => {
    e.preventDefault();
    chrome.tabs.create({ url: NFT_MARKETPLACE_URL });
  });

  // Send modal
  $('showSendBtn')?.addEventListener('click', async () => {
    openModal('sendModal');
    await populateSendTokenDropdown();
    updateSendAvailableBalance();
    renderSendRestrictionStatus();
    void refreshExtensionRestrictionStatus({ updateSend: true });
  });
  $('closeSendModal')?.addEventListener('click', () => closeModal('sendModal'));
  $('cancelSendBtn')?.addEventListener('click', () => closeModal('sendModal'));
  $('confirmSendBtn')?.addEventListener('click', handleSend);
  $('sendMaxBtn')?.addEventListener('click', async () => {
    const wallet = getActiveWallet();
    if (!wallet) return;
    try {
      const result = await rpc().getBalance(wallet.address);
      const spendable = baseUnitBigIntExt(result?.spendable || result?.spores || 0);
      const feeSpores = 1_000_000n;
      const maxSend = spendable > feeSpores ? spendable - feeSpores : 0n;
      $('sendAmount').value = maxSend > 0n ? formatLicnBaseUnitsExactExt(maxSend) : '';
    } catch { /* ignore */ }
  });

  // Receive modal
  $('showReceiveBtn')?.addEventListener('click', () => { openReceiveModal('receive'); });
  $('showDepositBtn')?.addEventListener('click', () => { openReceiveModal('deposit'); });
  $('closeReceiveModal')?.addEventListener('click', () => closeModal('receiveModal'));
  $('receiveTabBtn')?.addEventListener('click', () => switchReceiveTab('receive'));
  $('depositTabBtn')?.addEventListener('click', () => switchReceiveTab('deposit'));
  $('copyNativeAddr')?.addEventListener('click', async () => {
    const wallet = getActiveWallet();
    if (wallet) { await navigator.clipboard.writeText(wallet.address); showToast('Address copied!', 'success'); }
  });
  $('copyEvmAddr')?.addEventListener('click', async () => {
    const addr = $('walletAddressEVM')?.value;
    if (addr) { await navigator.clipboard.writeText(addr); showToast('EVM address copied!', 'success'); }
  });
  $('depositSOL')?.addEventListener('click', () => startExtensionDeposit('solana'));
  $('depositETH')?.addEventListener('click', () => startExtensionDeposit('ethereum'));
  $('depositBNB')?.addEventListener('click', () => startExtensionDeposit('bsc'));
  $('depositNEOX')?.addEventListener('click', () => startExtensionDeposit('neox'));
  $('depositBTC')?.addEventListener('click', () => startExtensionDeposit('bitcoin'));

  // Settings modal
  $('navSettingsBtn')?.addEventListener('click', () => { loadSettingsValues(); openModal('settingsModal'); });
  $('closeSettingsModal')?.addEventListener('click', () => closeModal('settingsModal'));
  $('saveNetworkBtn')?.addEventListener('click', handleSaveNetwork);
  $('exportPrivKeyBtn')?.addEventListener('click', handleExportPrivKey);
  $('exportJsonBtn')?.addEventListener('click', handleExportJson);
  $('exportSeedBtn')?.addEventListener('click', handleExportSeed);
  $('changePasswordBtn')?.addEventListener('click', handleChangePassword);
  $('renameWalletBtn')?.addEventListener('click', handleRenameWallet);
  $('clearHistoryBtn')?.addEventListener('click', handleClearHistory);
  $('deleteWalletBtn')?.addEventListener('click', handleDeleteWallet);
  $('autoLockTimer')?.addEventListener('change', handleAutoLockChange);
  $('currencyDisplay')?.addEventListener('change', handleCurrencyChange);
  $('decimalPlaces')?.addEventListener('change', handleDecimalsChange);
  $('sendToken')?.addEventListener('change', updateSendAvailableBalance);

  // Wallet selector toggle
  $('walletSelectorBtn')?.addEventListener('click', () => {
    $('walletSelectorWrap')?.classList.toggle('open');
  });
  document.addEventListener('click', e => {
    const wrap = $('walletSelectorWrap');
    if (wrap && !wrap.contains(e.target)) wrap.classList.remove('open');
  });

  // Close modals on backdrop click
  document.querySelectorAll('.modal').forEach(modal => {
    modal.addEventListener('click', e => { if (e.target === modal) closeModal(modal.id); });
  });

  // Auto-lock on activity
  ['click', 'keydown', 'mousemove'].forEach(evt => {
    document.addEventListener(evt, () => {
      if (!state?.isLocked) scheduleAutoLock(state.settings?.lockTimeout || 300000);
    });
  });
}

function loadSettingsValues() {
  const ns = $('networkSelect');
  if (ns) ns.value = state?.network?.selected || DEFAULT_NETWORK;
  const alt = $('autoLockTimer');
  if (alt) {
    const mins = Math.round((state?.settings?.lockTimeout || 300000) / 60000);
    alt.value = String(mins);
  }
  const cd = $('currencyDisplay');
  if (cd) cd.value = state?.settings?.currency || 'USD';
  const dp = $('decimalPlaces');
  if (dp) dp.value = String(state?.settings?.decimals || 6);
  const mainnetRPC = $('mainnetRPC');
  if (mainnetRPC) mainnetRPC.value = state?.settings?.mainnetRPC || '';
  const testnetRPC = $('testnetRPC');
  if (testnetRPC) testnetRPC.value = state?.settings?.testnetRPC || '';
}

/* ──────────────────────────────────────────
   Boot
   ────────────────────────────────────────── */
async function boot() {
  state = await loadState();
  if (!state.network) state.network = { selected: DEFAULT_NETWORK };

  wireEvents();
  applyExtensionInputGuards();
  initCarousel();

  if (state.wallets.length === 0) {
    showScreen('welcomeScreen');
  } else if (state.isLocked) {
    showScreen('unlockScreen');
  } else {
    await showDashboard();

    // Handle hash-based tab navigation (e.g. full.html#identity)
    const hash = window.location.hash.replace('#', '');
    if (hash) {
      const requestedShieldTransfer = hash === 'shield-transfer';
      const tabName = requestedShieldTransfer ? 'shield' : hash;
      const tabBtn = document.querySelector(`.dashboard-tab[data-tab="${tabName}"]`);
      if (tabBtn) {
        tabBtn.click();
        if (requestedShieldTransfer) {
          setTimeout(() => showShieldModal('transfer'), 350);
        }
      }
    }
  }

  if (!state.isLocked) {
    await scheduleAutoLock(state.settings?.lockTimeout || 300000);
  }
}

boot();
