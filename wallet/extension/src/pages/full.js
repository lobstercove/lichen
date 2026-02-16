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
  base58Encode
} from '../core/crypto-service.js';
import { buildSignedNativeTransferTransaction, encodeTransactionBase64 } from '../core/tx-service.js';
import { notify } from '../core/notification-service.js';

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

function showScreen(id) {
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

  createdMnemonic = generateMnemonic();
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
    await persist({ ...state, isLocked: false });
    showToast('Wallet unlocked!', 'success');
    showDashboard();
  } catch {
    showToast('Incorrect password', 'error');
  }
}

async function handleLock() {
  await persist({ ...state, isLocked: true });
  showScreen('unlockScreen');
}

async function handleLogout() {
  if (!confirm('This will remove all wallets from this extension. Make sure you have your seed phrase backed up!')) return;
  await chrome.storage.local.clear();
  state = { wallets: [], activeWalletId: null, isLocked: true, settings: { currency: 'USD', lockTimeout: 300000 }, network: { selected: 'local-testnet' } };
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
  const dropdown = $('walletDropdown');
  dropdown.innerHTML = state.wallets.map(w =>
    `<div class="wallet-dropdown-item ${w.id === state.activeWalletId ? 'active' : ''}" data-wid="${w.id}">
      <i class="fas fa-wallet" style="margin-right:0.5rem;"></i> ${w.name} <span style="color:var(--text-muted);margin-left:auto;font-size:0.78rem;">${maskAddr(w.address)}</span>
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
      if (name === 'governance') loadGovernanceTab();
      if (name === 'bridge') loadBridgeTab();
      if (name === 'prediction') loadPredictionTab();
    });
  });
}

async function refreshBalance() {
  const wallet = getActiveWallet();
  if (!wallet) return;

  try {
    const result = await rpc().getBalance(wallet.address);
    const raw = Number(result?.balance || 0);
    const molt = raw / 1_000_000_000;
    const d = decimals();
    $('totalBalance').textContent = `${molt.toLocaleString(undefined, { maximumFractionDigits: d })} MOLT`;
    $('balanceUsd').textContent = `$${(molt * 0.05).toFixed(2)} USD`;
  } catch {
    $('totalBalance').textContent = '0.00 MOLT';
    $('balanceUsd').textContent = '$0.00 USD';
  }
}

async function loadIdentityTab() {
  const wallet = getActiveWallet();
  const container = $('identityContent');
  if (!wallet || !container) return;

  container.innerHTML = '<div class="empty-state"><i class="fas fa-spinner fa-spin"></i> Loading MoltyID...</div>';

  try {
    const result = await rpc().call('getIdentity', [wallet.address]);
    const identity = result?.identity || result;
    if (!identity || !identity.name) {
      container.innerHTML = `
        <div class="empty-state">
          <span class="empty-icon"><i class="fas fa-fingerprint"></i></span>
          <h3>No MoltyID Yet</h3>
          <p>Register an on-chain identity to get started with MoltyID.</p>
        </div>
      `;
      return;
    }
    const rep = Number(identity.reputation || 0);
    const repPct = Math.min(100, (rep / 10000) * 100);
    container.innerHTML = `
      <div style="text-align:center;padding:1.5rem 0;">
        <div style="font-size:2rem;"><i class="fas fa-fingerprint" style="color:var(--primary);"></i></div>
        <h3 style="margin:0.75rem 0 0.25rem;">${identity.name}${identity.molt_name ? ' <span style="color:var(--primary);">' + identity.molt_name + '</span>' : ''}</h3>
        <div style="font-size:0.9rem;color:var(--text-muted);">Reputation: ${rep.toLocaleString()} / 10,000</div>
        <div style="margin-top:0.75rem;height:6px;background:var(--bg-tertiary);border-radius:3px;overflow:hidden;max-width:300px;margin-left:auto;margin-right:auto;">
          <div style="height:100%;width:${repPct}%;background:var(--primary);border-radius:3px;"></div>
        </div>
      </div>
    `;
  } catch {
    container.innerHTML = '<div class="empty-state"><p>Failed to load identity</p></div>';
  }
}

async function loadGovernanceTab() {
  const container = $('govProposalsList');
  if (!container) return;
  container.innerHTML = '<div class="empty-state"><span class="spinner"></span></div>';

  try {
    const result = await rpc().call('getGovernanceProposals', []);
    const proposals = result?.proposals || (Array.isArray(result) ? result : []);
    if (!proposals.length) {
      container.innerHTML = '<p class="empty-state"><i class="fas fa-vote-yea"></i> No active proposals</p>';
      return;
    }
    container.innerHTML = proposals.slice(0, 10).map(p => `
      <div class="proposal-item">
        <strong>${p.title || 'Proposal #' + p.id}</strong>
        <span class="label-badge">${p.status || 'active'}</span>
      </div>
    `).join('');
  } catch {
    container.innerHTML = '<p class="empty-state"><i class="fas fa-vote-yea"></i> Failed to load proposals</p>';
  }
}

async function loadBridgeTab() {
  // Bridge form is static HTML; no additional data load needed
}

async function loadPredictionTab() {
  const container = $('predMarketsList');
  if (!container) return;
  container.innerHTML = '<div class="empty-state"><span class="spinner"></span></div>';

  try {
    const result = await rpc().call('getPredictionMarkets', []);
    const markets = result?.markets || (Array.isArray(result) ? result : []);
    if (!markets.length) {
      container.innerHTML = '<p class="empty-state"><i class="fas fa-chart-bar"></i> No active markets</p>';
      return;
    }
    container.innerHTML = markets.slice(0, 10).map(m => `
      <div class="market-item">
        <strong>${m.question || 'Market #' + m.id}</strong>
        <span class="label-badge">${m.status || 'open'}</span>
      </div>
    `).join('');
  } catch {
    container.innerHTML = '<p class="empty-state"><i class="fas fa-chart-bar"></i> Failed to load markets</p>';
  }
}

async function loadAssets() {
  const wallet = getActiveWallet();
  const list = $('assetsList');
  if (!wallet || !list) return;

  list.innerHTML = '<div class="empty-state"><span class="spinner"></span></div>';

  try {
    const result = await rpc().getBalance(wallet.address);
    const raw = Number(result?.balance || 0);
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
          <div class="asset-amount">${molt.toLocaleString(undefined, { maximumFractionDigits: d })}</div>
          <div class="asset-value">$${(molt * 0.05).toFixed(2)}</div>
        </div>
      </div>
    `;
  } catch {
    list.innerHTML = '<div class="empty-state"><p>Failed to load assets</p></div>';
  }
}

