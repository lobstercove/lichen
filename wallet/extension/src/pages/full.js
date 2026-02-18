/* full.js — Full-page wallet view for the MoltWallet extension.
   Replicates the website wallet UI using the extension's core modules. */

import { loadState, saveState } from '../core/state-store.js';
import { getRpcEndpoint, MoltChainRPC } from '../core/rpc-service.js';
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
  hexToBytes,
  base58Encode,
  generateEVMAddress,
  keccak256
} from '../core/crypto-service.js';
import { buildSignedNativeTransferTransaction, encodeTransactionBase64, registerEvmAddress } from '../core/tx-service.js';
import { notify } from '../core/notification-service.js';
import { requestBridgeDepositAddress, getBridgeDepositStatus } from '../core/bridge-service.js';
import {
  loadIdentityDetails,
  registerIdentity,
  addIdentitySkill,
  updateIdentityAgentType,
  vouchForIdentity,
  setIdentityEndpoint,
  setIdentityAvailability,
  setIdentityRate,
  registerMoltName,
  renewMoltName,
  transferMoltName,
  releaseMoltName
} from '../core/identity-service.js';
import { stakeMolt, unstakeStMolt, claimReefStake, loadStakingSnapshot } from '../core/staking-service.js';

/* ──────────────────────────────────────────
   State
   ────────────────────────────────────────── */
let state = null;
let createdMnemonic = '';
let createdKeypair = null;

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
  return new MoltChainRPC(endpoint);
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
    registerEvmAddress({ wallet, privateKeyHex: createdKeypair.privateKey, network: state.network?.selected, settings: state.settings }).catch(() => {});

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
}

async function handleImportSeed() {
  const seed = $('importSeed').value.trim();
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
    registerEvmAddress({ wallet, privateKeyHex: kp.privateKey, network: state.network?.selected, settings: state.settings }).catch(() => {});
    showToast('Wallet imported!', 'success');
    showDashboard();
  } catch (e) {
    showToast(`Import failed: ${e.message}`, 'error');
  }
}

async function handleImportPrivKey() {
  const key = $('importPrivKey').value.trim().replace(/^0x/, '');
  const pw = $('importPasswordPriv').value;
  if (!key || key.length !== 64) { showToast('Private key must be 64 hex characters', 'error'); return; }
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
    registerEvmAddress({ wallet, privateKeyHex: kp.privateKey, network: state.network?.selected, settings: state.settings }).catch(() => {});
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
    if (json.secretKey && Array.isArray(json.secretKey) && json.secretKey.length === 64) {
      const seed = new Uint8Array(json.secretKey.slice(0, 32));
      kp = await privateKeyToKeypair(bytesToHex(seed));
    } else if (json.privateKey) {
      kp = await privateKeyToKeypair(json.privateKey.replace(/^0x/, ''));
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
    registerEvmAddress({ wallet, privateKeyHex: kp.privateKey, network: state.network?.selected, settings: state.settings }).catch(() => {});
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
      if (name === 'identity') loadIdentityTab();
      if (name === 'staking') loadStakingTab();
    });
  });
}

