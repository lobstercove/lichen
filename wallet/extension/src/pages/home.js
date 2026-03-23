import { loadState } from '../core/state-store.js';
import { loadIdentitySnapshot, registerIdentity, addIdentitySkill } from '../core/identity-service.js';
import { loadStakingSnapshot, stakeLicn, unstakeStLicn } from '../core/staking-service.js';
import { loadBridgeSnapshot, requestBridgeDepositAddress, getBridgeDepositStatus } from '../core/bridge-service.js';
import { loadNftSnapshot } from '../core/nft-service.js';
import { isValidAddress } from '../core/crypto-service.js';

let latestState = null;
let activeDepositId = null;
let depositPollTimer = null;
const BRIDGE_CHAINS = ['solana', 'ethereum', 'bsc'];
const BRIDGE_ASSETS = ['usdc', 'usdt'];
const BRIDGE_STATUS_MAP = {
  issued: 'Waiting for deposit...',
  pending: 'Deposit detected, confirming...',
  confirmed: 'Deposit confirmed! Sweeping to treasury...',
  swept: 'Swept. Minting wrapped tokens on Lichen...',
  credited: 'Deposit credited on Lichen',
  expired: 'Deposit expired'
};

function shortAddress(address) {
  if (!address) return '—';
  if (address.length < 16) return address;
  return `${address.slice(0, 10)}...${address.slice(-8)}`;
}

// P0-FIX: HTML-escape to prevent XSS in innerHTML templates
function escapeHtml(value) {
  return String(value ?? '')
    .replaceAll('&', '&amp;')
    .replaceAll('<', '&lt;')
    .replaceAll('>', '&gt;')
    .replaceAll('"', '&quot;')
    .replaceAll("'", '&#39;');
}

// AUDIT-FIX FE-8: Secure password prompt using modal with masked input instead of prompt()
function securePasswordPrompt(label = 'Wallet password (for signing):') {
  return new Promise((resolve) => {
    const overlay = document.createElement('div');
    overlay.style.cssText = 'position:fixed;top:0;left:0;width:100%;height:100%;background:rgba(0,0,0,0.6);display:flex;align-items:center;justify-content:center;z-index:99999;';
    overlay.innerHTML = `
      <div style="background:var(--bg,#1a1b26);border:1px solid var(--border,#333);border-radius:12px;padding:1.5rem;width:340px;max-width:90vw;">
        <p style="margin:0 0 0.75rem;font-size:0.9rem;color:var(--text,#e0e0e0);">${escapeHtml(label)}</p>
        <input type="password" id="_secPwInput" placeholder="Enter password" autocomplete="off"
          style="width:100%;padding:0.6rem;border-radius:8px;border:1px solid var(--border,#444);background:var(--card-bg,#24253a);color:var(--text,#e0e0e0);box-sizing:border-box;margin-bottom:0.75rem;">
        <div style="display:flex;gap:0.5rem;">
          <button id="_secPwOk" style="flex:1;padding:0.5rem;border-radius:8px;border:none;background:var(--primary,#6C5CE7);color:#fff;cursor:pointer;">OK</button>
          <button id="_secPwCancel" style="flex:1;padding:0.5rem;border-radius:8px;border:1px solid var(--border,#444);background:transparent;color:var(--text,#e0e0e0);cursor:pointer;">Cancel</button>
        </div>
      </div>`;
    document.body.appendChild(overlay);
    const input = overlay.querySelector('#_secPwInput');
    input.focus();
    const finish = (val) => { overlay.remove(); resolve(val); };
    overlay.querySelector('#_secPwOk').addEventListener('click', () => finish(input.value || null));
    overlay.querySelector('#_secPwCancel').addEventListener('click', () => finish(null));
    input.addEventListener('keydown', (e) => { if (e.key === 'Enter') finish(input.value || null); });
  });
}