async function loadActivity() {
  const wallet = getActiveWallet();
  const list = $('activityList');
  if (!wallet || !list) return;

  list.innerHTML = '<div class="empty-state"><span class="spinner"></span></div>';

  try {
    const result = await rpc().getTransactionsByAddress(wallet.address, { limit: 15 });
    const txs = result?.transactions || (Array.isArray(result) ? result : []);

    if (!txs.length) {
      list.innerHTML = '<div class="empty-state"><span class="empty-icon"><i class="fas fa-history"></i></span><p>No recent activity</p></div>';
      return;
    }

    list.innerHTML = txs.map(tx => {
      const sig = tx.signature || tx.hash || 'unknown';
      const block = tx.block_height || tx.slot || '—';
      const short = `${String(sig).slice(0, 10)}…${String(sig).slice(-6)}`;
      const isSend = (tx.from === wallet.address);
      const amt = tx.amount ? (Number(tx.amount) / 1_000_000_000).toFixed(4) : '';
      return `
        <div class="activity-item">
          <div class="activity-icon ${isSend ? 'send' : 'receive'}">
            <i class="fas fa-arrow-${isSend ? 'up' : 'down'}"></i>
          </div>
          <div class="activity-details">
            <div class="activity-type">${isSend ? 'Sent' : 'Received'}${amt ? ` ${amt} MOLT` : ''}</div>
            <div class="activity-date">Block #${block} · ${short}</div>
          </div>
        </div>`;
    }).join('');
  } catch {
    list.innerHTML = '<div class="empty-state"><p>Failed to load activity</p></div>';
  }
}

/* ──────────────────────────────────────────
   Send Modal
   ────────────────────────────────────────── */
function openModal(id) { $(id)?.classList.add('show'); }
function closeModal(id) { $(id)?.classList.remove('show'); }

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
    const spendable = Number(balResult?.spendable || balResult?.balance || 0) / 1_000_000_000;
    if (spendable < amount + 0.001) {
      showToast(`Insufficient balance: need ${(amount + 0.001).toFixed(6)}, have ${spendable.toFixed(6)}`, 'error');
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

function deriveEvmAddress(pubKeyHex) {
  // Simple placeholder — proper EVM address derivation needs keccak256
  // Return a truncated hex representation as a stand-in
  if (!pubKeyHex) return '';
  const hex = pubKeyHex.replace(/^0x/, '');
  return '0x' + hex.slice(0, 40);
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
    const raw = Number(result?.spendable || result?.balance || 0) / 1_000_000_000;
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
      const spendable = Number(result?.spendable || result?.balance || 0) / 1_000_000_000;
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
  $('depositSOL')?.addEventListener('click', () => showToast('Solana bridge coming soon'));
  $('depositETH')?.addEventListener('click', () => showToast('Ethereum bridge coming soon'));

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
  }

  if (!state.isLocked) {
    await scheduleAutoLock(state.settings?.lockTimeout || 300000);
  }
}

boot();
