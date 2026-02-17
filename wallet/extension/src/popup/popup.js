import { loadState, saveState } from '../core/state-store.js';
import { getRpcEndpoint, MoltChainRPC } from '../core/rpc-service.js';
import { clearAutoLockAlarm, scheduleAutoLock } from '../core/lock-service.js';
import {
  decryptPrivateKey,
  encryptPrivateKey,
  generateId,
  generateMnemonic,
  isValidAddress,
  isValidMnemonic,
  mnemonicToKeypair,
  privateKeyToKeypair,
  bytesToHex
} from '../core/crypto-service.js';
import { buildSignedNativeTransferTransaction, encodeTransactionBase64, registerEvmAddress } from '../core/tx-service.js';
import { notify } from '../core/notification-service.js';

let state = null;
let pendingGeneratedMnemonic = '';
let fullCarouselTimer = null;
let createWizardState = {
  step: 1,
  mnemonicWords: [],
  selectedWords: []
};

function isFullPageMode() {
  const params = new URLSearchParams(window.location.search);
  return params.get('mode') === 'full';
}

function applyViewMode() {
  if (isFullPageMode()) {
    document.body.classList.add('full-page');
    document.getElementById('welcomeScreen')?.classList.add('welcome-screen');
  }
}

function initFullWelcomeCarousel() {
  if (!isFullPageMode()) return;

  const root = document.querySelector('.web-welcome');
  if (!root) return;

  const slides = Array.from(root.querySelectorAll('.carousel-slide'));
  const dots = Array.from(root.querySelectorAll('.carousel-dot'));
  if (!slides.length || !dots.length) return;

  let current = Math.max(0, slides.findIndex((slide) => slide.classList.contains('active')));
  if (current === -1) current = 0;

  const showSlide = (index) => {
    const normalized = ((index % slides.length) + slides.length) % slides.length;
    current = normalized;

    slides.forEach((slide, i) => {
      slide.classList.toggle('active', i === normalized);
    });
    dots.forEach((dot, i) => {
      dot.classList.toggle('active', i === normalized);
    });
  };

  dots.forEach((dot) => {
    dot.addEventListener('click', () => {
      const next = Number(dot.dataset.slide || 0);
      showSlide(next);
    });
  });

  if (fullCarouselTimer) {
    clearInterval(fullCarouselTimer);
  }
  fullCarouselTimer = setInterval(() => {
    showSlide(current + 1);
  }, 3500);

  showSlide(current);
}

const screens = {
  welcome: document.getElementById('welcomeScreen'),
  create: document.getElementById('createScreen'),
  import: document.getElementById('importScreen'),
  unlock: document.getElementById('unlockScreen'),
  dashboard: document.getElementById('dashboardScreen')
};

const statusField = document.getElementById('statusField');

// Security: clear all sensitive input fields across all screens
function clearAllInputs() {
  document.querySelectorAll('input, textarea').forEach(el => {
    if (el.type !== 'hidden' && el.type !== 'checkbox' && el.type !== 'radio') {
      el.value = '';
    }
  });
}

function showScreen(key) {
  clearAllInputs();
  Object.values(screens).forEach((element) => element.classList.remove('active'));
  screens[key].classList.add('active');
}

function shuffleCopy(items) {
  const array = [...items];
  for (let index = array.length - 1; index > 0; index -= 1) {
    const swapIndex = Math.floor(Math.random() * (index + 1));
    [array[index], array[swapIndex]] = [array[swapIndex], array[index]];
  }
  return array;
}

function setCreateStep(step) {
  createWizardState.step = step;

  document.querySelectorAll('[data-create-step-content]').forEach((node) => {
    const nodeStep = Number(node.getAttribute('data-create-step-content'));
    node.classList.toggle('active', nodeStep === step);
  });

  document.querySelectorAll('[data-create-step]').forEach((node) => {
    const nodeStep = Number(node.getAttribute('data-create-step'));
    node.classList.toggle('active', nodeStep === step);
    node.classList.toggle('completed', nodeStep < step);
  });
}

function renderCreateConfirmSlots() {
  const slotsRoot = document.getElementById('createConfirmSlots');
  if (!slotsRoot) return;

  const selected = createWizardState.selectedWords;
  const expected = createWizardState.mnemonicWords;
  slotsRoot.innerHTML = expected.map((_, index) => {
    const selectedWord = selected[index] || '';
    const isFilled = Boolean(selectedWord);
    const isCorrect = isFilled && selectedWord === expected[index];
    return `
      <button type="button" class="confirm-slot ${isFilled ? 'filled' : ''} ${isCorrect ? 'correct' : ''}" data-confirm-slot="${index}">
        <span class="slot-number">${index + 1}.</span>
        <span>${selectedWord || ''}</span>
      </button>
    `;
  }).join('');

  slotsRoot.querySelectorAll('[data-confirm-slot]').forEach((node) => {
    node.addEventListener('click', () => {
      const index = Number(node.getAttribute('data-confirm-slot'));
      if (!Number.isInteger(index)) return;
      if (!createWizardState.selectedWords[index]) return;
      createWizardState.selectedWords[index] = '';
      renderCreateConfirmSlots();
      renderCreateConfirmPool();
      updateCreateConfirmButton();
    });
  });
}

