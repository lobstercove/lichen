import { loadState } from '../core/state-store.js';
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
import { isValidAddress } from '../core/crypto-service.js';

let latestState = null;

function setStatus(message) {
  const node = document.getElementById('identityActionStatus');
  if (node) node.textContent = message;
}

function setHtml(id, html) {
  const node = document.getElementById(id);
  if (node) node.innerHTML = html;
}

function shortAddress(address) {
  if (!address) return '—';
  return address.length > 18 ? `${address.slice(0, 10)}...${address.slice(-8)}` : address;
}

function parseIntegerRange(value, label, min, max) {
  const parsed = Number(value);
  if (!Number.isInteger(parsed) || parsed < min || parsed > max) {
    throw new Error(`${label} must be an integer between ${min} and ${max}`);
  }
  return parsed;
}

function parseNonNegativeNumber(value, label) {
  const parsed = Number(value);
  if (!Number.isFinite(parsed) || parsed < 0) {
    throw new Error(`${label} must be a non-negative number`);
  }
  return parsed;
}

function isHttpUrl(url) {
  try {
    const parsed = new URL(String(url));
    return parsed.protocol === 'http:' || parsed.protocol === 'https:';
  } catch {
    return false;
  }
}

function normalizeMoltName(input) {
  return String(input || '').trim().toLowerCase().replace(/\.molt$/, '');
}

function validateMoltNameOrThrow(input) {
  const normalized = normalizeMoltName(input);
  if (!normalized) {
    throw new Error('Name is required');
  }
  if (normalized.length < 3 || normalized.length > 32) {
    throw new Error('Name must be 3-32 characters');
  }
  if (!/^[a-z0-9][a-z0-9-]*[a-z0-9]$/.test(normalized)) {
    throw new Error('Name must use a-z, 0-9, hyphen and cannot start/end with hyphen');
  }
  return normalized;
}

async function loadIdentityPage() {
  const state = await loadState();
  latestState = state;
  const wallet = state.wallets?.find((w) => w.id === state.activeWalletId) || null;
  if (!wallet) {
    setHtml('identityMeta', 'No active wallet. Open popup to create/import wallet.');
    return;
  }

  const network = state.network?.selected || 'local-testnet';
  setHtml('identityMeta', `<strong>${wallet.name}</strong> • ${network} • ${shortAddress(wallet.address)}`);

  const details = await loadIdentityDetails(wallet.address, network).catch(() => null);
  if (!details) {
    setHtml('identityProfile', '<div class="id-list">Identity service unavailable.</div>');
    setHtml('identitySkills', '<div class="id-list">—</div>');
    setHtml('identityVouches', '<div class="id-list">—</div>');
    setHtml('identityAchievements', '<div class="id-list">—</div>');
    return;
  }

  setHtml('identityProfile', `
    <div class="id-list">
      <div class="id-row"><strong>Name:</strong> ${details.name || 'Unregistered'}</div>
      <div class="id-row"><strong>Reputation:</strong> ${details.reputation.toLocaleString()}</div>
      <div class="id-row"><strong>Agent Type:</strong> ${details.agentType ?? '—'}</div>
      <div class="id-row"><strong>Status:</strong> ${details.active ? 'Active' : 'Inactive'}</div>
      <div class="id-row"><strong>Endpoint:</strong> ${details.endpoint || '—'}</div>
      <div class="id-row"><strong>Availability:</strong> ${details.availability}</div>
      <div class="id-row"><strong>Rate:</strong> ${details.rate.toFixed(6)} MOLT</div>
    </div>
  `);

  const skills = details.skills.length
    ? details.skills.map((s) => `<div class="id-row">${s.name || 'Skill'} — ${s.proficiency || 0}</div>`).join('')
    : '<div class="id-row">No skills yet</div>';
  setHtml('identitySkills', `<div class="id-list">${skills}</div>`);

  const vouches = `
    <div class="id-row"><strong>Received:</strong> ${details.vouchesReceived.length}</div>
    <div class="id-row"><strong>Given:</strong> ${details.vouchesGiven.length}</div>
  `;
  setHtml('identityVouches', `<div class="id-list">${vouches}</div>`);

  const achievements = details.achievements.length
    ? details.achievements.map((a) => `<div class="id-row">${a.name || a.id || 'Achievement'}</div>`).join('')
    : '<div class="id-row">No achievements yet</div>';
  setHtml('identityAchievements', `<div class="id-list">${achievements}</div>`);
}

async function withWalletAction(run) {
  const state = latestState || await loadState();
  const wallet = state.wallets?.find((w) => w.id === state.activeWalletId) || null;
  if (!wallet) {
    alert('No active wallet');
    return;
  }

  const network = state.network?.selected || 'local-testnet';
  await run({ wallet, network });
}

