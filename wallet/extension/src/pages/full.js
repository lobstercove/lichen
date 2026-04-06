/* full.js — Full-page wallet view for the LichenWallet extension.
   Replicates the website wallet UI using the extension's core modules. */

import { loadState, saveState } from '../core/state-store.js';
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
  generateEVMAddress
} from '../core/crypto-service.js';
import { buildSignedNativeTransferTransaction, encodeTransactionBase64, registerEvmAddress } from '../core/tx-service.js';
import { notify } from '../core/notification-service.js';
import { requestBridgeDepositAddress, getBridgeDepositStatus } from '../core/bridge-service.js';
import { hasBridgeAccessAuth } from '../core/bridge-service.js';
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

const NFT_MARKETPLACE_URL = 'https://marketplace.lichen.network';

/* ──────────────────────────────────────────
   State
   ────────────────────────────────────────── */
let state = null;
let createdMnemonic = '';
let createdKeypair = null;
let _licnUsdPriceCache = { value: 0.10, ts: 0 };

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

function rpc() {
  const network = state?.network?.selected || 'local-testnet';
  const endpoint = getRpcEndpoint(network, state?.settings || {});
  return new LichenRPC(endpoint);
}

function sporesToLicn(value) {
  const raw = Number(value);
  return Number.isFinite(raw) ? raw / 1_000_000_000 : 0;
}