function renderCreateConfirmPool() {
  const poolRoot = document.getElementById('createConfirmPool');
  if (!poolRoot) return;

  const selected = createWizardState.selectedWords;
  const usedCountByWord = selected.reduce((acc, word) => {
    if (!word) return acc;
    acc[word] = (acc[word] || 0) + 1;
    return acc;
  }, {});

  poolRoot.innerHTML = createWizardState.poolWords.map((word, index) => {
    const expectedCount = createWizardState.mnemonicWords.filter((entry) => entry === word).length;
    const usedCount = usedCountByWord[word] || 0;
    const isUsed = usedCount >= expectedCount;
    return `
      <button type="button" class="confirm-word ${isUsed ? 'used' : ''}" data-confirm-word-index="${index}">${word}</button>
    `;
  }).join('');

  poolRoot.querySelectorAll('[data-confirm-word-index]').forEach((node) => {
    node.addEventListener('click', () => {
      if (node.classList.contains('used')) return;
      const index = Number(node.getAttribute('data-confirm-word-index'));
      if (!Number.isInteger(index)) return;
      const word = createWizardState.poolWords[index];
      const nextSlot = createWizardState.selectedWords.findIndex((entry) => !entry);
      if (nextSlot === -1) return;
      createWizardState.selectedWords[nextSlot] = word;
      renderCreateConfirmSlots();
      renderCreateConfirmPool();
      updateCreateConfirmButton();
    });
  });
}

function updateCreateConfirmButton() {
  const finishButton = document.getElementById('createFinish');
  if (!finishButton) return;

  const selected = createWizardState.selectedWords;
  const expected = createWizardState.mnemonicWords;
  const complete = selected.every((word) => Boolean(word));
  const exact = complete && selected.every((word, index) => word === expected[index]);
  finishButton.disabled = !exact;
}

function startCreateFlow() {
  createWizardState = {
    step: 1,
    mnemonicWords: [],
    selectedWords: [],
    poolWords: []
  };
  pendingGeneratedMnemonic = '';

  const mnemonicField = document.getElementById('createMnemonic');
  if (mnemonicField) mnemonicField.value = '';
  setCreateStep(1);
  showScreen('create');
}

function buildConfirmChallengeFromMnemonic(mnemonic) {
  const words = String(mnemonic || '').trim().toLowerCase().split(/\s+/).filter(Boolean);
  createWizardState.mnemonicWords = words;
  createWizardState.selectedWords = Array.from({ length: words.length }, () => '');
  createWizardState.poolWords = shuffleCopy(words);
  renderCreateConfirmSlots();
  renderCreateConfirmPool();
  updateCreateConfirmButton();
}

function setStatus(message) {
  if (statusField) {
    statusField.textContent = message;
  }
}

function getActiveWallet() {
  return state.wallets.find((wallet) => wallet.id === state.activeWalletId) || null;
}

function maskAddress(address) {
  if (!address || address.length < 12) return address || '';
  return `${address.slice(0, 8)}...${address.slice(-6)}`;
}

function refreshSelector() {
  const selector = document.getElementById('walletSelector');
  selector.innerHTML = '';

  state.wallets.forEach((wallet) => {
    const option = document.createElement('option');
    option.value = wallet.id;
    option.textContent = wallet.name;
    if (wallet.id === state.activeWalletId) option.selected = true;
    selector.appendChild(option);
  });
}

function resolveRpcEndpoint(network) {
  return getRpcEndpoint(network, state?.settings || {});
}

function displayDecimals() {
  const decimals = Number(state?.settings?.decimals ?? 6);
  if (!Number.isInteger(decimals) || decimals < 0 || decimals > 12) {
    return 6;
  }
  return decimals;
}

function setDashboardPanel(panelName) {
  // Toggle tab content visibility
  document.querySelectorAll('.popup-tab-content').forEach((el) => {
    el.classList.toggle('active', el.dataset.tab === panelName);
  });
  // Toggle active tab button
  document.querySelectorAll('.popup-dash-tab').forEach((tab) => {
    tab.classList.toggle('active', tab.dataset.tab === panelName);
  });
}

function lockTimeoutMsFromMinutes(minutes) {
  const parsed = Number(minutes);
  if (!Number.isFinite(parsed) || parsed < 0) return 300000;
  if (parsed === 0) return 0;
  return parsed * 60 * 1000;
}

function lockTimeoutMinutesFromMs(ms) {
  const parsed = Number(ms);
  if (!Number.isFinite(parsed) || parsed <= 0) return '5';
  return String(Math.round(parsed / 60000));
}

async function saveAutoLockSettings() {
  const selectedMinutes = document.getElementById('settingsLockTimeout').value;
  const lockTimeout = lockTimeoutMsFromMinutes(selectedMinutes);

  await persistAndRender({
    ...state,
    settings: {
      ...(state.settings || {}),
      lockTimeout
    }
  });

  if (state.isLocked) {
    setStatus('Auto-lock saved');
    return;
  }

  if (lockTimeout === 0) {
    await clearAutoLockAlarm();
    setStatus('Auto-lock disabled');
    return;
  }

  await scheduleAutoLock(lockTimeout);
  setStatus(`Auto-lock set to ${selectedMinutes} minute(s)`);
}

function getSettingsPassword() {
  const password = document.getElementById('settingsPassword').value || '';
  if (!password) {
    throw new Error('Password is required');
  }
  return password;
}

function setExportOutput(value) {
  const field = document.getElementById('settingsExportOutput');
  if (field) field.value = value;
}

function downloadTextFile(filename, content) {
  const blob = new Blob([content], { type: 'text/plain' });
  const url = URL.createObjectURL(blob);
  const a = document.createElement('a');
  a.href = url;
  a.download = filename;
  a.click();
  URL.revokeObjectURL(url);
}

async function exportPrivateKeyAction() {
  const wallet = getActiveWallet();
  if (!wallet) {
    setStatus('No active wallet');
    return;
  }

  try {
    const password = getSettingsPassword();
    const privateKeyHex = await decryptPrivateKey(wallet.encryptedKey, password);
    setExportOutput(privateKeyHex);
    setStatus('Private key exported to output field');
  } catch (error) {
    setStatus(`Export failed: ${error?.message || error}`);
  }
}