async function runAction(label, fn) {
  setStatus(`${label}...`);
  try {
    const result = await fn();
    setStatus(`${label} submitted: ${String(result?.txHash || 'ok').slice(0, 24)}...`);
    await loadIdentityPage();
  } catch (error) {
    setStatus(`${label} failed: ${error?.message || error}`);
  }
}

async function actionRegister() {
  await withWalletAction(async ({ wallet, network }) => {
    const displayName = prompt('Display name:', wallet.name || 'MoltUser');
    if (!displayName) return;
    if (displayName.trim().length < 2 || displayName.trim().length > 32) {
      setStatus('Register identity failed: name must be 2-32 chars');
      return;
    }
    let agentType;
    try {
      agentType = parseIntegerRange(prompt('Agent type (0-9):', '9') || '9', 'Agent type', 0, 9);
    } catch (error) {
      setStatus(`Register identity failed: ${error.message}`);
      return;
    }
    const password = prompt('Wallet password:', '');
    if (!password) return;

    await runAction('Register identity', () => registerIdentity({
      wallet,
      password,
      network,
      displayName: displayName.trim(),
      agentType
    }));
  });
}

async function actionAddSkill() {
  await withWalletAction(async ({ wallet, network }) => {
    const skillName = prompt('Skill name:', 'Rust');
    if (!skillName) return;
    if (skillName.trim().length < 2 || skillName.trim().length > 32) {
      setStatus('Add skill failed: skill name must be 2-32 chars');
      return;
    }
    let proficiency;
    try {
      proficiency = parseIntegerRange(prompt('Proficiency (1-100):', '50') || '50', 'Proficiency', 1, 100);
    } catch (error) {
      setStatus(`Add skill failed: ${error.message}`);
      return;
    }
    const password = prompt('Wallet password:', '');
    if (!password) return;

    await runAction('Add skill', () => addIdentitySkill({
      wallet,
      password,
      network,
      skillName: skillName.trim(),
      proficiency
    }));
  });
}

async function actionUpdateAgentType() {
  await withWalletAction(async ({ wallet, network }) => {
    let agentType;
    try {
      agentType = parseIntegerRange(prompt('New agent type (0-9):', '9') || '9', 'Agent type', 0, 9);
    } catch (error) {
      setStatus(`Update agent type failed: ${error.message}`);
      return;
    }
    const password = prompt('Wallet password:', '');
    if (!password) return;

    await runAction('Update agent type', () => updateIdentityAgentType({
      wallet,
      password,
      network,
      agentType
    }));
  });
}

async function actionVouch() {
  await withWalletAction(async ({ wallet, network }) => {
    const vouchee = prompt('Address to vouch for:', '');
    if (!vouchee) return;
    if (!isValidAddress(vouchee.trim())) {
      setStatus('Vouch failed: invalid recipient address');
      return;
    }
    const password = prompt('Wallet password:', '');
    if (!password) return;

    await runAction('Vouch', () => vouchForIdentity({
      wallet,
      password,
      network,
      vouchee: vouchee.trim()
    }));
  });
}

async function actionSetEndpoint() {
  await withWalletAction(async ({ wallet, network }) => {
    const endpoint = prompt('Agent endpoint URL:', 'https://');
    if (endpoint === null) return;
    const normalized = endpoint.trim();
    if (normalized && !isHttpUrl(normalized)) {
      setStatus('Set endpoint failed: must be a valid http(s) URL');
      return;
    }
    const password = prompt('Wallet password:', '');
    if (!password) return;

    await runAction('Set endpoint', () => setIdentityEndpoint({
      wallet,
      password,
      network,
      endpoint: normalized
    }));
  });
}

async function actionSetAvailability() {
  await withWalletAction(async ({ wallet, network }) => {
    const value = prompt('Availability (online/offline):', 'online');
    if (!value) return;
    const normalized = value.trim().toLowerCase();
    if (normalized !== 'online' && normalized !== 'offline') {
      setStatus('Set availability failed: use online or offline');
      return;
    }
    const online = normalized === 'online';
    const password = prompt('Wallet password:', '');
    if (!password) return;

    await runAction('Set availability', () => setIdentityAvailability({
      wallet,
      password,
      network,
      online
    }));
  });
}

async function actionSetRate() {
  await withWalletAction(async ({ wallet, network }) => {
    let rateMolt;
    try {
      rateMolt = parseNonNegativeNumber(prompt('Rate (MOLT per request):', '0.001') || '0.001', 'Rate');
    } catch (error) {
      setStatus(`Set rate failed: ${error.message}`);
      return;
    }
    const password = prompt('Wallet password:', '');
    if (!password) return;

    await runAction('Set rate', () => setIdentityRate({
      wallet,
      password,
      network,
      rateMolt
    }));
  });
}