function setText(id, value) {
  const node = document.getElementById(id);
  if (node) {
    node.textContent = value;
  }
}

function setHtml(id, html) {
  const node = document.getElementById(id);
  if (node) {
    node.innerHTML = html;
  }
}

function setActionStatus(message) {
  setText('homeActionStatus', message);
}

function parsePositiveAmount(value, label) {
  const amount = Number(value);
  if (!Number.isFinite(amount) || amount <= 0) {
    throw new Error(`${label} must be a positive number`);
  }
  return amount;
}

function parseIntegerRange(value, label, min, max) {
  const parsed = Number(value);
  if (!Number.isInteger(parsed) || parsed < min || parsed > max) {
    throw new Error(`${label} must be an integer between ${min} and ${max}`);
  }
  return parsed;
}

async function loadSnapshots() {
  const state = await loadState();
  latestState = state;
  const activeWallet = state.wallets?.find((wallet) => wallet.id === state.activeWalletId) || null;

  setText('metaNetwork', state.network?.selected || 'local-testnet');
  setText('metaWallet', activeWallet?.name || 'No wallet');
  setText('metaAddress', shortAddress(activeWallet?.address || ''));

  if (!activeWallet) {
    const empty = 'No active wallet. Open popup to create or import.';
    setHtml('identitySnapshot', empty);
    setHtml('stakingSnapshot', empty);
    setHtml('bridgeSnapshot', empty);
    setHtml('nftSnapshot', empty);
    setHtml('wsSnapshot', 'No active wallet');
    return;
  }

  const network = state.network?.selected || 'local-testnet';

  const [identity, staking, bridge, nfts] = await Promise.all([
    loadIdentitySnapshot(activeWallet.address, network).catch(() => null),
    loadStakingSnapshot(activeWallet.address, network).catch(() => null),
    loadBridgeSnapshot(activeWallet.address, network).catch(() => null),
    loadNftSnapshot(activeWallet.address, network).catch(() => null)
  ]);

  if (!identity) {
    setHtml('identitySnapshot', 'Identity service unavailable');
  } else {
    setHtml('identitySnapshot', `
      <div><strong>Name:</strong> ${identity.name || 'Unregistered'}</div>
      <div><strong>Reputation:</strong> ${identity.reputation.toLocaleString()}</div>
      <div><strong>Skills:</strong> ${identity.skills}</div>
      <div><strong>Status:</strong> ${identity.active ? 'Active' : 'Inactive'}</div>
    `);
  }

  if (!staking) {
    setHtml('stakingSnapshot', 'Staking service unavailable');
  } else {
    setHtml('stakingSnapshot', `
      <div><strong>Staked:</strong> ${staking.staked.toFixed(4)} stLICN</div>
      <div><strong>Rewards:</strong> ${staking.rewards.toFixed(4)} LICN</div>
      <div><strong>Validator:</strong> ${staking.validator || '—'}</div>
    `);
  }

  if (!bridge) {
    setHtml('bridgeSnapshot', 'Bridge service unavailable');
  } else {
    setHtml('bridgeSnapshot', `
      <div><strong>Total Deposits:</strong> ${bridge.totalDeposits}</div>
      <div><strong>Pending:</strong> ${bridge.pending}</div>
    `);
  }

  if (!nfts) {
    setHtml('nftSnapshot', 'NFT service unavailable');
  } else {
    setHtml('nftSnapshot', `
      <div><strong>Total NFTs:</strong> ${nfts.count}</div>
      <div><strong>MTS-721:</strong> ${nfts.standards.mts721}</div>
      <div><strong>MTS-1155:</strong> ${nfts.standards.mts1155}</div>
    `);
  }

  await refreshWsStatus();
}

function clearDepositPolling() {
  if (depositPollTimer) {
    clearInterval(depositPollTimer);
    depositPollTimer = null;
  }
}