async function exportMnemonicAction() {
  const wallet = getActiveWallet();
  if (!wallet) {
    setStatus('No active wallet');
    return;
  }

  if (!wallet.encryptedMnemonic) {
    setStatus('No seed phrase stored for this wallet');
    return;
  }

  try {
    const password = getSettingsPassword();
    const mnemonic = await decryptPrivateKey(wallet.encryptedMnemonic, password);
    setExportOutput(mnemonic);
    setStatus('Seed phrase exported to output field');
  } catch (error) {
    setStatus(`Export failed: ${error?.message || error}`);
  }
}

async function exportKeystoreJsonAction() {
  const wallet = getActiveWallet();
  if (!wallet) {
    setStatus('No active wallet');
    return;
  }

  try {
    const password = getSettingsPassword();
    const privateKeyHex = await decryptPrivateKey(wallet.encryptedKey, password);
    const privateKeyBytes = Uint8Array.from({ length: privateKeyHex.length / 2 }, (_, i) => parseInt(privateKeyHex.substr(i * 2, 2), 16));
    const publicKeyBytes = Uint8Array.from({ length: wallet.publicKey.length / 2 }, (_, i) => parseInt(wallet.publicKey.substr(i * 2, 2), 16));
    const secretKey = new Uint8Array(64);
    secretKey.set(privateKeyBytes, 0);
    secretKey.set(publicKeyBytes, 32);

    const keystore = {
      name: wallet.name,
      address: wallet.address,
      publicKey: Array.from(publicKeyBytes),
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

    setExportOutput(JSON.stringify(keystore, null, 2));
    setStatus('JSON keystore exported');
  } catch (error) {
    setStatus(`Export failed: ${error?.message || error}`);
  }
}

async function downloadPrivateKeyAction() {
  const wallet = getActiveWallet();
  if (!wallet) {
    setStatus('No active wallet');
    return;
  }

  try {
    const password = getSettingsPassword();
    const privateKeyHex = await decryptPrivateKey(wallet.encryptedKey, password);
    const content = `MoltWallet Private Key\nWallet: ${wallet.name}\nAddress: ${wallet.address}\nExported: ${new Date().toISOString()}\n\nPrivate Key (Hex):\n${privateKeyHex}\n`;
    downloadTextFile(`molt-wallet-private-key-${wallet.name}-${Date.now()}.txt`, content);
    setStatus('Private key file downloaded');
  } catch (error) {
    setStatus(`Download failed: ${error?.message || error}`);
  }
}

async function downloadMnemonicAction() {
  const wallet = getActiveWallet();
  if (!wallet || !wallet.encryptedMnemonic) {
    setStatus('No seed phrase stored for this wallet');
    return;
  }

  try {
    const password = getSettingsPassword();
    const mnemonic = await decryptPrivateKey(wallet.encryptedMnemonic, password);
    const content = `MoltWallet Seed Phrase\nWallet: ${wallet.name}\nAddress: ${wallet.address}\nExported: ${new Date().toISOString()}\n\nSeed Phrase (12 words):\n${mnemonic}\n`;
    downloadTextFile(`molt-wallet-seed-${wallet.name}-${Date.now()}.txt`, content);
    setStatus('Seed phrase file downloaded');
  } catch (error) {
    setStatus(`Download failed: ${error?.message || error}`);
  }
}

async function copyExportOutputAction() {
  const value = document.getElementById('settingsExportOutput').value;
  if (!value) {
    setStatus('Nothing to copy');
    return;
  }

  await navigator.clipboard.writeText(value);
  setStatus('Output copied');
}

async function loadAssets() {
  const wallet = getActiveWallet();
  const assetsList = document.getElementById('assetsList');
  if (!wallet || !assetsList) return;

  const endpoint = resolveRpcEndpoint(state.network?.selected || 'local-testnet');
  const rpc = new MoltChainRPC(endpoint);

  assetsList.innerHTML = '<div class="popup-status">Loading assets...</div>';

  try {
    const result = await rpc.getBalance(wallet.address);
    const raw = Number(result?.shells || result?.spendable || 0);
    const molt = raw / 1_000_000_000;
    const decimals = displayDecimals();

    assetsList.innerHTML = `
      <div class="popup-activity-item">
        <div class="popup-asset-icon">🦞</div>
        <div class="popup-asset-info">
          <strong>MOLT</strong>
          <span>Native token</span>
        </div>
        <div class="popup-asset-amount">
          <strong>${molt.toLocaleString(undefined, { maximumFractionDigits: decimals })}</strong>
          <span>MOLT</span>
        </div>
      </div>
    `;
  } catch (error) {
    assetsList.innerHTML = '<div class="popup-status">Failed to load assets</div>';
  }
}

async function loadActivity() {
  const wallet = getActiveWallet();
  const activityList = document.getElementById('activityList');
  if (!wallet || !activityList) return;

  const endpoint = resolveRpcEndpoint(state.network?.selected || 'local-testnet');
  const rpc = new MoltChainRPC(endpoint);

  activityList.innerHTML = '<div class="popup-status">Loading activity...</div>';

  try {
    const result = await rpc.getTransactionsByAddress(wallet.address, {
      limit: 12
    });

    const txs = result?.transactions || (Array.isArray(result) ? result : []);

    if (!txs.length) {
      activityList.innerHTML = '<div class="popup-status">No recent activity</div>';
      return;
    }

    activityList.innerHTML = txs.map((tx) => {
      const sig = tx.signature || tx.hash || tx.txid || 'unknown';
      const shortSig = `${String(sig).slice(0, 8)}...${String(sig).slice(-4)}`;
      const isSent = (tx.from === wallet.address);

      // 14 type mappings — aligned with wallet website
      const typeMap = {
        'Transfer': isSent ? 'Sent' : 'Received',
        'Airdrop': 'Airdrop',
        'Stake': 'Staked',
        'Unstake': 'Unstaked',
        'ClaimUnstake': 'Claimed Unstake',
        'RegisterEvmAddress': 'EVM Registration',
        'Contract': 'Contract Call',
        'CreateCollection': 'Created Collection',
        'MintNFT': 'Minted NFT',
        'TransferNFT': isSent ? 'Sent NFT' : 'Received NFT',
        'Reward': 'Reward',
        'GenesisTransfer': 'Genesis Transfer',
        'GenesisMint': 'Genesis Mint',
      };
      const type = typeMap[tx.type] || (isSent ? 'Sent' : 'Received');
      const amountVal = tx.amount_shells ? tx.amount_shells : (tx.amount || 0);
      const amt = (amountVal / 1_000_000_000).toLocaleString(undefined, { maximumFractionDigits: 4 });
      const ts = tx.timestamp ? new Date(tx.timestamp * 1000).toLocaleString() : '';

      // Icons & colors — aligned with wallet website
      let icon = isSent ? 'fa-arrow-up' : 'fa-arrow-down';
      let color = isSent ? '#ff6b35' : '#4ade80';
      let sign = isSent ? '-' : '+';

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

      const address = isSent ? (tx.to || '') : (tx.from || '');
      const displayAddr = address && address.length > 20 ? address.slice(0, 8) + '...' + address.slice(-4) : (address || '');

      // Fee display: show actual fee amount for 0-amount contract calls / EVM registration
      const isZeroAmount = Number(amountVal) === 0;
      const isFeeOnly = tx.type === 'RegisterEvmAddress' || (tx.type === 'Contract' && isZeroAmount);
      const feeShells = tx.fee_shells || tx.fee || 0;
      const feeAmt = (feeShells / 1_000_000_000).toLocaleString(undefined, { maximumFractionDigits: 4 });
      const amountStr = isFeeOnly ? `${feeAmt} MOLT` : `${sign}${amt} MOLT`;
      const feeTag = isFeeOnly ? '<span style="display:inline-block;margin-left:0.3rem;padding:0.05rem 0.35rem;border-radius:4px;font-size:0.6rem;background:rgba(245,158,11,0.15);color:#f59e0b;font-weight:600;vertical-align:middle;">FEE</span>' : '';

      return `
        <div class="popup-activity-item" style="cursor:pointer;" title="${sig}">
          <div style="display:flex;align-items:center;gap:8px;">
            <div style="width:28px;height:28px;border-radius:50%;background:${color}22;display:flex;align-items:center;justify-content:center;flex-shrink:0;">
              <i class="fas ${icon}" style="color:${color};font-size:0.75rem;"></i>
            </div>
            <div style="flex:1;min-width:0;">
              <div style="font-weight:600;font-size:0.85rem;">${type}${displayAddr ? `<span style="margin-left:0.35rem;font-size:0.7rem;opacity:0.5;">${displayAddr}</span>` : ''}</div>
              <div style="font-size:0.7rem;opacity:0.5;">${shortSig}</div>
            </div>
            <div style="text-align:right;">
              <div style="font-weight:600;font-size:0.85rem;color:${color};">${amountStr}${feeTag}</div>
              <div style="font-size:0.65rem;opacity:0.5;">${ts}</div>
            </div>
          </div>
        </div>
      `;
    }).join('');
  } catch (error) {
    activityList.innerHTML = '<div class="popup-status">Failed to load activity</div>';
  }
}

async function loadIdentityPanel() {
  const wallet = getActiveWallet();
  const container = document.getElementById('identityContent');
  if (!wallet || !container) return;

  container.innerHTML = '<div class="popup-status"><i class="fas fa-spinner fa-spin"></i> Loading MoltyID...</div>';

  const endpoint = resolveRpcEndpoint(state.network?.selected || 'local-testnet');
  const rpcClient = new MoltChainRPC(endpoint);

  try {
    const profile = await rpcClient.call('getMoltyIdProfile', [wallet.address]).catch(() => null);
    if (!profile || !profile.name) {
      container.innerHTML = `
        <div class="popup-empty-state" style="text-align:center;padding:1rem 0;">
          <div style="font-size:1.5rem;margin-bottom:0.5rem;"><i class="fas fa-fingerprint" style="color:var(--primary);"></i></div>
          <p style="font-weight:600;margin-bottom:0.25rem;">No MoltyID registered yet</p>
          <p style="font-size:0.78rem;color:var(--text-muted);margin-bottom:0.75rem;">Create your on-chain identity, claim a .molt name, and build reputation.</p>
          <button id="popupRegisterIdentity" class="btn btn-primary btn-small" style="padding:0.5rem 1.25rem;">
            <i class="fas fa-plus"></i> Register Identity
          </button>
        </div>
      `;
      document.getElementById('popupRegisterIdentity')?.addEventListener('click', () => {
        chrome.tabs.create({ url: chrome.runtime.getURL('src/pages/full.html') + '#identity' });
      });
      return;
    }
    const rep = Number(profile.reputation?.score || 0);
    const moltName = profile.molt_name;
    const tierName = profile.reputation?.tier_name || 'Newcomer';
    const skills = Array.isArray(profile.skills) ? profile.skills : [];
    const vouchesReceived = Array.isArray(profile.vouches?.received) ? profile.vouches.received : [];
    const achievements = Array.isArray(profile.achievements) ? profile.achievements : [];
    const repPct = Math.min(100, (rep / 10000) * 100);
    const isActive = profile.is_active !== false && profile.is_active !== 0;
    container.innerHTML = `
      <div style="text-align:center;padding:0.75rem 0;">
        <div style="font-size:1.5rem;"><i class="fas fa-fingerprint" style="color:var(--primary);"></i></div>
        <h4 style="margin:0.5rem 0 0.25rem;">${profile.name}${moltName ? ' <span style="color:var(--primary);">' + moltName + (moltName.endsWith('.molt') ? '' : '.molt') + '</span>' : ''}</h4>
        <div style="font-size:0.78rem;color:var(--text-muted);margin-bottom:0.25rem;">${tierName} · ${profile.agent_type_name || 'General'}${isActive ? ' · <span style="color:#4ade80;">Active</span>' : ''}</div>
        <div style="font-size:0.82rem;color:var(--text-muted);">Reputation: ${rep.toLocaleString()} / 10,000</div>
        <div style="margin-top:0.5rem;height:4px;background:var(--bg-tertiary);border-radius:2px;overflow:hidden;">
          <div style="height:100%;width:${repPct}%;background:var(--primary);border-radius:2px;"></div>
        </div>
        ${skills.length > 0 ? `<div style="font-size:0.75rem;color:var(--text-muted);margin-top:0.5rem;">Skills: ${skills.map(s => s.name).join(', ')}</div>` : ''}
        <div style="display:flex;justify-content:center;gap:1rem;margin-top:0.5rem;font-size:0.75rem;color:var(--text-muted);">
          <span><i class="fas fa-handshake"></i> ${vouchesReceived.length} vouches</span>
          <span><i class="fas fa-award"></i> ${achievements.length} achievements</span>
        </div>
        <button id="popupManageIdentity" class="btn btn-secondary btn-small" style="margin-top:0.75rem;font-size:0.75rem;">
          <i class="fas fa-external-link-alt"></i> Manage Identity
        </button>
      </div>
    `;
    document.getElementById('popupManageIdentity')?.addEventListener('click', () => {
      chrome.tabs.create({ url: chrome.runtime.getURL('src/pages/full.html') + '#identity' });
    });
  } catch {
    container.innerHTML = '<div class="popup-status">Failed to load identity</div>';
  }
}

async function refreshBalance() {
  const wallet = getActiveWallet();
  if (!wallet) {
    setStatus('No active wallet');
    return;
  }

  const endpoint = resolveRpcEndpoint(state.network?.selected || 'local-testnet');
  const rpc = new MoltChainRPC(endpoint);

  setStatus('Refreshing balance...');

  try {
    const result = await rpc.getBalance(wallet.address);
    const raw = Number(result?.shells || result?.spendable || 0);
    const balanceMolt = raw / 1_000_000_000;
    document.getElementById('walletBalance').textContent = `${balanceMolt.toLocaleString(undefined, { minimumFractionDigits: 2, maximumFractionDigits: 9 })} MOLT`;
    const usdEl = document.getElementById('balanceUsd');
    if (usdEl) usdEl.textContent = `$${(balanceMolt * 0.10).toLocaleString(undefined, { minimumFractionDigits: 2, maximumFractionDigits: 6 })} USD`;
    setStatus(`Connected: ${state.network?.selected || 'local-testnet'}`);
  } catch (error) {
    document.getElementById('walletBalance').textContent = '0.00 MOLT';
    setStatus('RPC unavailable (showing cached state)');
  }
}

async function handleSendNow() {
  const wallet = getActiveWallet();
  if (!wallet) return;

  const to = document.getElementById('sendTo').value.trim();
  const amount = Number(document.getElementById('sendAmount').value || 0);
  const password = document.getElementById('sendPassword').value;

  if (!isValidAddress(to)) {
    alert('Recipient must be a valid base58 wallet address.');
    return;
  }
  if (!amount || amount <= 0) {
    alert('Enter a valid amount.');
    return;
  }
  if (!password) {
    alert('Password is required to sign.');
    return;
  }

  const endpoint = resolveRpcEndpoint(state.network?.selected || 'local-testnet');
  const rpc = new MoltChainRPC(endpoint);

  try {
    setStatus('Checking balance...');

    const balResult = await rpc.getBalance(wallet.address);
    const spendable = Number(balResult?.spendable || balResult?.shells || 0) / 1_000_000_000;
    const totalNeeded = amount + 0.001;
    if (spendable < totalNeeded) {
      alert(`Insufficient MOLT: need ${totalNeeded.toLocaleString(undefined, { maximumFractionDigits: 9 })}, have ${spendable.toLocaleString(undefined, { maximumFractionDigits: 9 })}.`);
      return;
    }

    setStatus('Decrypting key...');
    const privateKeyHex = await decryptPrivateKey(wallet.encryptedKey, password);

    setStatus('Building transaction...');
    const latestBlock = await rpc.getLatestBlock();
    const transaction = await buildSignedNativeTransferTransaction({
      privateKeyHex,
      fromPublicKeyHex: wallet.publicKey,
      toAddress: to,
      amountMolt: amount,
      blockhash: latestBlock.hash
    });

    const txBase64 = encodeTransactionBase64(transaction);

    setStatus('Broadcasting...');
    const txSig = await rpc.sendTransaction(txBase64);

    await notify('MoltWallet', 'Transaction submitted successfully');
    setStatus(`Sent • ${String(txSig).slice(0, 12)}...`);
    document.getElementById('sendTo').value = '';
    document.getElementById('sendAmount').value = '';
    document.getElementById('sendPassword').value = '';
    await refreshBalance();
    await loadAssets();
    await loadActivity();
  } catch (error) {
    setStatus('Send failed');
    alert(`Transaction failed: ${error?.message || error}`);
  }
}

async function saveRpcSettings() {
  const mainnetRPC = document.getElementById('settingsMainnetRpc').value.trim();
  const testnetRPC = document.getElementById('settingsTestnetRpc').value.trim();
  const localTestnetRPC = document.getElementById('settingsLocalTestnetRpc').value.trim();
  const localMainnetRPC = document.getElementById('settingsLocalMainnetRpc').value.trim();
  const mainnetCustody = document.getElementById('settingsMainnetCustody').value.trim();
  const testnetCustody = document.getElementById('settingsTestnetCustody').value.trim();
  const localCustody = document.getElementById('settingsLocalCustody').value.trim();

  await persistAndRender({
    ...state,
    settings: {
      ...(state.settings || {}),
      mainnetRPC,
      testnetRPC,
      localTestnetRPC,
      localMainnetRPC,
      mainnetCustody,
      testnetCustody,
      localCustody
    }
  });

  setStatus('RPC/custody endpoints saved');
}

async function saveDisplaySettings() {
  const currency = document.getElementById('settingsCurrency').value;
  const decimals = Number(document.getElementById('settingsDecimals').value);

  await persistAndRender({
    ...state,
    settings: {
      ...(state.settings || {}),
      currency,
      decimals
    }
  });

  setStatus(`Display settings saved (${currency}, ${decimals} decimals)`);
}

async function changePasswordAction() {
  const wallet = getActiveWallet();
  if (!wallet) {
    setStatus('No active wallet');
    return;
  }

  const currentPassword = document.getElementById('settingsCurrentPassword').value;
  const newPassword = document.getElementById('settingsNewPassword').value;
  const confirmPassword = document.getElementById('settingsNewPasswordConfirm').value;

  if (!currentPassword) {
    setStatus('Current password is required');
    return;
  }
  if (!newPassword || newPassword.length < 8) {
    setStatus('New password must be at least 8 characters');
    return;
  }
  if (newPassword !== confirmPassword) {
    setStatus('New passwords do not match');
    return;
  }

  try {
    const privateKeyHex = await decryptPrivateKey(wallet.encryptedKey, currentPassword);
    const nextEncryptedKey = await encryptPrivateKey(privateKeyHex, newPassword);

    let nextEncryptedMnemonic = wallet.encryptedMnemonic || null;
    if (wallet.encryptedMnemonic) {
      const mnemonic = await decryptPrivateKey(wallet.encryptedMnemonic, currentPassword);
      nextEncryptedMnemonic = await encryptPrivateKey(mnemonic, newPassword);
    }

    const wallets = state.wallets.map((entry) => {
      if (entry.id !== wallet.id) return entry;
      return {
        ...entry,
        encryptedKey: nextEncryptedKey,
        encryptedMnemonic: nextEncryptedMnemonic
      };
    });

    await persistAndRender({
      ...state,
      wallets
    });

    document.getElementById('settingsCurrentPassword').value = '';
    document.getElementById('settingsNewPassword').value = '';
    document.getElementById('settingsNewPasswordConfirm').value = '';
    setStatus('Password updated and keys re-encrypted');
  } catch (error) {
    setStatus(`Password update failed: ${error?.message || error}`);
  }
}

async function persistAndRender(nextState) {
  state = nextState;
  await saveState(state);
  render();
}

async function createWalletFromMnemonic(mnemonic, password, walletName) {
  const keypair = await mnemonicToKeypair(mnemonic);
  const encryptedKey = await encryptPrivateKey(keypair.privateKey, password);
  const encryptedMnemonic = await encryptPrivateKey(mnemonic, password);

  const wallet = {
    id: generateId(),
    name: walletName,
    address: keypair.address,
    publicKey: keypair.publicKey,
    encryptedKey,
    encryptedMnemonic,
    hasMnemonic: true,
    createdAt: Date.now()
  };

  const wallets = [...state.wallets, wallet];
  await persistAndRender({
    ...state,
    wallets,
    activeWalletId: wallet.id,
    isLocked: false
  });

  // Register EVM address on-chain in background
  registerEvmAddress({ wallet, privateKeyHex: keypair.privateKey, network: state.network?.selected, settings: state.settings }).catch(() => {});
}

async function createWalletFromPrivateKeyHex(privateKeyHex, password, walletName) {
  const keypair = await privateKeyToKeypair(privateKeyHex);
  const encryptedKey = await encryptPrivateKey(keypair.privateKey, password);

  const wallet = {
    id: generateId(),
    name: walletName,
    address: keypair.address,
    publicKey: keypair.publicKey,
    encryptedKey,
    hasMnemonic: false,
    createdAt: Date.now()
  };

  const wallets = [...state.wallets, wallet];
  await persistAndRender({
    ...state,
    wallets,
    activeWalletId: wallet.id,
    isLocked: false
  });

  // Register EVM address on-chain in background
  registerEvmAddress({ wallet, privateKeyHex: keypair.privateKey, network: state.network?.selected, settings: state.settings }).catch(() => {});
}

function normalizePrivateKeyHex(privateKeyHex) {
  const normalized = String(privateKeyHex || '').trim().toLowerCase().replace(/^0x/, '');
  if (!/^[0-9a-f]+$/.test(normalized)) {
    throw new Error('Private key must be hex');
  }
  if (normalized.length !== 64 && normalized.length !== 128) {
    throw new Error('Private key must be 64 hex chars (or 128 hex chars secret key)');
  }

  if (normalized.length === 128) {
    return normalized.slice(0, 64);
  }

  return normalized;
}

function parseKeystoreToPrivateKeyHex(rawJson) {
  let data;
  try {
    data = JSON.parse(rawJson);
  } catch {
    throw new Error('Invalid JSON format');
  }

  if (typeof data?.privateKey === 'string') {
    return normalizePrivateKeyHex(data.privateKey);
  }

  if (Array.isArray(data?.privateKey)) {
    const bytes = Uint8Array.from(data.privateKey);
    if (bytes.length !== 32 && bytes.length !== 64) {
      throw new Error('privateKey array must be 32 or 64 bytes');
    }
    return bytesToHex(bytes.slice(0, 32));
  }

  if (Array.isArray(data?.secretKey)) {
    const bytes = Uint8Array.from(data.secretKey);
    if (bytes.length !== 64 && bytes.length !== 32) {
      throw new Error('secretKey array must be 64 or 32 bytes');
    }
    return bytesToHex(bytes.slice(0, 32));
  }

  throw new Error('Unsupported keystore format');
}

async function handleCreateStep1Continue() {
  const password = document.getElementById('createPassword').value.trim();
  const confirmPassword = document.getElementById('createPasswordConfirm').value.trim();

  if (password.length < 8) {
    alert('Password must be at least 8 characters.');
    return;
  }

  if (password !== confirmPassword) {
    alert('Passwords do not match.');
    return;
  }

  pendingGeneratedMnemonic = generateMnemonic();
  document.getElementById('createMnemonic').value = pendingGeneratedMnemonic;
  setCreateStep(2);
}

async function handleCreateStep2Continue() {
  if (!pendingGeneratedMnemonic) {
    alert('Recovery phrase not generated. Go back and try again.');
    return;
  }

  buildConfirmChallengeFromMnemonic(pendingGeneratedMnemonic);
  setCreateStep(3);
}

async function handleCreateFinish() {
  const password = document.getElementById('createPassword').value.trim();
  const confirmPassword = document.getElementById('createPasswordConfirm').value.trim();

  if (!pendingGeneratedMnemonic) {
    alert('Generate your recovery phrase first.');
    return;
  }

  if (password.length < 8 || password !== confirmPassword) {
    alert('Provide matching valid passwords.');
    return;
  }

  const exact = createWizardState.selectedWords.length
    && createWizardState.selectedWords.every((word, index) => word === createWizardState.mnemonicWords[index]);
  if (!exact) {
    alert('Seed phrase confirmation is incomplete or incorrect.');
    return;
  }

  const walletNumber = state.wallets.length + 1;
  await createWalletFromMnemonic(pendingGeneratedMnemonic, password, `Wallet ${walletNumber}`);
  startCreateFlow();
}

async function handleImportSave() {
  const importType = document.getElementById('importType').value;
  const mnemonic = document.getElementById('importMnemonic').value.trim().toLowerCase();
  const privateKeyRaw = document.getElementById('importPrivateKey').value.trim();
  const keystoreRaw = document.getElementById('importJson').value.trim();
  const password = document.getElementById('importPassword').value.trim();

  if (password.length < 8) {
    alert('Password must be at least 8 characters.');
    return;
  }

  const walletNumber = state.wallets.length + 1;

  if (importType === 'mnemonic') {
    if (!isValidMnemonic(mnemonic)) {
      alert('Invalid recovery phrase. Enter 12 valid words.');
      return;
    }
    await createWalletFromMnemonic(mnemonic, password, `Imported ${walletNumber}`);
    return;
  }

  if (importType === 'privateKey') {
    try {
      const privateKeyHex = normalizePrivateKeyHex(privateKeyRaw);
      await createWalletFromPrivateKeyHex(privateKeyHex, password, `Imported Key ${walletNumber}`);
      return;
    } catch (error) {
      alert(`Private key import failed: ${error?.message || error}`);
      return;
    }
  }

  if (importType === 'json') {
    try {
      const privateKeyHex = parseKeystoreToPrivateKeyHex(keystoreRaw);
      await createWalletFromPrivateKeyHex(privateKeyHex, password, `Imported JSON ${walletNumber}`);
      return;
    } catch (error) {
      alert(`JSON import failed: ${error?.message || error}`);
      return;
    }
  }

  alert('Unsupported import type');
}

function updateImportTypeUi() {
  const importType = document.getElementById('importType').value;
  const mnemonicRow = document.getElementById('importMnemonicRow');
  const privateKeyRow = document.getElementById('importPrivateKeyRow');
  const jsonRow = document.getElementById('importJsonRow');

  if (mnemonicRow) mnemonicRow.style.display = importType === 'mnemonic' ? 'block' : 'none';
  if (privateKeyRow) privateKeyRow.style.display = importType === 'privateKey' ? 'block' : 'none';
  if (jsonRow) jsonRow.style.display = importType === 'json' ? 'block' : 'none';

  document.querySelectorAll('.import-tab').forEach((tab) => {
    const isActive = tab.dataset.importType === importType;
    tab.classList.toggle('active', isActive);
  });
}

async function handleUnlock() {
  const wallet = getActiveWallet();
  if (!wallet) return;

  const password = document.getElementById('unlockPassword').value;
  try {
    await decryptPrivateKey(wallet.encryptedKey, password);
    await persistAndRender({
      ...state,
      isLocked: false
    });
    await scheduleAutoLock(state.settings?.lockTimeout || 300000);
    document.getElementById('unlockPassword').value = '';
  } catch (error) {
    alert('Invalid password.');
  }
}

async function handleLock() {
  await clearAutoLockAlarm();
  clearAllInputs();
  await persistAndRender({
    ...state,
    isLocked: true
  });
}

async function handleLogout() {
  if (!confirm('This will remove all wallets from this extension. Make sure you have your seed phrase backed up!')) return;
  await clearAutoLockAlarm();
  clearAllInputs();
  await chrome.storage.local.clear();
  state = { wallets: [], activeWalletId: null, isLocked: false, settings: { currency: 'USD', lockTimeout: 300000 }, network: { selected: 'local-testnet' } };
  showScreen('welcome');
}

function renderUnlock() {
  const wallet = getActiveWallet();
  document.getElementById('unlockWalletName').textContent = wallet
    ? `Unlock ${wallet.name} (${maskAddress(wallet.address)})`
    : 'No active wallet';
  showScreen('unlock');
}

async function renderDashboard() {
  const wallet = getActiveWallet();
  if (!wallet) {
    showScreen('welcome');
    return;
  }

  refreshSelector();
  document.getElementById('networkSelector').value = state.network?.selected || 'local-testnet';
  document.getElementById('settingsMainnetRpc').value = state.settings?.mainnetRPC || '';
  document.getElementById('settingsTestnetRpc').value = state.settings?.testnetRPC || '';
  document.getElementById('settingsLocalTestnetRpc').value = state.settings?.localTestnetRPC || '';
  document.getElementById('settingsLocalMainnetRpc').value = state.settings?.localMainnetRPC || '';
  document.getElementById('settingsMainnetCustody').value = state.settings?.mainnetCustody || '';
  document.getElementById('settingsTestnetCustody').value = state.settings?.testnetCustody || '';
  document.getElementById('settingsLocalCustody').value = state.settings?.localCustody || '';
  document.getElementById('settingsCurrency').value = state.settings?.currency || 'USD';
  document.getElementById('settingsDecimals').value = String(state.settings?.decimals ?? 6);
  document.getElementById('settingsLockTimeout').value = lockTimeoutMinutesFromMs(state.settings?.lockTimeout || 300000);
  document.getElementById('walletAddress').value = wallet.address;
  document.getElementById('receiveAddress').value = wallet.address;
  setDashboardPanel('assets');
  showScreen('dashboard');
  await refreshBalance();
  await loadAssets();
}

async function render() {
  if (state.wallets.length === 0) {
    showScreen('welcome');
    return;
  }

  if (state.isLocked) {
    renderUnlock();
    return;
  }

  await renderDashboard();
}

function wireEvents() {
  document.getElementById('openFullPage').addEventListener('click', () => {
    chrome.tabs.create({ url: chrome.runtime.getURL('src/pages/full.html') });
  });

  document.querySelectorAll('[data-action="goCreate"]').forEach((button) => {
    button.addEventListener('click', () => startCreateFlow());
  });
  document.querySelectorAll('[data-action="goImport"]').forEach((button) => {
    button.addEventListener('click', () => showScreen('import'));
  });

  document.getElementById('backFromCreate').addEventListener('click', (event) => {
    event.preventDefault();
    if (createWizardState.step > 1) {
      setCreateStep(createWizardState.step - 1);
      return;
    }
    showScreen('welcome');
  });

  document.getElementById('backFromImport').addEventListener('click', (event) => {
    event.preventDefault();
    showScreen('welcome');
  });

  document.getElementById('createStep1Continue').addEventListener('click', handleCreateStep1Continue);
  document.getElementById('createStep2Continue').addEventListener('click', handleCreateStep2Continue);
  document.getElementById('createFinish').addEventListener('click', handleCreateFinish);
  document.getElementById('importSave').addEventListener('click', handleImportSave);
  document.querySelectorAll('.import-tab').forEach((tab) => {
    tab.addEventListener('click', () => {
      const nextType = tab.dataset.importType;
      if (!nextType) return;
      document.getElementById('importType').value = nextType;
      updateImportTypeUi();
    });
  });
  document.getElementById('unlockSubmit').addEventListener('click', handleUnlock);
  document.getElementById('unlockLogoutBtn').addEventListener('click', handleLogout);
  document.getElementById('lockWallet').addEventListener('click', handleLock);
  document.getElementById('logoutWallet').addEventListener('click', handleLogout);
  document.getElementById('refreshBalance').addEventListener('click', refreshBalance);

  // Dashboard tab switching
  document.querySelectorAll('.popup-dash-tab').forEach((tab) => {
    tab.addEventListener('click', async () => {
      const tabName = tab.dataset.tab;
      setDashboardPanel(tabName);
      if (tabName === 'assets') await loadAssets();
      if (tabName === 'activity') await loadActivity();
      if (tabName === 'identity') await loadIdentityPanel();
    });
  });

  // Balance card action buttons
  document.getElementById('showSendPanel').addEventListener('click', () => setDashboardPanel('send'));
  document.getElementById('showReceivePanel').addEventListener('click', () => setDashboardPanel('receive'));
  document.getElementById('showDepositPanel').addEventListener('click', () => setDashboardPanel('receive'));

  // Nav-bar settings gear
  document.getElementById('showSettingsPanel').addEventListener('click', () => setDashboardPanel('settings'));

  document.getElementById('copyAddress').addEventListener('click', async () => {
    const wallet = getActiveWallet();
    if (!wallet) return;
    await navigator.clipboard.writeText(wallet.address);
    setStatus('Address copied');
  });

  document.getElementById('sendNow').addEventListener('click', handleSendNow);
  document.getElementById('saveLockTimeout').addEventListener('click', saveAutoLockSettings);
  document.getElementById('changePasswordBtn').addEventListener('click', changePasswordAction);
  document.getElementById('saveRpcSettings').addEventListener('click', saveRpcSettings);
  document.getElementById('saveDisplaySettings').addEventListener('click', saveDisplaySettings);
  document.getElementById('exportPrivateKey').addEventListener('click', exportPrivateKeyAction);
  document.getElementById('exportMnemonic').addEventListener('click', exportMnemonicAction);
  document.getElementById('exportKeystoreJson').addEventListener('click', exportKeystoreJsonAction);
  document.getElementById('downloadPrivateKey').addEventListener('click', downloadPrivateKeyAction);
  document.getElementById('downloadMnemonic').addEventListener('click', downloadMnemonicAction);
  document.getElementById('copyExportOutput').addEventListener('click', copyExportOutputAction);

  document.getElementById('walletSelector').addEventListener('change', async (event) => {
    await persistAndRender({
      ...state,
      activeWalletId: event.target.value
    });
  });

  document.getElementById('networkSelector').addEventListener('change', async (event) => {
    await persistAndRender({
      ...state,
      network: {
        ...(state.network || {}),
        selected: event.target.value
      }
    });
  });

  ['click', 'keydown', 'mousemove'].forEach((evt) => {
    document.addEventListener(evt, () => {
      if (!state?.isLocked) {
        scheduleAutoLock(state.settings?.lockTimeout || 300000);
      }
    });
  });
}

async function boot() {
  applyViewMode();
  initFullWelcomeCarousel();

  state = await loadState();
  if (!state.network) {
    state.network = { selected: 'local-testnet' };
  }
  wireEvents();
  updateImportTypeUi();
  if (!state.isLocked) {
    await scheduleAutoLock(state.settings?.lockTimeout || 300000);
  }
  await render();
}

boot();