function getFullBalanceSnapshot(result) {
  return {
    totalLicn: sporesToLicn(result?.spores ?? result?.balance ?? result?.total ?? result?.spendable ?? 0),
    spendableLicn: sporesToLicn(result?.spendable ?? result?.available ?? result?.spores ?? result?.balance ?? 0),
    stakedLicn: sporesToLicn(result?.staked ?? result?.staked_spores ?? 0),
    pendingRewardsLicn: sporesToLicn(result?.pending_rewards ?? result?.pendingRewards ?? 0),
    lockedLicn: sporesToLicn(result?.locked ?? result?.locked_spores ?? 0),
    mossStakedLicn: sporesToLicn(result?.moss_staked ?? result?.mossStaked ?? 0)
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

async function getLiveLicnUsdPrice() {
  const now = Date.now();
  if (now - _licnUsdPriceCache.ts < 60_000 && _licnUsdPriceCache.value > 0) {
    return _licnUsdPriceCache.value;
  }

  const endpoint = getRpcEndpoint(state?.network?.selected || 'local-testnet', state?.settings || {});
  const apiBase = rpcEndpointToApiBase(endpoint);
  if (!apiBase) return _licnUsdPriceCache.value || 0.10;

  try {
    const response = await fetch(`${apiBase}/oracle/prices`);
    if (!response.ok) throw new Error('oracle fetch failed');
    const data = await response.json();
    const feeds = Array.isArray(data?.feeds) ? data.feeds : [];
    const licnFeed = feeds.find((feed) => String(feed?.asset || '').toUpperCase() === 'LICN');
    const price = Number(licnFeed?.price || 0);
    if (Number.isFinite(price) && price > 0) {
      _licnUsdPriceCache = { value: price, ts: now };
      return price;
    }
  } catch {
    // Fall back to cached/default price.
  }

  return _licnUsdPriceCache.value || 0.10;
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
  let key = $('importPrivKey').value.trim().replace(/^0x/, '');
  const pw = $('importPasswordPriv').value;
  if (!key || !/^[0-9a-fA-F]{64}$/.test(key)) { showToast('Private key must be exactly 64 hex characters (0-9, a-f)', 'error'); return; }
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
  state = { wallets: [], activeWalletId: null, isLocked: false, settings: { currency: 'USD', lockTimeout: 300000 }, network: { selected: 'local-testnet' } };
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
  if (ns) ns.value = state.network?.selected || 'local-testnet';

  // Dashboard tabs
  setupDashboardTabs();

  // Load data
  await refreshBalance();
  await loadAssets();
  await loadActivity();
  await loadNftsTab();
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
    const network = state?.network?.selected || 'local-testnet';
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
    const licnUsdPrice = await getLiveLicnUsdPrice();
    $('totalBalance').textContent = `${balanceSnapshot.totalLicn.toLocaleString(undefined, { minimumFractionDigits: 2, maximumFractionDigits: 9 })} LICN`;
    $('balanceUsd').textContent = `$${(balanceSnapshot.totalLicn * licnUsdPrice).toLocaleString(undefined, { minimumFractionDigits: 2, maximumFractionDigits: 6 })} USD`;

    const breakdownEl = $('balanceBreakdown');
    if (breakdownEl) {
      const hasBreakdown = balanceSnapshot.stakedLicn > 0 || balanceSnapshot.lockedLicn > 0 || balanceSnapshot.mossStakedLicn > 0 || balanceSnapshot.pendingRewardsLicn > 0;
      if (hasBreakdown) {
        const parts = [`<i class="fas fa-wallet" style="opacity:0.5;"></i> Spendable: <strong>${balanceSnapshot.spendableLicn.toLocaleString(undefined, { maximumFractionDigits: 4 })}</strong>`];
        if (balanceSnapshot.stakedLicn > 0) parts.push(`<i class="fas fa-lock" style="opacity:0.5;"></i> Staked: <strong>${balanceSnapshot.stakedLicn.toLocaleString(undefined, { maximumFractionDigits: 4 })}</strong>`);
        if (balanceSnapshot.mossStakedLicn > 0) parts.push(`<i class="fas fa-coins" style="opacity:0.5;"></i> Liquid Staking: <strong>${balanceSnapshot.mossStakedLicn.toLocaleString(undefined, { maximumFractionDigits: 4 })}</strong>`);
        if (balanceSnapshot.pendingRewardsLicn > 0) parts.push(`<i class="fas fa-gift" style="opacity:0.5;"></i> Rewards: <strong>${balanceSnapshot.pendingRewardsLicn.toLocaleString(undefined, { maximumFractionDigits: 4 })}</strong>`);
        if (balanceSnapshot.lockedLicn > 0) parts.push(`<i class="fas fa-hourglass" style="opacity:0.5;"></i> Locked: <strong>${balanceSnapshot.lockedLicn.toLocaleString(undefined, { maximumFractionDigits: 4 })}</strong>`);
        breakdownEl.innerHTML = parts.join(' &nbsp;·&nbsp; ');
        breakdownEl.style.display = 'block';
      } else {
        breakdownEl.innerHTML = '';
        breakdownEl.style.display = 'none';
      }
    }
  } catch {
    $('totalBalance').textContent = '0.00 LICN';
    $('balanceUsd').textContent = '$0.00 USD';
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
    const [poolInfo, position, queue] = await Promise.all([
      rpcClient.call('getMossStakePoolInfo').catch(() => null),
      rpcClient.call('getStakingPosition', [wallet.address]).catch(() => null),
      rpcClient.call('getUnstakingQueue', [wallet.address]).catch(() => ({ pending_requests: [] })),
    ]);

    const stLicn = Number(position?.st_licn_amount || 0) / 1e9;
    const value = Number(position?.current_value_licn || 0) / 1e9;
    const rewards = Number(position?.rewards_earned || 0) / 1e9;
    const totalStaked = Number(poolInfo?.total_licn_staked || 0) / 1e9;
    const lockTier = position?.lock_tier_name || 'Flexible';
    const multiplier = position?.reward_multiplier || 1.0;
    const lockUntil = Number(position?.lock_until || 0);

    // Determine if position is locked
    const currentSlot = Math.floor(Date.now() / 400);
    const isLocked = lockUntil > 0 && lockUntil > currentSlot;
    const remainingDays = isLocked ? Math.ceil((lockUntil - currentSlot) / 216000) : 0;

    const tierNames = ['Flexible', '30-Day', '180-Day', '365-Day'];
    const tierMultipliers = ['1.0x', '1.6x', '2.4x', '3.6x'];
    const tierColors = ['#94a3b8', '#60a5fa', '#a78bfa', '#f59e0b'];
    const poolTiers = poolInfo?.tiers || [];

    const lockBanner = isLocked
      ? `<div style="margin-top:1rem;padding:0.75rem 1rem;background:rgba(249,115,22,0.1);border:1px solid rgba(249,115,22,0.3);border-radius:8px;font-size:0.85rem;color:#f97316;">
           <i class="fas fa-lock"></i> Position locked (${lockTier}). ~${remainingDays} days remaining.
         </div>`
      : '';

    container.innerHTML = `
      <div style="background:linear-gradient(135deg,rgba(59,130,246,0.1),rgba(37,99,235,0.1));padding:1.5rem;border-radius:12px;margin-bottom:1.5rem;">
        <h3 style="margin:0 0 0.5rem 0;display:flex;align-items:center;gap:0.5rem;">
          <i class="fas fa-water" style="color:#3b82f6;"></i> Liquid Staking
        </h3>
        <p style="margin:0;font-size:0.85rem;color:var(--text-muted);">
          Stake LICN to receive stLICN. Rewards auto-compound. Choose a lock tier for boosted rewards.
        </p>
      </div>

      <div style="display:grid;grid-template-columns:repeat(3,1fr);gap:1rem;margin-bottom:1.5rem;">
        <div style="background:var(--card-bg);padding:1rem;border-radius:10px;border:1px solid var(--border);text-align:center;">
          <div style="font-size:0.75rem;color:var(--text-muted);margin-bottom:0.25rem;">Your stLICN</div>
          <div style="font-size:1.2rem;font-weight:700;color:var(--text);">${stLicn.toLocaleString(undefined, { maximumFractionDigits: 4 })}</div>
        </div>
        <div style="background:var(--card-bg);padding:1rem;border-radius:10px;border:1px solid var(--border);text-align:center;">
          <div style="font-size:0.75rem;color:var(--text-muted);margin-bottom:0.25rem;">Value</div>
          <div style="font-size:1.2rem;font-weight:700;color:var(--text);">${value.toLocaleString(undefined, { maximumFractionDigits: 4 })} LICN</div>
        </div>
        <div style="background:var(--card-bg);padding:1rem;border-radius:10px;border:1px solid var(--border);text-align:center;">
          <div style="font-size:0.75rem;color:var(--text-muted);margin-bottom:0.25rem;">Rewards Earned</div>
          <div style="font-size:1.2rem;font-weight:700;color:#10b981;">${rewards.toLocaleString(undefined, { maximumFractionDigits: 4 })} LICN</div>
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
        ${tierNames.map((name, i) => {
      const isActive = lockTier === name && stLicn > 0;
      const apyVal = poolTiers[i]?.apy_percent;
      const apyLabel = apyVal != null && apyVal > 0 ? apyVal.toFixed(1) + '% APY' : tierMultipliers[i] + ' rewards';
      return `<div style="background:var(--card-bg);padding:0.75rem;border-radius:8px;border:2px solid ${isActive ? tierColors[i] : 'var(--border)'};text-align:center;">
            <div style="font-size:0.8rem;font-weight:600;color:${tierColors[i]};">${name}</div>
            <div style="font-size:0.72rem;color:var(--text-muted);">${apyLabel}</div>
            ${isActive ? '<div style="font-size:0.65rem;color:#10b981;margin-top:0.25rem;"><i class="fas fa-check-circle"></i> Active</div>' : ''}
          </div>`;
    }).join('')}
      </div>

      <div style="background:var(--card-bg);padding:1rem;border-radius:10px;border:1px solid var(--border);margin-bottom:1rem;font-size:0.85rem;color:var(--text-muted);">
        <i class="fas fa-info-circle" style="color:#3b82f6;"></i>
        <strong>Flexible:</strong> 7-day cooldown, 1x rewards.
        <strong>Locked tiers</strong> earn boosted rewards but funds are locked for the chosen duration.
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
        <h4 style="margin-bottom:1rem;">Pending Unstakes (7-day cooldown)</h4>
        <div id="fullUnstakesList"></div>
      </div>
    `;

    // Pending unstakes
    const pendingReqs = queue?.pending_requests || [];
    if (pendingReqs.length > 0) {
      $('fullPendingUnstakes').style.display = 'block';
      $('fullUnstakesList').innerHTML = pendingReqs.map(req => {
        const amt = (Number(req.licn_to_receive || req.amount || 0) / 1e9).toLocaleString(undefined, { maximumFractionDigits: 4 });
        const cs = Math.floor(Date.now() / 400);
        const claimable = req.claimable_at <= cs;
        return `<div style="padding:0.75rem;background:var(--card-bg);border-radius:8px;border:1px solid var(--border);margin-bottom:0.5rem;display:flex;justify-content:space-between;align-items:center;">
          <span style="font-weight:600;">${amt} LICN</span>
          ${claimable
            ? '<button class="btn btn-small fullClaimBtn" style="padding:0.3rem 0.8rem;font-size:0.8rem;background:#10b981;border:none;border-radius:6px;color:#fff;cursor:pointer;font-weight:600;"><i class="fas fa-check-circle"></i> Claim</button>'
            : `<span style="color:var(--text-muted);font-size:0.8rem;"><i class="fas fa-clock"></i> ~${((req.claimable_at - cs) / 216000).toFixed(1)} days</span>`
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
      <input type="number" id="stakeAmountInput" placeholder="0.00" style="width:100%;padding:0.75rem;border-radius:8px;border:1px solid var(--border);background:var(--card-bg);color:var(--text);margin-bottom:1rem;box-sizing:border-box;">
      <label style="font-size:0.85rem;font-weight:600;display:block;margin-bottom:0.25rem;">Lock Tier</label>
      <select id="stakeTierSelect" style="width:100%;padding:0.75rem;border-radius:8px;border:1px solid var(--border);background:var(--card-bg);color:var(--text);margin-bottom:1rem;box-sizing:border-box;">
        <option value="0">Flexible — 7-day cooldown, 1x rewards</option>
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

  overlay.querySelector('#stakeCancelBtn').addEventListener('click', () => overlay.remove());
  overlay.querySelector('#stakeConfirmBtn').addEventListener('click', async () => {
    let amount = parseFloat(overlay.querySelector('#stakeAmountInput').value);
    const tier = parseInt(overlay.querySelector('#stakeTierSelect').value, 10);
    const password = overlay.querySelector('#stakePasswordInput').value;
    const statusEl = overlay.querySelector('#stakeModalStatus');
    if (!amount || amount <= 0) { statusEl.textContent = 'Enter a valid amount'; return; }
    if (!password) { statusEl.textContent = 'Password required'; return; }
    // Balance guard: check spendable LICN
    try {
      const balResult = await rpc().getBalance(wallet.address);
      const spendable = Number(balResult?.spendable || balResult?.spores || 0) / 1_000_000_000;
      const maxStakable = Math.max(0, spendable - 0.001);
      if (maxStakable <= 0) { statusEl.textContent = 'Insufficient LICN balance'; return; }
      if (amount > maxStakable) {
        amount = parseFloat(maxStakable.toFixed(6));
        overlay.querySelector('#stakeAmountInput').value = amount;
        statusEl.textContent = `Adjusted to available: ${amount} LICN`;
        return;
      }
    } catch (e) { /* let RPC reject */ }
    try {
      statusEl.innerHTML = '<i class="fas fa-spinner fa-spin"></i> Staking...';
      await stakeLicn({ wallet, password, amountLicn: amount, tier, network: state.network?.selected || 'local-testnet' });
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
      <p style="font-size:0.85rem;color:var(--text-muted);margin-bottom:1rem;">After requesting, there is a <strong>7-day cooldown</strong> before you can claim your LICN.</p>
      <label style="font-size:0.85rem;font-weight:600;display:block;margin-bottom:0.25rem;">Amount (stLICN)</label>
      <input type="number" id="unstakeAmountInput" placeholder="0.00" style="width:100%;padding:0.75rem;border-radius:8px;border:1px solid var(--border);background:var(--card-bg);color:var(--text);margin-bottom:1rem;box-sizing:border-box;">
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

  overlay.querySelector('#unstakeCancelBtn').addEventListener('click', () => overlay.remove());
  overlay.querySelector('#unstakeConfirmBtn').addEventListener('click', async () => {
    let amount = parseFloat(overlay.querySelector('#unstakeAmountInput').value);
    const password = overlay.querySelector('#unstakePasswordInput').value;
    const statusEl = overlay.querySelector('#unstakeModalStatus');
    if (!amount || amount <= 0) { statusEl.textContent = 'Enter a valid amount'; return; }
    if (!password) { statusEl.textContent = 'Password required'; return; }
    // Balance guard: check stLICN position
    try {
      const pos = await rpc().call('getStakingPosition', [wallet.address]);
      const stLicn = (pos?.st_licn_amount || 0) / 1_000_000_000;
      if (stLicn <= 0) { statusEl.textContent = 'No stLICN balance to unstake'; return; }
      if (amount > stLicn) {
        amount = parseFloat(stLicn.toFixed(6));
        overlay.querySelector('#unstakeAmountInput').value = amount;
        statusEl.textContent = `Adjusted to stLICN balance: ${amount}`;
        return;
      }
    } catch (e) { /* let RPC reject */ }
    try {
      statusEl.innerHTML = '<i class="fas fa-spinner fa-spin"></i> Unstaking...';
      await unstakeStLicn({ wallet, password, amountLicn: amount, network: state.network?.selected || 'local-testnet' });
      statusEl.innerHTML = '<span style="color:#10b981;">✓ Unstake initiated! 7-day cooldown.</span>';
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
    const currentSlot = Math.floor(Date.now() / 400);
    const claimable = pending.filter(r => r.claimable_at <= currentSlot);
    if (claimable.length === 0) {
      alert('No matured unstakes to claim');
      return;
    }
  } catch (e) { /* let RPC reject */ }
  try {
    const balResult = await rpc().call('getBalance', [wallet.address]);
    const spendable = (balResult?.spendable || balResult?.balance || 0) / 1e9;
    if (spendable < 0.001) {
      alert('Insufficient LICN for transaction fee (need 0.001 LICN)');
      return;
    }
  } catch (e) { /* let RPC reject */ }

  const password = prompt('Enter wallet password to claim unstake:');
  if (!password) return;
  try {
    await claimMossStake({ wallet, password, network: state.network?.selected || 'local-testnet' });
    alert('Claim successful!');
    loadStakingTab();
  } catch (err) {
    alert('Claim failed: ' + err.message);
  }
}

// ──────────────────────────────────────────
// Shield (ZK Privacy) Tab
// ──────────────────────────────────────────
let _shieldedState = { initialized: false, balance: 0, address: null, viewingKey: null, notes: [], poolStats: null };

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

async function ensureShieldedStateInitialized(wallet) {
  if (_shieldedState.initialized && _shieldedState.address && _shieldedState.viewingKey) {
    return true;
  }

  const password = await securePasswordPrompt('Enter your wallet password to initialize shielded privacy.');
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
      viewingKey,
    };
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

  let shieldedBalance = 0;
  let ownedNotes = [];
  let shieldedAddress = _shieldedState.address || 'Initialize shielded wallet to derive';
  try {
    const addr = _shieldedState.address;
    const notesRes = addr ? await rpcClient.call('getShieldedNotes', [addr]) : [];
    if (Array.isArray(notesRes)) {
      ownedNotes = notesRes;
      shieldedBalance = notesRes.filter(n => !n.spent).reduce((s, n) => s + Number(n.value || 0), 0);
    }
  } catch (_) { }

  _shieldedState = { ..._shieldedState, balance: shieldedBalance, address: shieldedAddress, notes: ownedNotes, poolStats };

  const balLicn = (shieldedBalance / 1_000_000_000).toFixed(4);
  const poolLicn = poolStats ? ((poolStats.pool_balance || 0) / 1_000_000_000).toFixed(2) : '—';
  const commitCount = poolStats ? (poolStats.commitment_count || poolStats.commitmentCount || 0).toLocaleString() : '—';
  const unspent = ownedNotes.filter(n => !n.spent);

  const notesHtml = unspent.length > 0
    ? unspent.map(n => `
        <div style="padding:0.75rem;background:var(--card-bg);border-radius:8px;border:1px solid var(--border);margin-bottom:0.5rem;display:flex;justify-content:space-between;align-items:center;">
          <div>
            <div style="font-weight:600;"><i class="fas fa-lock" style="color:#10b981;margin-right:0.25rem;"></i>${(Number(n.value || 0) / 1e9).toFixed(4)} LICN</div>
            <div style="font-size:0.7rem;color:var(--text-muted);">Note #${n.index || '?'} &bull; ${(n.commitment || '').slice(0, 12)}...</div>
          </div>
          <span style="font-size:0.7rem;background:rgba(16,185,129,0.1);color:#10b981;padding:0.2rem 0.5rem;border-radius:4px;"><i class="fas fa-check-circle"></i> Unspent</span>
        </div>`).join('')
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
          <button class="btn btn-small btn-secondary" id="extUnshieldBtn"><i class="fas fa-arrow-up"></i> Unshield</button>
        </div>
      </div>
      <button class="btn btn-primary" id="extPrivateTransferBtn" style="width:100%;padding:0.75rem;">
        <i class="fas fa-paper-plane"></i> Private Transfer
      </button>
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
  $('extPrivateTransferBtn')?.addEventListener('click', () => showShieldModal('transfer'));
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
  const titles = { shield: 'Shield LICN', unshield: 'Unshield LICN', transfer: 'Private Transfer' };
  const icons = { shield: 'fa-arrow-down', unshield: 'fa-arrow-up', transfer: 'fa-paper-plane' };

  const extraField = type === 'unshield'
    ? `<label style="font-size:0.85rem;font-weight:600;display:block;margin-bottom:0.25rem;">Recipient Address</label>
       <input type="text" id="shieldModalRecipient" placeholder="Base58 address" style="width:100%;padding:0.75rem;border-radius:8px;border:1px solid var(--border);background:var(--card-bg);color:var(--text);margin-bottom:1rem;box-sizing:border-box;">`
    : type === 'transfer'
      ? `<label style="font-size:0.85rem;font-weight:600;display:block;margin-bottom:0.25rem;">Recipient Viewing Key</label>
       <input type="text" id="shieldModalRecipient" placeholder="64-char hex viewing key" style="width:100%;padding:0.75rem;border-radius:8px;border:1px solid var(--border);background:var(--card-bg);color:var(--text);margin-bottom:1rem;box-sizing:border-box;">`
      : '';

  const overlay = document.createElement('div');
  overlay.className = 'modal-overlay';
  overlay.style.cssText = 'position:fixed;top:0;left:0;width:100%;height:100%;background:rgba(0,0,0,0.6);display:flex;align-items:center;justify-content:center;z-index:10000;';
  overlay.innerHTML = `
    <div style="background:var(--bg);border:1px solid var(--border);border-radius:16px;padding:2rem;width:420px;max-width:90vw;">
      <h3 style="margin:0 0 1rem;"><i class="fas ${icons[type]}" style="color:#10b981;"></i> ${titles[type]}</h3>
      <label style="font-size:0.85rem;font-weight:600;display:block;margin-bottom:0.25rem;">Amount (LICN)</label>
      <input type="number" id="shieldModalAmount" placeholder="0.00" style="width:100%;padding:0.75rem;border-radius:8px;border:1px solid var(--border);background:var(--card-bg);color:var(--text);margin-bottom:1rem;box-sizing:border-box;">
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

  overlay.querySelector('#shieldModalCancel').addEventListener('click', () => overlay.remove());
  overlay.querySelector('#shieldModalConfirm').addEventListener('click', async () => {
    const amount = parseFloat(overlay.querySelector('#shieldModalAmount').value);
    const password = overlay.querySelector('#shieldModalPassword').value;
    const recipient = overlay.querySelector('#shieldModalRecipient')?.value?.trim() || '';
    const statusEl = overlay.querySelector('#shieldModalStatus');

    if (!amount || amount <= 0) { statusEl.textContent = 'Enter a valid amount'; return; }
    if (!password) { statusEl.textContent = 'Password required'; return; }
    if (type !== 'shield' && !recipient) { statusEl.textContent = 'Recipient required'; return; }

    // Balance guard
    try {
      const wallet = getActiveWallet();
      if (type === 'shield') {
        const balResult = await rpc().call('getBalance', [wallet.address]);
        const spendable = (balResult?.spendable || balResult?.balance || 0) / 1e9;
        const maxShieldable = Math.max(0, spendable - 0.001);
        if (maxShieldable <= 0) { statusEl.textContent = 'Insufficient LICN balance to shield'; return; }
        if (amount > maxShieldable) { statusEl.textContent = `Max shieldable: ${maxShieldable.toFixed(4)} LICN`; return; }
      } else {
        // unshield/transfer: check shielded balance
        const shieldedBal = (_shieldedState.balance || 0) / 1e9;
        if (shieldedBal <= 0) { statusEl.textContent = 'No shielded balance available'; return; }
        if (amount > shieldedBal) { statusEl.textContent = `Max available: ${shieldedBal.toFixed(4)} LICN`; return; }
      }
    } catch (e) { /* let RPC reject */ }

    statusEl.innerHTML = '<i class="fas fa-spinner fa-spin"></i> Submitting...';
    try {
      const wallet = getActiveWallet();
      const net = state.network?.selected || 'local-testnet';
      const txType = type === 'shield' ? 16 : type === 'unshield' ? 17 : 18;
      // Build shielded transaction via RPC
      const result = await rpc().call('sendShieldedTransaction', [{
        type: txType,
        from: wallet.address,
        amount: Math.round(amount * 1e9),
        recipient: recipient || undefined,
        password
      }]);
      statusEl.innerHTML = '<i class="fas fa-check-circle" style="color:#10b981;"></i> ' + (result?.message || 'Transaction submitted');
      setTimeout(() => { overlay.remove(); loadShieldTab(); }, 1500);
    } catch (err) {
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
    const lichenNameDisplay = data.lichenName
      ? (data.lichenName.endsWith('.lichen') ? data.lichenName : data.lichenName + '.lichen')
      : '';
    // Avoid "name name.lichen" duplicate when display name matches licn name
    const lichenBase = data.lichenName ? data.lichenName.replace(/\.licn$/, '').toLowerCase() : '';
    const rawDisplayLower = (data.name || '').toLowerCase().replace(/\.licn$/, '');
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
        const label = escapeHtmlExt(v.voucher_name ? v.voucher_name + '.lichen' : fmtAddr(v.voucher, 8));
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
            <div style="font-size:1.25rem;font-weight:700;">${escapeHtmlExt(data.lichenName.endsWith('.lichen') ? data.lichenName : data.lichenName + '.lichen')}</div>
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
    const minAttr = f.min !== undefined ? ` min="${f.min}"` : '';
    const maxAttr = f.max !== undefined ? ` max="${f.max}"` : '';
    const stepAttr = f.step !== undefined ? ` step="${f.step}"` : '';
    return `<div class="form-group" style="margin-bottom:0.75rem;"><label style="font-size:0.82rem;color:var(--text-muted);display:block;margin-bottom:0.25rem;">${f.label}</label><input type="${f.type || 'text'}" id="idModal_${f.id}" class="form-input" placeholder="${f.placeholder || ''}" value="${f.value || ''}"${minAttr}${maxAttr}${stepAttr} style="width:100%;"></div>`;
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
    { id: 'name', label: 'Name (without .licn)', type: 'text', placeholder: 'myname (5+ characters)' },
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
      const n = (nameInput?.value || '').toLowerCase().replace(/\.licn$/, '').trim();
      const d = Math.max(1, Math.min(10, parseInt(durationInput?.value) || 1));
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
  const name = (currentName || '').replace(/\.licn$/, '');

  showIdentityPrompt(`Renew ${name}.licn`, [
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
  const name = (currentName || '').replace(/\.licn$/, '');

  showIdentityPrompt(`Transfer ${name}.licn`, [
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
  const name = (currentName || '').replace(/\.licn$/, '');

  if (!confirm(`Release ${name}.licn? This is permanent and cannot be undone.`)) return;

  showIdentityPrompt(`Confirm Release: ${name}.licn`, [
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
    if (Number(values.rate || 0) !== data.rate) {
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
    const result = await rpc().getBalance(wallet.address);
    const raw = Number(result?.spores || result?.spendable || 0);
    const licn = raw / 1_000_000_000;
    const d = decimals();

    list.innerHTML = `
      <div class="asset-item">
        <div class="asset-icon" style="background:rgba(0, 201, 219,0.12);color:var(--primary);">🦞</div>
        <div class="asset-info">
          <div class="asset-name">LICN</div>
          <div class="asset-symbol">Lichen Native Token</div>
        </div>
        <div class="asset-balance">
          <div class="asset-amount">${licn.toLocaleString(undefined, { minimumFractionDigits: 2, maximumFractionDigits: 9 })}</div>
          <div class="asset-value">$${(licn * 0.10).toLocaleString(undefined, { minimumFractionDigits: 2, maximumFractionDigits: 6 })}</div>
        </div>
      </div>
    `;
  } catch {
    list.innerHTML = '<div class="empty-state"><p>Failed to load assets</p></div>';
  }
}

let _activityPage = 0;
const ACTIVITY_PER_PAGE = 20;

async function loadActivity(reset = true) {
  const wallet = getActiveWallet();
  const list = $('activityList');
  if (!wallet || !list) return;

  if (reset) {
    _activityPage = 0;
    list.innerHTML = '<div class="empty-state"><span class="spinner"></span></div>';
  }

  try {
    const result = await rpc().getTransactionsByAddress(wallet.address, { limit: ACTIVITY_PER_PAGE, offset: _activityPage * ACTIVITY_PER_PAGE });
    const txs = result?.transactions || (Array.isArray(result) ? result : []);

    if (!txs.length && _activityPage === 0) {
      list.innerHTML = '<div class="empty-state"><span class="empty-icon"><i class="fas fa-history"></i></span><p>No recent activity</p></div>';
      return;
    }

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
        'CreateCollection': 'Created Collection',
        'MintNFT': 'Minted NFT',
        'TransferNFT': isSend ? 'Sent NFT' : 'Received NFT',
        'Reward': 'Reward',
        'GenesisTransfer': 'Genesis Transfer',
        'GenesisMint': 'Genesis Mint',
        'MossStakeDeposit': 'Staked (Liquid Staking)',
        'MossStakeUnstake': 'Unstake Requested',
        'MossStakeClaim': 'Claimed Unstake',
        'MossStakeTransfer': 'MossStake Transfer',
        'DeployContract': 'Deploy Contract',
        'SetContractABI': 'Set Contract ABI',
        'FaucetAirdrop': 'Faucet Airdrop',
        'RegisterSymbol': 'Register Symbol',
        'CreateAccount': 'Create Account',
        'GrantRepay': 'Grant Repay',
      };
      const type = typeMap[tx.type] || (isSend ? 'Sent' : 'Received');

      // Icons & colors — aligned with wallet website
      let icon = isSend ? 'fa-arrow-up' : 'fa-arrow-down';
      let color = isSend ? '#00C9DB' : '#4ade80';
      let sign = isSend ? '-' : '+';

      if (tx.type === 'Stake' || tx.type === 'Unstake' || tx.type === 'ClaimUnstake' || tx.type === 'MossStakeDeposit' || tx.type === 'MossStakeUnstake' || tx.type === 'MossStakeClaim' || tx.type === 'MossStakeTransfer') {
        icon = 'fa-coins'; color = '#a78bfa';
      } else if (tx.type === 'RegisterEvmAddress' || tx.type === 'RegisterSymbol' || tx.type === 'SetContractABI') {
        icon = 'fa-link'; color = '#94a3b8';
      } else if (tx.type === 'Contract' || tx.type === 'DeployContract') {
        icon = 'fa-file-code'; color = '#f59e0b';
      } else if (tx.type === 'Reward' || tx.type === 'GenesisTransfer' || tx.type === 'GenesisMint' || tx.type === 'GrantRepay') {
        icon = 'fa-gift'; color = '#4ade80'; sign = '+';
      } else if (tx.type === 'Airdrop' || tx.type === 'FaucetAirdrop') {
        icon = 'fa-parachute-box'; color = '#60a5fa'; sign = '+';
      } else if (tx.type === 'CreateAccount') {
        icon = 'fa-user-plus'; color = '#94a3b8';
      }

      const address = isSend ? (tx.to || '') : (tx.from || '');
      const displayAddr = address && address.length > 20 ? address.slice(0, 8) + '…' + address.slice(-4) : (address || '');
      const amountVal = tx.amount_spores ? tx.amount_spores : (tx.amount || 0);
      const amt = (Number(amountVal) / 1_000_000_000).toLocaleString(undefined, { maximumFractionDigits: 4 });
      const ts = tx.timestamp ? new Date(tx.timestamp * 1000).toLocaleString() : '';
      const explorerLink = sig !== 'unknown' ? `${explorerBase}${sig}` : '#';

      // Fee display: show actual fee amount for 0-amount contract calls / EVM registration
      const isZeroAmount = Number(amountVal) === 0;
      const isFeeOnly = tx.type === 'RegisterEvmAddress' || (tx.type === 'Contract' && isZeroAmount);
      const feeSpores = tx.fee_spores || tx.fee || 0;
      const feeAmt = (Number(feeSpores) / 1_000_000_000).toLocaleString(undefined, { maximumFractionDigits: 4 });
      const amountStr = isFeeOnly ? `${feeAmt} LICN` : `${sign}${amt} LICN`;
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
      // Remove previous "Load More" button before appending
      const prevBtn = list.querySelector('.activity-load-more');
      if (prevBtn) prevBtn.remove();
      list.insertAdjacentHTML('beforeend', html);
    }

    // Add "Load More" if we got a full page
    if (txs.length >= ACTIVITY_PER_PAGE) {
      _activityPage++;
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
    if (_activityPage === 0) list.innerHTML = '<div class="empty-state"><p>Failed to load activity</p></div>';
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
  const amount = Number($('sendAmount').value || 0);
  const pw = $('sendPassword').value;

  if (!isValidAddress(to)) { showToast('Invalid recipient address', 'error'); return; }
  if (!amount || amount <= 0) { showToast('Enter a valid amount', 'error'); return; }
  if (!pw) { showToast('Password required to sign', 'error'); return; }

  try {
    const balResult = await rpc().getBalance(wallet.address);
    const spendable = Number(balResult?.spendable || balResult?.spores || 0) / 1_000_000_000;
    const maxSendable = Math.max(0, spendable - 0.001);
    if (maxSendable <= 0) {
      showToast('Insufficient LICN balance (not enough to cover fee)', 'error');
      return;
    }
    if (amount > maxSendable) {
      $('sendAmount').value = maxSendable.toFixed(6);
      showToast(`Amount adjusted to available balance: ${maxSendable.toFixed(6)} LICN`, 'error');
      return;
    }

    const privKey = await decryptPrivateKey(wallet.encryptedKey, pw);
    const block = await rpc().getLatestBlock();
    const blockhash = block?.hash || block?.blockhash || 'genesis';

    const tx = await buildSignedNativeTransferTransaction({
      privateKeyHex: privKey,
      fromAddress: wallet.address,
      toAddress: to,
      amountLicn: amount,
      blockhash
    });

    const encoded = encodeTransactionBase64(tx);
    await rpc().sendTransaction(encoded);

    showToast('Transaction sent!', 'success');
    closeModal('sendModal');
    $('sendTo').value = '';
    $('sendAmount').value = '';
    $('sendPassword').value = '';
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
  bsc: { label: 'BNB Chain', assets: ['bnb', 'usdc', 'usdt'] }
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

async function startExtensionDeposit(chain) {
  const wallet = getActiveWallet();
  if (!wallet) { showToast('No active wallet', 'error'); return; }
  if (!isValidAddress(wallet.address)) { showToast('Invalid wallet address', 'error'); return; }

  const chainLabels = { solana: 'Solana', ethereum: 'Ethereum', bsc: 'BNB Chain' };
  const chainLabel = chainLabels[chain] || chain;
  const chainAssets = (BRIDGE_CHAINS_EXT[chain] || { assets: ['usdc', 'usdt'] }).assets;

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
  const network = state?.network?.selected || 'local-testnet';

  const resultEl = container.querySelector('#extDepositResult');
  if (resultEl) { resultEl.style.display = 'block'; resultEl.innerHTML = '<p style="text-align:center;"><i class="fas fa-spinner fa-spin"></i> Requesting deposit address...</p>'; }

  // Hide asset buttons
  container.querySelectorAll('[data-bridge-asset]').forEach(b => b.style.display = 'none');

  try {
    if (hasBridgeAccessAuth(wallet) && resultEl) {
      resultEl.innerHTML = '<p style="text-align:center;"><i class="fas fa-key"></i> Refreshing bridge authorization for a new deposit request...</p>';
    }

    const password = await promptPassword('Enter wallet password to sign bridge access authorization:');
    if (!password) {
      if (resultEl) resultEl.innerHTML = '<p style="color:#EF476F;text-align:center;">Bridge authorization cancelled.</p>';
      container.querySelectorAll('[data-bridge-asset]').forEach(b => b.style.display = '');
      return;
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
          <div style="margin-bottom:0.5rem;"><strong>Send ${safeAsset} on ${escapeHtmlExt(chainLabel)} to:</strong></div>
          <div class="mono" style="word-break:break-all;background:rgba(0,0,0,0.15);padding:0.5rem;border-radius:6px;margin-bottom:0.5rem;cursor:pointer;" id="extDepositAddr">${safeAddr}</div>
          <div style="font-size:0.8rem;color:var(--text-muted);margin-bottom:0.5rem;">Deposit ID: ${safeId}</div>
          <div id="extDepositStatus" style="font-size:0.85rem;"><i class="fas fa-clock" style="color:var(--text-muted);"></i> Waiting for deposit...</div>
        </div>
      `;
      container.querySelector('#extDepositAddr')?.addEventListener('click', () => {
        navigator.clipboard.writeText(response.address)
          .then(() => showToast('Deposit address copied!', 'success'))
          .catch(() => showToast('Copy failed', 'error'));
      });
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
}

/* ──────────────────────────────────────────
   Send — available balance display
   ────────────────────────────────────────── */
async function populateSendTokenDropdown() {
  const select = $('sendToken');
  if (!select) return;
  const wallet = getActiveWallet();
  if (!wallet) return;
  select.innerHTML = '<option value="LICN">LICN</option>';
  try {
    const accounts = await rpc().call('getTokenAccounts', [wallet.address]);
    if (Array.isArray(accounts)) {
      for (const acct of accounts) {
        const sym = acct.symbol || acct.token_symbol || '';
        const bal = Number(acct.balance || acct.amount || 0);
        if (sym && bal > 0) {
          select.innerHTML += `<option value="${sym}">${sym}</option>`;
        }
      }
    }
  } catch { /* fallback: only LICN */ }
  // Add stLICN if user has a staking position
  try {
    const pos = await rpc().call('getStakingPosition', [wallet.address]);
    if (pos && pos.st_licn_amount > 0) {
      select.innerHTML += '<option value="stLICN">stLICN</option>';
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
  });
  $('closeSendModal')?.addEventListener('click', () => closeModal('sendModal'));
  $('cancelSendBtn')?.addEventListener('click', () => closeModal('sendModal'));
  $('confirmSendBtn')?.addEventListener('click', handleSend);
  $('sendMaxBtn')?.addEventListener('click', async () => {
    const wallet = getActiveWallet();
    if (!wallet) return;
    try {
      const result = await rpc().getBalance(wallet.address);
      const spendable = Number(result?.spendable || result?.spores || 0) / 1_000_000_000;
      $('sendAmount').value = Math.max(0, spendable - 0.001).toFixed(6);
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
  if (ns) ns.value = state?.network?.selected || 'local-testnet';
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
  if (!state.network) state.network = { selected: 'local-testnet' };

  wireEvents();
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
      const tabBtn = document.querySelector(`.dashboard-tab[data-tab="${hash}"]`);
      if (tabBtn) tabBtn.click();
    }
  }

  if (!state.isLocked) {
    await scheduleAutoLock(state.settings?.lockTimeout || 300000);
  }
}

boot();