async function actionRegisterName() {
  await withWalletAction(async ({ wallet, network }) => {
    const name = prompt('Name to register (without .molt):', 'myname');
    if (!name) return;

    let normalizedName;
    try {
      normalizedName = validateMoltNameOrThrow(name);
      if (normalizedName.length <= 4) {
        throw new Error('3-4 char names are auction-only');
      }
    } catch (error) {
      setStatus(`Register .molt failed: ${error.message}`);
      return;
    }

    let durationYears;
    try {
      durationYears = parseIntegerRange(prompt('Duration in years (1-10):', '1') || '1', 'Duration', 1, 10);
    } catch (error) {
      setStatus(`Register .molt failed: ${error.message}`);
      return;
    }
    const password = prompt('Wallet password:', '');
    if (!password) return;

    await runAction('Register .molt', () => registerMoltName({
      wallet,
      password,
      network,
      name: normalizedName,
      durationYears
    }));
  });
}

async function actionRenewName() {
  await withWalletAction(async ({ wallet, network }) => {
    const name = prompt('Name to renew (without .molt):', 'myname');
    if (!name) return;

    let normalizedName;
    try {
      normalizedName = validateMoltNameOrThrow(name);
    } catch (error) {
      setStatus(`Renew .molt failed: ${error.message}`);
      return;
    }

    let additionalYears;
    try {
      additionalYears = parseIntegerRange(prompt('Additional years (1-10):', '1') || '1', 'Additional years', 1, 10);
    } catch (error) {
      setStatus(`Renew .molt failed: ${error.message}`);
      return;
    }
    const password = prompt('Wallet password:', '');
    if (!password) return;

    await runAction('Renew .molt', () => renewMoltName({
      wallet,
      password,
      network,
      name: normalizedName,
      additionalYears
    }));
  });
}

async function actionTransferName() {
  await withWalletAction(async ({ wallet, network }) => {
    const name = prompt('Name to transfer (without .molt):', 'myname');
    if (!name) return;

    let normalizedName;
    try {
      normalizedName = validateMoltNameOrThrow(name);
    } catch (error) {
      setStatus(`Transfer .molt failed: ${error.message}`);
      return;
    }

    const recipient = prompt('Recipient address:', '');
    if (!recipient) return;
    if (!isValidAddress(recipient.trim())) {
      setStatus('Transfer .molt failed: invalid recipient address');
      return;
    }
    const password = prompt('Wallet password:', '');
    if (!password) return;

    await runAction('Transfer .molt', () => transferMoltName({
      wallet,
      password,
      network,
      name: normalizedName,
      recipient: recipient.trim()
    }));
  });
}

async function actionReleaseName() {
  await withWalletAction(async ({ wallet, network }) => {
    const name = prompt('Name to release (without .molt):', 'myname');
    if (!name) return;

    let normalizedName;
    try {
      normalizedName = validateMoltNameOrThrow(name);
    } catch (error) {
      setStatus(`Release .molt failed: ${error.message}`);
      return;
    }

    const confirmRelease = prompt('Type RELEASE to confirm:', '');
    if (confirmRelease !== 'RELEASE') {
      setStatus('Release cancelled');
      return;
    }
    const password = prompt('Wallet password:', '');
    if (!password) return;

    await runAction('Release .molt', () => releaseMoltName({
      wallet,
      password,
      network,
      name: normalizedName
    }));
  });
}

document.getElementById('refreshIdentity')?.addEventListener('click', loadIdentityPage);
document.getElementById('backHome')?.addEventListener('click', () => {
  location.href = chrome.runtime.getURL('src/pages/home.html');
});
document.getElementById('actionRegister')?.addEventListener('click', actionRegister);
document.getElementById('actionAddSkill')?.addEventListener('click', actionAddSkill);
document.getElementById('actionUpdateAgentType')?.addEventListener('click', actionUpdateAgentType);
document.getElementById('actionVouch')?.addEventListener('click', actionVouch);
document.getElementById('actionSetEndpoint')?.addEventListener('click', actionSetEndpoint);
document.getElementById('actionSetAvailability')?.addEventListener('click', actionSetAvailability);
document.getElementById('actionSetRate')?.addEventListener('click', actionSetRate);
document.getElementById('actionRegisterName')?.addEventListener('click', actionRegisterName);
document.getElementById('actionRenewName')?.addEventListener('click', actionRenewName);
document.getElementById('actionTransferName')?.addEventListener('click', actionTransferName);
document.getElementById('actionReleaseName')?.addEventListener('click', actionReleaseName);

loadIdentityPage();