async function refreshBalance() {
  const wallet = getActiveWallet();
  if (!wallet) return;

  try {
    const result = await rpc().getBalance(wallet.address);
    const raw = Number(result?.shells || result?.spendable || 0);
    const molt = raw / 1_000_000_000;
    const d = decimals();
    $('totalBalance').textContent = `${molt.toLocaleString(undefined, { minimumFractionDigits: 2, maximumFractionDigits: 9 })} MOLT`;
    $('balanceUsd').textContent = `$${(molt * 0.10).toLocaleString(undefined, { minimumFractionDigits: 2, maximumFractionDigits: 6 })} USD`;
  } catch {
    $('totalBalance').textContent = '0.00 MOLT';
    $('balanceUsd').textContent = '$0.00 USD';
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
  { value: 9, label: 'General', desc: 'Multi-purpose or uncategorized agent' }
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
  { id: 1, name: 'First Transaction', icon: 'fas fa-exchange-alt' },
  { id: 2, name: 'Governance Voter', icon: 'fas fa-vote-yea' },
  { id: 3, name: 'Builder', icon: 'fas fa-code' },
  { id: 4, name: 'Trusted', icon: 'fas fa-shield-alt' },
  { id: 5, name: 'Veteran', icon: 'fas fa-medal' },
  { id: 6, name: 'Legend', icon: 'fas fa-crown' },
  { id: 7, name: 'Endorsed', icon: 'fas fa-handshake' },
  { id: 8, name: 'Graduated', icon: 'fas fa-graduation-cap' }
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
      rpcClient.call('getReefStakePoolInfo').catch(() => null),
      rpcClient.call('getStakingPosition', [wallet.address]).catch(() => null),
      rpcClient.call('getUnstakingQueue', [wallet.address]).catch(() => ({ pending_requests: [] })),
    ]);

    const stMolt = Number(position?.st_molt_amount || 0) / 1e9;
    const value = Number(position?.current_value_molt || 0) / 1e9;
    const rewards = Number(position?.rewards_earned || 0) / 1e9;
    const totalStaked = Number(poolInfo?.total_molt_staked || 0) / 1e9;
    const lockTier = position?.lock_tier_name || 'Flexible';
    const multiplier = position?.reward_multiplier || 1.0;
    const lockUntil = Number(position?.lock_until || 0);

    // Determine if position is locked
    const currentSlot = Math.floor(Date.now() / 400);
    const isLocked = lockUntil > 0 && lockUntil > currentSlot;
    const remainingDays = isLocked ? Math.ceil((lockUntil - currentSlot) / 216000) : 0;

    const tierNames = ['Flexible', '30-Day', '90-Day', '365-Day'];
    const tierMultipliers = ['1.0x', '1.5x', '2.0x', '3.0x'];
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
          <i class="fas fa-water" style="color:#3b82f6;"></i> ReefStake — Liquid Staking
        </h3>
        <p style="margin:0;font-size:0.85rem;color:var(--text-muted);">
          Stake MOLT to receive stMOLT. Rewards auto-compound. Choose a lock tier for boosted rewards.
        </p>
      </div>

      <div style="display:grid;grid-template-columns:repeat(3,1fr);gap:1rem;margin-bottom:1.5rem;">
        <div style="background:var(--card-bg);padding:1rem;border-radius:10px;border:1px solid var(--border);text-align:center;">
          <div style="font-size:0.75rem;color:var(--text-muted);margin-bottom:0.25rem;">Your stMOLT</div>
          <div style="font-size:1.2rem;font-weight:700;color:var(--text);">${stMolt.toLocaleString(undefined,{maximumFractionDigits:4})}</div>
        </div>
        <div style="background:var(--card-bg);padding:1rem;border-radius:10px;border:1px solid var(--border);text-align:center;">
          <div style="font-size:0.75rem;color:var(--text-muted);margin-bottom:0.25rem;">Value</div>
          <div style="font-size:1.2rem;font-weight:700;color:var(--text);">${value.toLocaleString(undefined,{maximumFractionDigits:4})} MOLT</div>
        </div>
        <div style="background:var(--card-bg);padding:1rem;border-radius:10px;border:1px solid var(--border);text-align:center;">
          <div style="font-size:0.75rem;color:var(--text-muted);margin-bottom:0.25rem;">Rewards Earned</div>
          <div style="font-size:1.2rem;font-weight:700;color:#10b981;">${rewards.toLocaleString(undefined,{maximumFractionDigits:4})} MOLT</div>
        </div>
      </div>

      <div style="display:grid;grid-template-columns:repeat(2,1fr);gap:0.75rem;margin-bottom:1.5rem;">
        <div style="background:var(--card-bg);padding:0.75rem;border-radius:8px;border:1px solid var(--border);text-align:center;">
          <div style="font-size:0.7rem;color:var(--text-muted);">Your Tier</div>
          <div style="font-weight:600;color:#a78bfa;">${lockTier}</div>
        </div>
        <div style="background:var(--card-bg);padding:0.75rem;border-radius:8px;border:1px solid var(--border);text-align:center;">
          <div style="font-size:0.7rem;color:var(--text-muted);">Total Pool</div>
          <div style="font-weight:600;color:var(--text);">${totalStaked.toLocaleString(undefined,{maximumFractionDigits:0})} MOLT</div>
        </div>
      </div>

      <div style="display:grid;grid-template-columns:repeat(4,1fr);gap:0.75rem;margin-bottom:1.5rem;" id="fullTiersGrid">
        ${tierNames.map((name, i) => {
          const isActive = lockTier === name || (i === 0 && lockTier === 'Flexible');
          const apyVal = poolTiers[i]?.apy_percent;
          const apyLabel = apyVal != null && apyVal > 0 ? apyVal.toFixed(1) + '% APY' : tierMultipliers[i] + ' rewards';
          return `<div style="background:var(--card-bg);padding:0.75rem;border-radius:8px;border:2px solid ${isActive ? tierColors[i] : 'var(--border)'};text-align:center;">
            <div style="font-size:0.8rem;font-weight:600;color:${tierColors[i]};">${name}</div>
            <div style="font-size:0.72rem;color:var(--text-muted);">${apyLabel}</div>
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
          <i class="fas fa-arrow-down"></i> Stake MOLT
        </button>
        <button id="fullUnstakeBtn" class="btn btn-secondary" style="width:100%;padding:1rem;font-size:0.9rem;${isLocked ? 'opacity:0.5;cursor:not-allowed;' : ''}">
          <i class="fas fa-arrow-up"></i> Unstake stMOLT
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
        const amt = (Number(req.molt_to_receive || req.amount || 0) / 1e9).toLocaleString(undefined, { maximumFractionDigits: 4 });
        const cs = Math.floor(Date.now() / 400);
        const claimable = req.claimable_at <= cs;
        return `<div style="padding:0.75rem;background:var(--card-bg);border-radius:8px;border:1px solid var(--border);margin-bottom:0.5rem;display:flex;justify-content:space-between;align-items:center;">
          <span style="font-weight:600;">${amt} MOLT</span>
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
      <h3 style="margin:0 0 1rem;"><i class="fas fa-layer-group" style="color:#3b82f6;"></i> Stake to ReefStake</h3>
      <label style="font-size:0.85rem;font-weight:600;display:block;margin-bottom:0.25rem;">Amount (MOLT)</label>
      <input type="number" id="stakeAmountInput" placeholder="0.00" style="width:100%;padding:0.75rem;border-radius:8px;border:1px solid var(--border);background:var(--card-bg);color:var(--text);margin-bottom:1rem;box-sizing:border-box;">
      <label style="font-size:0.85rem;font-weight:600;display:block;margin-bottom:0.25rem;">Lock Tier</label>
      <select id="stakeTierSelect" style="width:100%;padding:0.75rem;border-radius:8px;border:1px solid var(--border);background:var(--card-bg);color:var(--text);margin-bottom:1rem;box-sizing:border-box;">
        <option value="0">Flexible — 7-day cooldown, 1x rewards</option>
        <option value="1">30-Day Lock — 1.5x rewards</option>
        <option value="2">90-Day Lock — 2x rewards</option>
        <option value="3">365-Day Lock — 3x rewards</option>
      </select>
      <label style="font-size:0.85rem;font-weight:600;display:block;margin-bottom:0.25rem;">Wallet Password</label>
      <input type="password" id="stakePasswordInput" placeholder="Enter password" style="width:100%;padding:0.75rem;border-radius:8px;border:1px solid var(--border);background:var(--card-bg);color:var(--text);margin-bottom:1.25rem;box-sizing:border-box;">
      <div style="display:flex;gap:0.75rem;">
        <button id="stakeConfirmBtn" class="btn btn-primary" style="flex:1;padding:0.75rem;">Stake MOLT</button>
        <button id="stakeCancelBtn" class="btn btn-secondary" style="flex:1;padding:0.75rem;">Cancel</button>
      </div>
      <div id="stakeModalStatus" style="margin-top:0.75rem;font-size:0.85rem;text-align:center;"></div>
    </div>
  `;
  document.body.appendChild(overlay);

  overlay.querySelector('#stakeCancelBtn').addEventListener('click', () => overlay.remove());
  overlay.querySelector('#stakeConfirmBtn').addEventListener('click', async () => {
    const amount = parseFloat(overlay.querySelector('#stakeAmountInput').value);
    const tier = parseInt(overlay.querySelector('#stakeTierSelect').value, 10);
    const password = overlay.querySelector('#stakePasswordInput').value;
    const statusEl = overlay.querySelector('#stakeModalStatus');
    if (!amount || amount <= 0) { statusEl.textContent = 'Enter a valid amount'; return; }
    if (!password) { statusEl.textContent = 'Password required'; return; }
    try {
      statusEl.innerHTML = '<i class="fas fa-spinner fa-spin"></i> Staking...';
      await stakeMolt({ wallet, password, amountMolt: amount, tier, network: state.network?.selected || 'local-testnet' });
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
      <h3 style="margin:0 0 1rem;"><i class="fas fa-unlock-alt" style="color:#f59e0b;"></i> Unstake from ReefStake</h3>
      <p style="font-size:0.85rem;color:var(--text-muted);margin-bottom:1rem;">After requesting, there is a <strong>7-day cooldown</strong> before you can claim your MOLT.</p>
      <label style="font-size:0.85rem;font-weight:600;display:block;margin-bottom:0.25rem;">Amount (stMOLT)</label>
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
    const amount = parseFloat(overlay.querySelector('#unstakeAmountInput').value);
    const password = overlay.querySelector('#unstakePasswordInput').value;
    const statusEl = overlay.querySelector('#unstakeModalStatus');
    if (!amount || amount <= 0) { statusEl.textContent = 'Enter a valid amount'; return; }
    if (!password) { statusEl.textContent = 'Password required'; return; }
    try {
      statusEl.innerHTML = '<i class="fas fa-spinner fa-spin"></i> Unstaking...';
      await unstakeStMolt({ wallet, password, amountMolt: amount, network: state.network?.selected || 'local-testnet' });
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
  const password = prompt('Enter wallet password to claim unstake:');
  if (!password) return;
  try {
    await claimReefStake({ wallet, password, network: state.network?.selected || 'local-testnet' });
    alert('Claim successful!');
    loadStakingTab();
  } catch (err) {
    alert('Claim failed: ' + err.message);
  }
}

async function loadIdentityTab() {
  const wallet = getActiveWallet();
  const container = $('identityContent');
  if (!wallet || !container) return;

  container.innerHTML = '<div class="empty-state"><i class="fas fa-spinner fa-spin"></i> Loading MoltyID...</div>';

  try {
    const data = await loadIdentityDetails(wallet.address, state.network?.selected);

    if (!data) {
      // No identity — show onboarding with Register step
      container.innerHTML = `
        <div class="id-banner" style="text-align:center;padding:1.5rem;">
          <div style="font-size:2rem;margin-bottom:0.75rem;"><i class="fas fa-fingerprint" style="color:var(--primary);"></i></div>
          <h3 style="margin-bottom:0.25rem;">MoltyID — On-Chain Identity</h3>
          <p style="color:var(--text-muted);font-size:0.85rem;">Your portable reputation, .molt name, skills, and agent profile — all on MoltChain.</p>
        </div>
        <div class="id-onboard" style="display:flex;flex-direction:column;gap:0.5rem;padding:0 1rem 1rem;">
          <div class="id-onboard-step" id="idRegisterStep" style="display:flex;align-items:center;gap:1rem;padding:1rem;background:var(--bg-card);border:1px solid var(--primary);border-radius:12px;cursor:pointer;transition:background 0.2s;">
            <div style="width:36px;height:36px;border-radius:50%;background:var(--primary);color:#fff;display:flex;align-items:center;justify-content:center;font-weight:700;flex-shrink:0;">1</div>
            <div style="flex:1;">
              <div style="font-weight:600;">Register Identity</div>
              <div style="font-size:0.82rem;color:var(--text-muted);">Choose a display name and agent type. Free — only the 0.0001 MOLT tx fee.</div>
            </div>
            <i class="fas fa-chevron-right" style="color:var(--primary);"></i>
          </div>
          <div style="display:flex;align-items:center;gap:1rem;padding:1rem;background:var(--bg-card);border:1px solid var(--border);border-radius:12px;opacity:0.5;">
            <div style="width:36px;height:36px;border-radius:50%;background:var(--bg-tertiary);color:var(--text-muted);display:flex;align-items:center;justify-content:center;flex-shrink:0;"><i class="fas fa-lock" style="font-size:0.65rem;"></i></div>
            <div style="flex:1;">
              <div style="font-weight:600;">Claim a .molt Name</div>
              <div style="font-size:0.82rem;color:var(--text-muted);">Register a human-readable name (5+ chars, 20 MOLT/year).</div>
            </div>
          </div>
          <div style="display:flex;align-items:center;gap:1rem;padding:1rem;background:var(--bg-card);border:1px solid var(--border);border-radius:12px;opacity:0.5;">
            <div style="width:36px;height:36px;border-radius:50%;background:var(--bg-tertiary);color:var(--text-muted);display:flex;align-items:center;justify-content:center;flex-shrink:0;"><i class="fas fa-lock" style="font-size:0.65rem;"></i></div>
            <div style="flex:1;">
              <div style="font-weight:600;">Build Reputation</div>
              <div style="font-size:0.82rem;color:var(--text-muted);">Earn rep through transactions, governance, vouches. Unlock trust tiers.</div>
            </div>
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
    const moltNameDisplay = data.name && data.name.endsWith('.molt') ? data.name : (data.name ? data.name + '.molt' : '');
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
          const name = String(s.name || s.skill || 'Unnamed');
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
          const label = v.voucher_name ? v.voucher_name + '.molt' : fmtAddr(v.voucher, 8);
          return `<span style="display:inline-block;padding:0.2rem 0.6rem;background:var(--bg-tertiary);border-radius:6px;font-size:0.75rem;margin:0.15rem;">${label}</span>`;
        }).join('')
      : '<span style="color:var(--text-muted);font-size:0.82rem;">None yet</span>';

    const allAchievements = ACHIEVEMENT_DEFS.map(def => {
      const earned = achievedIds.has(def.id);
      return `<span style="display:inline-block;padding:0.25rem 0.6rem;border-radius:6px;font-size:0.75rem;margin:0.15rem;${earned ? 'background:var(--primary)18;color:var(--primary);border:1px solid var(--primary)33;' : 'background:var(--bg-tertiary);color:var(--text-muted);opacity:0.5;'}"><i class="${def.icon}"></i> ${def.name}</span>`;
    }).join('');

    container.innerHTML = `
      <!-- Profile Strip -->
      <div style="display:flex;align-items:center;gap:1rem;padding:1.25rem;border-bottom:1px solid var(--border);">
        <div style="width:48px;height:48px;border-radius:50%;background:${tier.color}18;border:2px solid ${tier.color};display:flex;align-items:center;justify-content:center;">
          <i class="fas fa-fingerprint" style="color:${tier.color};font-size:1.25rem;"></i>
        </div>
        <div style="flex:1;">
          <div style="font-weight:700;font-size:1.1rem;">${displayName}${moltNameDisplay ? ` <span style="color:var(--primary);">${moltNameDisplay}</span>` : ''}</div>
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

        <!-- .molt Name -->
        <div style="background:var(--bg-card);border:1px solid var(--border);border-radius:12px;padding:1rem;">
          <div style="display:flex;justify-content:space-between;align-items:center;margin-bottom:0.75rem;">
            <span style="font-weight:600;font-size:0.85rem;"><i class="fas fa-at"></i> .molt Name</span>
          </div>
          ${data.name ? `
            <div style="font-size:1.25rem;font-weight:700;">${data.name.endsWith('.molt') ? data.name : data.name + '.molt'}</div>
            <div style="display:flex;gap:0.5rem;margin-top:0.75rem;flex-wrap:wrap;">
              <button class="btn btn-small btn-secondary" id="idRenewNameBtn"><i class="fas fa-redo"></i> Renew</button>
              <button class="btn btn-small btn-secondary" id="idTransferNameBtn"><i class="fas fa-exchange-alt"></i> Transfer</button>
              <button class="btn btn-small btn-danger" id="idReleaseNameBtn" style="font-size:0.75rem;"><i class="fas fa-trash-alt"></i> Release</button>
            </div>
          ` : `
            <div style="color:var(--text-muted);font-size:0.82rem;margin-bottom:0.5rem;">No name registered</div>
            <small style="color:var(--text-muted);">5+ chars from 20 MOLT/yr</small>
            <button class="btn btn-small btn-primary" id="idRegisterNameBtn" style="margin-top:0.5rem;"><i class="fas fa-plus"></i> Register</button>
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
            <div><span style="color:var(--text-muted);display:block;font-size:0.72rem;">Endpoint</span><span style="font-family:monospace;">${data.endpoint || '<em style="opacity:0.4;">Not set</em>'}</span></div>
            <div><span style="color:var(--text-muted);display:block;font-size:0.72rem;">Status</span>${data.availability === 'online' ? '<span style="color:#4ade80;">Online</span>' : '<span style="color:var(--text-muted);">Offline</span>'}</div>
            <div><span style="color:var(--text-muted);display:block;font-size:0.72rem;">Rate</span>${data.rate.toLocaleString(undefined, { maximumFractionDigits: 9 })} MOLT/req</div>
          </div>
        </div>
      </div>
    `;

    // Wire action buttons
    $('idEditProfileBtn')?.addEventListener('click', () => showIdentityEditProfileModal(data.agentType));
    $('idAddSkillBtn')?.addEventListener('click', () => showIdentityAddSkillModal());
    $('idVouchBtn')?.addEventListener('click', () => showIdentityVouchModal());
    $('idRegisterNameBtn')?.addEventListener('click', () => showIdentityRegisterNameModal());
    $('idRenewNameBtn')?.addEventListener('click', () => showIdentityRenewNameModal(data.name));
    $('idTransferNameBtn')?.addEventListener('click', () => showIdentityTransferNameModal(data.name));
    $('idReleaseNameBtn')?.addEventListener('click', () => showIdentityReleaseNameModal(data.name));
    $('idConfigAgentBtn')?.addEventListener('click', () => showIdentityAgentConfigModal(data));

  } catch (e) {
    container.innerHTML = `<div class="empty-state"><p>Failed to load identity: ${e.message}</p></div>`;
  }
}

/* ── Identity Action Modals ── */

function showIdentityPrompt(title, fields, onSubmit) {
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
    return `<div class="form-group" style="margin-bottom:0.75rem;"><label style="font-size:0.82rem;color:var(--text-muted);display:block;margin-bottom:0.25rem;">${f.label}</label><input type="${f.type || 'text'}" id="idModal_${f.id}" class="form-input" placeholder="${f.placeholder || ''}" value="${f.value || ''}" style="width:100%;"></div>`;
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
      await loadIdentityTab();
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

  showIdentityPrompt('Register MoltyID', [
    { type: 'info', html: 'Create your on-chain identity. Choose a display name and agent type.<br><small>Free — only the 0.0001 MOLT tx fee applies.</small>' },
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
    { id: 'proficiency', label: 'Proficiency (1-100)', type: 'number', placeholder: '50' },
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
    { type: 'info', html: 'Vouch for another MoltyID holder. Both parties must have registered identities.' },
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

  showIdentityPrompt('Register .molt Name', [
    { type: 'info', html: '<div style="display:flex;flex-direction:column;gap:0.25rem;"><div><strong>5+ chars</strong> — 20 MOLT/year</div><div style="opacity:0.6;"><strong>4 chars</strong> — 100 MOLT/year (auction only)</div><div style="opacity:0.6;"><strong>3 chars</strong> — 500 MOLT/year (auction only)</div></div><small>Names: lowercase, 3-32 chars (a-z, 0-9, hyphens). Duration: 1-10 years.</small>' },
    { id: 'name', label: 'Name (without .molt)', type: 'text', placeholder: 'myname' },
    { id: 'duration', label: 'Duration (years)', type: 'number', placeholder: '1', value: '1' },
    { id: 'password', label: 'Wallet Password', type: 'password', placeholder: 'Sign transaction' }
  ], async (values) => {
    await registerMoltName({
      wallet, password: values.password, network: state.network?.selected,
      name: values.name, durationYears: values.duration
    });
  });
}

async function showIdentityRenewNameModal(currentName) {
  const wallet = getActiveWallet();
  if (!wallet) return;
  const name = (currentName || '').replace(/\.molt$/, '');

  showIdentityPrompt(`Renew ${name}.molt`, [
    { id: 'years', label: 'Additional Years', type: 'number', placeholder: '1', value: '1' },
    { id: 'password', label: 'Wallet Password', type: 'password', placeholder: 'Sign transaction' }
  ], async (values) => {
    await renewMoltName({
      wallet, password: values.password, network: state.network?.selected,
      name, additionalYears: values.years
    });
  });
}

async function showIdentityTransferNameModal(currentName) {
  const wallet = getActiveWallet();
  if (!wallet) return;
  const name = (currentName || '').replace(/\.molt$/, '');

  showIdentityPrompt(`Transfer ${name}.molt`, [
    { type: 'info', html: 'Transfer ownership to another address. <strong>This is irreversible.</strong>' },
    { id: 'recipient', label: 'Recipient Address', type: 'text', placeholder: 'Base58 address' },
    { id: 'password', label: 'Wallet Password', type: 'password', placeholder: 'Sign transaction' }
  ], async (values) => {
    await transferMoltName({
      wallet, password: values.password, network: state.network?.selected,
      name, recipient: values.recipient
    });
  });
}

async function showIdentityReleaseNameModal(currentName) {
  const wallet = getActiveWallet();
  if (!wallet) return;
  const name = (currentName || '').replace(/\.molt$/, '');

  if (!confirm(`Release ${name}.molt? This is permanent and cannot be undone.`)) return;

  showIdentityPrompt(`Confirm Release: ${name}.molt`, [
    { type: 'info', html: `You are about to permanently release <strong>${name}.molt</strong>. It can be re-registered by anyone.` },
    { id: 'password', label: 'Wallet Password', type: 'password', placeholder: 'Sign transaction' }
  ], async (values) => {
    await releaseMoltName({
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
    { id: 'rate', label: 'Rate (MOLT per request)', type: 'number', placeholder: '0.001', value: String(data.rate || 0) },
    { id: 'availability', label: 'Availability', type: 'select', options: [
      { value: 'online', label: 'Online', selected: data.availability === 'online' },
      { value: 'offline', label: 'Offline', selected: data.availability !== 'online' }
    ]},
    { id: 'password', label: 'Wallet Password', type: 'password', placeholder: 'Sign transaction' }
  ], async (values) => {
    const tasks = [];
    if (values.endpoint !== (data.endpoint || '')) {
      tasks.push(() => setIdentityEndpoint({ wallet, password: values.password, network: state.network?.selected, endpoint: values.endpoint }));
    }
    if (Number(values.rate || 0) !== data.rate) {
      tasks.push(() => setIdentityRate({ wallet, password: values.password, network: state.network?.selected, rateMolt: values.rate }));
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
    const raw = Number(result?.shells || result?.spendable || 0);
    const molt = raw / 1_000_000_000;
    const d = decimals();

    list.innerHTML = `
      <div class="asset-item">
        <div class="asset-icon" style="background:rgba(255,107,53,0.12);color:var(--primary);">🦞</div>
        <div class="asset-info">
          <div class="asset-name">MOLT</div>
          <div class="asset-symbol">MoltChain Native Token</div>
        </div>
        <div class="asset-balance">
          <div class="asset-amount">${molt.toLocaleString(undefined, { minimumFractionDigits: 2, maximumFractionDigits: 9 })}</div>
          <div class="asset-value">$${(molt * 0.10).toLocaleString(undefined, { minimumFractionDigits: 2, maximumFractionDigits: 6 })}</div>
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
      };
      const type = typeMap[tx.type] || (isSend ? 'Sent' : 'Received');

      // Icons & colors — aligned with wallet website
      let icon = isSend ? 'fa-arrow-up' : 'fa-arrow-down';
      let color = isSend ? '#ff6b35' : '#4ade80';
      let sign = isSend ? '-' : '+';

      if (tx.type === 'Stake' || tx.type === 'Unstake' || tx.type === 'ClaimUnstake') {
        icon = 'fa-coins'; color = '#a78bfa';
      } else if (tx.type === 'RegisterEvmAddress') {
        icon = 'fa-link'; color = '#94a3b8';
      } else if (tx.type === 'Contract') {
        icon = 'fa-file-code'; color = '#f59e0b';
      } else if (tx.type === 'Reward' || tx.type === 'GenesisTransfer' || tx.type === 'GenesisMint') {
        icon = 'fa-gift'; color = '#4ade80'; sign = '+';
      } else if (tx.type === 'Airdrop') {
        icon = 'fa-parachute-box'; color = '#60a5fa';
      }

      const address = isSend ? (tx.to || '') : (tx.from || '');
      const displayAddr = address && address.length > 20 ? address.slice(0, 8) + '…' + address.slice(-4) : (address || '');
      const amountVal = tx.amount_shells ? tx.amount_shells : (tx.amount || 0);
      const amt = (Number(amountVal) / 1_000_000_000).toLocaleString(undefined, { maximumFractionDigits: 4 });
      const ts = tx.timestamp ? new Date(tx.timestamp * 1000).toLocaleString() : '';
      const explorerLink = sig !== 'unknown' ? `${explorerBase}${sig}` : '#';

      // Fee display: show actual fee amount for 0-amount contract calls / EVM registration
      const isZeroAmount = Number(amountVal) === 0;
      const isFeeOnly = tx.type === 'RegisterEvmAddress' || (tx.type === 'Contract' && isZeroAmount);
      const feeShells = tx.fee_shells || tx.fee || 0;
      const feeAmt = (Number(feeShells) / 1_000_000_000).toLocaleString(undefined, { maximumFractionDigits: 4 });
      const amountStr = isFeeOnly ? `${feeAmt} MOLT` : `${sign}${amt} MOLT`;
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
      list.insertAdjacentHTML('beforeend', `
        <div class="activity-load-more" style="text-align:center;padding:1rem;">
          <button onclick="loadActivity(false)" class="btn btn-small btn-secondary" style="padding:0.5rem 1.5rem;font-size:0.85rem;">
            Load More
          </button>
        </div>
      `);
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
    const spendable = Number(balResult?.spendable || balResult?.shells || 0) / 1_000_000_000;
    if (spendable < amount + 0.001) {
      showToast(`Insufficient balance: need ${(amount + 0.001).toLocaleString(undefined, { maximumFractionDigits: 9 })}, have ${spendable.toLocaleString(undefined, { maximumFractionDigits: 9 })}`, 'error');
      return;
    }

    const privKey = await decryptPrivateKey(wallet.encryptedKey, pw);
    const block = await rpc().getLatestBlock();
    const blockhash = block?.hash || block?.blockhash || 'genesis';

    const tx = await buildSignedNativeTransferTransaction({
      privateKeyHex: privKey,
      fromPublicKeyHex: wallet.publicKey,
      toAddress: to,
      amountMolt: amount,
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
    const privBytes = hexToBytes(privHex);
    const pubBytes = hexToBytes(wallet.publicKey);
    const secretKey = new Uint8Array(64);
    secretKey.set(privBytes, 0);
    secretKey.set(pubBytes, 32);

    const keystore = {
      name: wallet.name,
      address: wallet.address,
      publicKey: Array.from(pubBytes),
      secretKey: Array.from(secretKey),
      created: wallet.createdAt,
      exported: new Date().toISOString(),
      version: '1.0'
    };

    const blob = new Blob([JSON.stringify(keystore, null, 2)], { type: 'application/json' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = `molt-wallet-keystore-${wallet.name}-${Date.now()}.json`;
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
    // Generate EVM-compatible address (keccak256 of public key → last 20 bytes, 0x-prefixed)
    const evmEl = $('walletAddressEVM');
    if (evmEl) evmEl.value = wallet.evmAddress || deriveEvmAddress(wallet.publicKey) || '';
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
   Bridge Deposit — wired to custody service
   ────────────────────────────────────────── */
const BRIDGE_ASSETS_EXT = ['usdc', 'usdt'];
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
  return String(str).replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;').replace(/"/g,'&quot;').replace(/'/g,'&#x27;');
}

async function startExtensionDeposit(chain) {
  const wallet = getActiveWallet();
  if (!wallet) { showToast('No active wallet', 'error'); return; }
  if (!isValidAddress(wallet.address)) { showToast('Invalid wallet address', 'error'); return; }

  const chainLabels = { solana: 'Solana', ethereum: 'Ethereum' };
  const chainLabel = chainLabels[chain] || chain;

  // Show asset picker inline in depositTabContent
  const container = $('depositTabContent');
  if (!container) return;

  const tokenButtons = BRIDGE_ASSETS_EXT.map(a =>
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
    const response = await requestBridgeDepositAddress({
      userAddress: wallet.address,
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
        <div style="background:rgba(255,107,53,0.06);border-radius:8px;padding:1rem;text-align:left;">
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
        const statusResult = await getBridgeDepositStatus(extActiveDepositId, network);
        const statusValue = String(statusResult.status || 'issued').toLowerCase();
        const statusEl = container.querySelector('#extDepositStatus');
        const statusMap = {
          issued:    '<i class="fas fa-clock" style="color:var(--text-muted);"></i> Waiting for deposit...',
          pending:   '<i class="fas fa-spinner fa-spin" style="color:#FFD166;"></i> Deposit detected, confirming...',
          confirmed: '<i class="fas fa-check-circle" style="color:#06D6A0;"></i> Confirmed! Sweeping to treasury...',
          swept:     '<i class="fas fa-exchange-alt" style="color:#06D6A0;"></i> Swept! Minting wrapped tokens...',
          credited:  '<i class="fas fa-check-double" style="color:#06D6A0;"></i> Credited to your wallet!',
          expired:   '<i class="fas fa-times-circle" style="color:#EF476F;"></i> Deposit expired.'
        };
        if (statusEl) statusEl.innerHTML = statusMap[statusValue] || statusMap['issued'];
        consecutiveErrors = 0;
        if (statusValue === 'credited' || statusValue === 'expired') {
          clearExtDepositPolling();
          if (statusValue === 'credited') showToast('Bridge deposit credited!', 'success');
        }
      } catch {
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
    <p style="text-align:center;color:var(--text-secondary);margin-bottom:1.25rem;font-size:0.95rem;">Deposit assets to your MoltChain wallet via bridge</p>
    <div class="deposit-options">
      <div class="deposit-card" id="depositSOL">
        <div class="deposit-card-icon" style="background:rgba(153,69,255,0.12);color:#9945FF;"><i class="fas fa-sun"></i></div>
        <div class="deposit-card-info"><strong>Bridge from Solana</strong><span>USDC, USDT</span></div>
        <i class="fas fa-chevron-right" style="color:var(--text-muted);"></i>
      </div>
      <div class="deposit-card" id="depositETH">
        <div class="deposit-card-icon" style="background:rgba(98,126,234,0.12);color:#627EEA;"><i class="fab fa-ethereum"></i></div>
        <div class="deposit-card-info"><strong>Bridge from Ethereum</strong><span>USDC, USDT</span></div>
        <i class="fas fa-chevron-right" style="color:var(--text-muted);"></i>
      </div>
      <div class="deposit-card disabled">
        <div class="deposit-card-icon" style="background:rgba(255,107,53,0.12);color:var(--primary);"><i class="fas fa-credit-card"></i></div>
        <div class="deposit-card-info"><strong>Buy with Fiat</strong><span>Coming with mainnet launch</span></div>
        <span class="label-badge">Soon</span>
      </div>
    </div>
    <div style="text-align:center;margin-top:1.5rem;padding:0.75rem;background:rgba(255,107,53,0.08);border-radius:8px;font-size:0.85rem;color:var(--text-secondary);">
      <i class="fas fa-shield-alt" style="color:var(--primary);"></i> Bridge contracts are audited. Deposits typically confirm in 2-5 minutes.
    </div>
  `;
  // Re-wire click handlers
  container.querySelector('#depositSOL')?.addEventListener('click', () => startExtensionDeposit('solana'));
  container.querySelector('#depositETH')?.addEventListener('click', () => startExtensionDeposit('ethereum'));
}

function deriveEvmAddress(pubKeyHex) {
  if (!pubKeyHex) return '';
  const hex = pubKeyHex.replace(/^0x/, '');
  try {
    const pubBytes = new Uint8Array(32);
    for (let i = 0; i < 32; i++) pubBytes[i] = parseInt(hex.substr(i * 2, 2), 16);
    const hashHex = keccak256(pubBytes);
    return '0x' + hashHex.slice(-40);
  } catch {
    return '0x' + hex.slice(0, 40);
  }
}

/* ──────────────────────────────────────────
   Send — available balance display
   ────────────────────────────────────────── */
async function updateSendAvailableBalance() {
  const el = $('sendAvailableBalance');
  if (!el) return;
  const wallet = getActiveWallet();
  if (!wallet) { el.textContent = ''; return; }
  try {
    const result = await rpc().getBalance(wallet.address);
    const raw = Number(result?.spendable || result?.shells || 0) / 1_000_000_000;
    el.textContent = `Available: ${raw.toLocaleString(undefined, { maximumFractionDigits: decimals() })} MOLT`;
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
  showToast(`Network switched to ${ns.value}`, 'success');
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
  $('refreshNftsBtn')?.addEventListener('click', () => showToast('NFT refresh coming soon'));

  // Send modal
  $('showSendBtn')?.addEventListener('click', () => { openModal('sendModal'); updateSendAvailableBalance(); });
  $('closeSendModal')?.addEventListener('click', () => closeModal('sendModal'));
  $('cancelSendBtn')?.addEventListener('click', () => closeModal('sendModal'));
  $('confirmSendBtn')?.addEventListener('click', handleSend);
  $('sendMaxBtn')?.addEventListener('click', async () => {
    const wallet = getActiveWallet();
    if (!wallet) return;
    try {
      const result = await rpc().getBalance(wallet.address);
      const spendable = Number(result?.spendable || result?.shells || 0) / 1_000_000_000;
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