function renderDepositCard(data) {
  const card = document.getElementById('bridgeDepositCard');
  if (!card) return;
  const rawStatus = String(data.status || 'issued').toLowerCase();
  const displayStatus = BRIDGE_STATUS_MAP[rawStatus] || rawStatus || 'waiting';
  const safeAddress = escapeHtml(data.address || '—');
  const safeDepositId = escapeHtml(data.deposit_id || '—');
  const safeStatus = escapeHtml(displayStatus);
  card.style.display = 'block';
  card.innerHTML = `
    <div><strong>Deposit Address</strong></div>
    <div class="mono" style="margin: 0.35rem 0 0.45rem;" id="bridgeDepositAddress">${safeAddress}</div>
    <button class="btn btn-secondary btn-small" id="copyBridgeAddress" style="margin-bottom: 0.55rem;">Copy Address</button>
    <div><strong>Deposit ID:</strong> <span class="mono">${safeDepositId}</span></div>
    <div><strong>Status:</strong> <span id="bridgeDepositStatus">${safeStatus}</span></div>
  `;

  document.getElementById('copyBridgeAddress')?.addEventListener('click', () => {
    const value = String(data.address || '').trim();
    if (!value) {
      setActionStatus('Copy failed: no address available');
      return;
    }

    navigator.clipboard.writeText(value)
      .then(() => setActionStatus('Bridge deposit address copied'))
      .catch((error) => setActionStatus(`Copy failed: ${error?.message || error}`));
  });
}

async function requestDepositAddressAction() {
  const state = latestState || await loadState();
  const activeWallet = state.wallets?.find((wallet) => wallet.id === state.activeWalletId) || null;
  if (!activeWallet) {
    setActionStatus('Bridge request failed: no active wallet');
    alert('No active wallet');
    return;
  }
  if (!isValidAddress(activeWallet.address)) {
    setActionStatus('Bridge request failed: active wallet address is invalid');
    return;
  }

  const chain = document.getElementById('bridgeChain').value;
  const asset = document.getElementById('bridgeAsset').value;
  if (!BRIDGE_CHAINS.includes(chain)) {
    setActionStatus('Bridge request failed: invalid chain selection');
    return;
  }
  if (!BRIDGE_ASSETS.includes(asset)) {
    setActionStatus('Bridge request failed: invalid asset selection');
    return;
  }

  try {
    setActionStatus(`Requesting ${asset.toUpperCase()} deposit address on ${chain}...`);
    const wsStatus = await chrome.runtime.sendMessage({ type: 'LICHEN_WS_STATUS' }).catch(() => null);
    const hasWsSubscription = Boolean(wsStatus?.ok && wsStatus?.result?.subscribed);
    const pollIntervalMs = hasWsSubscription ? 30000 : 5000;

    const response = await requestBridgeDepositAddress({
      userAddress: activeWallet.address,
      chain,
      asset,
      network: state.network?.selected || 'local-testnet'
    });

    activeDepositId = response.deposit_id;
    renderDepositCard(response);
    setActionStatus(`Deposit address ready (${String(response.deposit_id || '').slice(0, 12)}...)`);

    clearDepositPolling();
    depositPollTimer = setInterval(async () => {
      if (!activeDepositId) return;
      try {
        const statusResult = await getBridgeDepositStatus(activeDepositId, state.network?.selected || 'local-testnet');
        const statusEl = document.getElementById('bridgeDepositStatus');
        const statusValue = String(statusResult.status || statusResult.state || 'unknown').toLowerCase();
        const statusLabel = BRIDGE_STATUS_MAP[statusValue] || statusValue;
        if (statusEl) {
          statusEl.textContent = statusLabel;
        }

        if (statusValue === 'credited' || statusValue === 'expired') {
          clearDepositPolling();
          setActionStatus(`Bridge deposit status: ${statusLabel}`);
        }
      } catch {
        // keep polling, ignore transient errors
      }
    }, pollIntervalMs);

    setActionStatus(`Deposit address ready (${String(response.deposit_id || '').slice(0, 12)}...) • polling ${pollIntervalMs / 1000}s`);
  } catch (error) {
    setActionStatus(`Bridge request failed: ${error?.message || error}`);
    alert(`Bridge request failed: ${error?.message || error}`);
  }
}

async function refreshWsStatus() {
  const result = await chrome.runtime.sendMessage({ type: 'LICHEN_WS_STATUS' }).catch(() => null);
  if (!result?.ok) {
    setHtml('wsSnapshot', 'Unavailable');
    return;
  }

  const ws = result.result;
  setHtml('wsSnapshot', `
    <div><strong>State:</strong> ${ws.state}</div>
    <div><strong>Network:</strong> ${ws.network || '—'}</div>
    <div><strong>Address:</strong> ${shortAddress(ws.address || '')}</div>
    <div><strong>Subscribed:</strong> ${ws.subscribed ? 'Yes' : 'No'}</div>
  `);
}

async function withActiveWalletAction(run) {
  const state = latestState || await loadState();
  const activeWallet = state.wallets?.find((wallet) => wallet.id === state.activeWalletId) || null;
  if (!activeWallet) {
    alert('No active wallet');
    return;
  }

  const network = state.network?.selected || 'local-testnet';
  await run({ state, activeWallet, network });
}

async function handleStake() {
  await withActiveWalletAction(async ({ activeWallet, network }) => {
    const amountText = prompt('Stake amount (LICN):', '1');
    if (!amountText) return;
    let amount;
    try {
      amount = parsePositiveAmount(amountText, 'Stake amount');
    } catch (error) {
      setActionStatus(`Stake failed: ${error.message}`);
      alert(error.message);
      return;
    }

    const password = await securePasswordPrompt();
    if (!password) return;

    try {
      setActionStatus(`Submitting stake of ${amount} LICN...`);
      const result = await stakeLicn({ wallet: activeWallet, password, amountLicn: amount, network });
      setActionStatus(`Stake submitted: ${String(result.txHash).slice(0, 20)}...`);
      alert(`Stake submitted: ${String(result.txHash).slice(0, 20)}...`);
      await loadSnapshots();
    } catch (error) {
      setActionStatus(`Stake failed: ${error?.message || error}`);
      alert(`Stake failed: ${error?.message || error}`);
    }
  });
}

async function handleUnstake() {
  await withActiveWalletAction(async ({ activeWallet, network }) => {
    const amountText = prompt('Unstake amount (stLICN):', '1');
    if (!amountText) return;
    let amount;
    try {
      amount = parsePositiveAmount(amountText, 'Unstake amount');
    } catch (error) {
      setActionStatus(`Unstake failed: ${error.message}`);
      alert(error.message);
      return;
    }

    const password = await securePasswordPrompt();
    if (!password) return;

    try {
      setActionStatus(`Submitting unstake of ${amount} stLICN...`);
      const result = await unstakeStLicn({ wallet: activeWallet, password, amountLicn: amount, network });
      setActionStatus(`Unstake submitted: ${String(result.txHash).slice(0, 20)}...`);
      alert(`Unstake submitted: ${String(result.txHash).slice(0, 20)}...`);
      await loadSnapshots();
    } catch (error) {
      setActionStatus(`Unstake failed: ${error?.message || error}`);
      alert(`Unstake failed: ${error?.message || error}`);
    }
  });
}

async function handleRegisterIdentity() {
  await withActiveWalletAction(async ({ activeWallet, network }) => {
    const displayName = prompt('Display name (LichenID):', activeWallet.name || 'LicnUser');
    if (!displayName) return;
    if (displayName.trim().length < 2 || displayName.trim().length > 32) {
      setActionStatus('Identity registration failed: name must be 2-32 chars');
      return;
    }
    const agentTypeText = prompt('Agent type (0-9):', '9');
    let agentType;
    try {
      agentType = parseIntegerRange(agentTypeText || '9', 'Agent type', 0, 9);
    } catch (error) {
      setActionStatus(`Identity registration failed: ${error.message}`);
      return;
    }
    const password = await securePasswordPrompt();
    if (!password) return;

    try {
      setActionStatus('Submitting identity registration...');
      const result = await registerIdentity({
        wallet: activeWallet,
        password,
        network,
        displayName,
        agentType
      });
      setActionStatus(`Identity registration submitted: ${String(result.txHash).slice(0, 20)}...`);
      alert(`Identity registration submitted: ${String(result.txHash).slice(0, 20)}...`);
      await loadSnapshots();
    } catch (error) {
      setActionStatus(`Identity registration failed: ${error?.message || error}`);
      alert(`Registration failed: ${error?.message || error}`);
    }
  });
}

async function handleAddSkill() {
  await withActiveWalletAction(async ({ activeWallet, network }) => {
    const skillName = prompt('Skill name:', 'Rust');
    if (!skillName) return;
    if (skillName.trim().length < 2 || skillName.trim().length > 32) {
      setActionStatus('Add skill failed: skill name must be 2-32 chars');
      return;
    }
    const proficiencyText = prompt('Proficiency (1-100):', '50');
    let proficiency;
    try {
      proficiency = parseIntegerRange(proficiencyText || '50', 'Proficiency', 1, 100);
    } catch (error) {
      setActionStatus(`Add skill failed: ${error.message}`);
      return;
    }
    const password = await securePasswordPrompt();
    if (!password) return;

    try {
      setActionStatus(`Submitting skill ${skillName.trim()}...`);
      const result = await addIdentitySkill({
        wallet: activeWallet,
        password,
        network,
        skillName,
        proficiency
      });
      setActionStatus(`Skill tx submitted: ${String(result.txHash).slice(0, 20)}...`);
      alert(`Skill tx submitted: ${String(result.txHash).slice(0, 20)}...`);
      await loadSnapshots();
    } catch (error) {
      setActionStatus(`Add skill failed: ${error?.message || error}`);
      alert(`Add skill failed: ${error?.message || error}`);
    }
  });
}

document.getElementById('refreshAll')?.addEventListener('click', loadSnapshots);
document.getElementById('requestBridgeAddress')?.addEventListener('click', requestDepositAddressAction);
document.getElementById('refreshWs')?.addEventListener('click', async () => {
  await chrome.runtime.sendMessage({ type: 'LICHEN_WS_SYNC' }).catch(() => null);
  await refreshWsStatus();
});
document.getElementById('openApproveQueue')?.addEventListener('click', () => {
  chrome.tabs.create({ url: chrome.runtime.getURL('src/pages/approve.html') });
});
document.getElementById('openIdentityDetail')?.addEventListener('click', () => {
  location.href = chrome.runtime.getURL('src/pages/identity.html');
});
document.getElementById('openNftDetail')?.addEventListener('click', () => {
  location.href = chrome.runtime.getURL('src/pages/nfts.html');
});
document.getElementById('openSettingsDetail')?.addEventListener('click', () => {
  location.href = chrome.runtime.getURL('src/pages/settings.html');
});
document.getElementById('openPopupHint')?.addEventListener('click', () => {
  setActionStatus('Use browser toolbar icon to access popup controls');
  alert('Use the extension toolbar icon to open the popup wallet controls.');
});
document.getElementById('stakeBtn')?.addEventListener('click', handleStake);
document.getElementById('unstakeBtn')?.addEventListener('click', handleUnstake);
document.getElementById('registerIdentityBtn')?.addEventListener('click', handleRegisterIdentity);
document.getElementById('addSkillBtn')?.addEventListener('click', handleAddSkill);

loadSnapshots();
setActionStatus('Ready');

window.addEventListener('beforeunload', () => {
  clearDepositPolling();
});
