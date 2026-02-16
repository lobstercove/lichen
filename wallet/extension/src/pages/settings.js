import { DEFAULT_STATE, loadState, saveState } from '../core/state-store.js';
import { clearAutoLockAlarm, scheduleAutoLock } from '../core/lock-service.js';
import { decryptPrivateKey, encryptPrivateKey, hexToBytes } from '../core/crypto-service.js';

let state = null;

function setStatus(message) {
  const node = document.getElementById('settingsStatus');
  if (node) node.textContent = message;
}

function shortAddress(address) {
  if (!address) return '—';
  return address.length > 18 ? `${address.slice(0, 10)}...${address.slice(-8)}` : address;
}

function getActiveWallet() {
  return state.wallets?.find((wallet) => wallet.id === state.activeWalletId) || null;
}

function lockTimeoutMinutesFromMs(ms) {
  const parsed = Number(ms);
  if (!Number.isFinite(parsed) || parsed <= 0) return '5';
  return String(Math.round(parsed / 60000));
}

function lockTimeoutMsFromMinutes(minutes) {
  const parsed = Number(minutes);
  if (!Number.isFinite(parsed) || parsed < 0) return 300000;
  if (parsed === 0) return 0;
  return parsed * 60 * 1000;
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

function setOriginsHtml(html) {
  const node = document.getElementById('approvedOriginsList');
  if (node) node.innerHTML = html;
}

function requireExportPassword() {
  const password = document.getElementById('settingsExportPassword').value;
  if (!password) {
    throw new Error('Password is required for exports');
  }
  return password;
}

function renderMeta() {
  const wallet = getActiveWallet();
  const meta = document.getElementById('settingsMeta');
  meta.textContent = wallet
    ? `${wallet.name} • ${state.network?.selected || 'local-testnet'} • ${shortAddress(wallet.address)}`
    : 'No active wallet';
}

function renderWalletSelector() {
  const selector = document.getElementById('settingsWalletSelector');
  selector.innerHTML = '';
  state.wallets.forEach((wallet) => {
    const option = document.createElement('option');
    option.value = wallet.id;
    option.textContent = wallet.name;
    if (wallet.id === state.activeWalletId) option.selected = true;
    selector.appendChild(option);
  });
}

function renderControls() {
  document.getElementById('settingsNetwork').value = state.network?.selected || 'local-testnet';
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
}

async function refresh() {
  state = await loadState();
  renderMeta();
  renderWalletSelector();
  renderControls();
  await loadApprovedOrigins();
}

async function persist(nextState) {
  state = nextState;
  await saveState(state);
  await refresh();
}

async function onSwitchWallet(event) {
  const walletId = event.target.value;
  await persist({ ...state, activeWalletId: walletId });
  setStatus('Active wallet updated');
}

async function onRenameWallet() {
  const wallet = getActiveWallet();
  if (!wallet) {
    setStatus('No active wallet');
    return;
  }

  const nextName = prompt('New wallet name:', wallet.name);
  if (!nextName) return;
  if (nextName.trim().length < 2 || nextName.trim().length > 32) {
    setStatus('Name must be 2-32 chars');
    return;
  }

  const wallets = state.wallets.map((entry) => entry.id === wallet.id
    ? { ...entry, name: nextName.trim() }
    : entry);
  await persist({ ...state, wallets });
  setStatus('Wallet renamed');
}

async function onRemoveWallet() {
  const wallet = getActiveWallet();
  if (!wallet) {
    setStatus('No active wallet');
    return;
  }

  const confirmDelete = prompt(`Type DELETE to remove ${wallet.name}:`, '');
  if (confirmDelete !== 'DELETE') {
    setStatus('Wallet removal cancelled');
    return;
  }

  const wallets = state.wallets.filter((entry) => entry.id !== wallet.id);
  if (wallets.length === 0) {
    await persist({ ...DEFAULT_STATE });
    await clearAutoLockAlarm();
    setStatus('Wallet removed; extension reset');
    return;
  }

  await persist({
    ...state,
    wallets,
    activeWalletId: wallets[0].id,
    isLocked: true
  });
  setStatus('Wallet removed and extension locked');
}

async function onLockNow() {
  await clearAutoLockAlarm();
  await persist({ ...state, isLocked: true });
  setStatus('Wallet locked');
}

async function onSaveNetwork() {
  const selected = document.getElementById('settingsNetwork').value;
  const mainnetRPC = document.getElementById('settingsMainnetRpc').value.trim();
  const testnetRPC = document.getElementById('settingsTestnetRpc').value.trim();
  const localTestnetRPC = document.getElementById('settingsLocalTestnetRpc').value.trim();
  const localMainnetRPC = document.getElementById('settingsLocalMainnetRpc').value.trim();
  const mainnetCustody = document.getElementById('settingsMainnetCustody').value.trim();
  const testnetCustody = document.getElementById('settingsTestnetCustody').value.trim();
  const localCustody = document.getElementById('settingsLocalCustody').value.trim();

  await persist({
    ...state,
    network: {
      ...(state.network || {}),
      selected
    },
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

  setStatus('Network settings saved');
}

async function onSaveDisplaySecurity() {
  const currency = document.getElementById('settingsCurrency').value;
  const decimals = Number(document.getElementById('settingsDecimals').value);
  const lockTimeout = lockTimeoutMsFromMinutes(document.getElementById('settingsLockTimeout').value);

  await persist({
    ...state,
    settings: {
      ...(state.settings || {}),
      currency,
      decimals,
      lockTimeout
    }
  });

  if (state.isLocked || lockTimeout === 0) {
    await clearAutoLockAlarm();
  } else {
    await scheduleAutoLock(lockTimeout);
  }

  setStatus('Display/security settings saved');
}

async function onChangePassword() {
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
    const encryptedKey = await encryptPrivateKey(privateKeyHex, newPassword);

    let encryptedMnemonic = wallet.encryptedMnemonic || null;
    if (wallet.encryptedMnemonic) {
      const mnemonic = await decryptPrivateKey(wallet.encryptedMnemonic, currentPassword);
      encryptedMnemonic = await encryptPrivateKey(mnemonic, newPassword);
    }

    const wallets = state.wallets.map((entry) => entry.id === wallet.id
      ? { ...entry, encryptedKey, encryptedMnemonic }
      : entry);

    await persist({ ...state, wallets });

    document.getElementById('settingsCurrentPassword').value = '';
    document.getElementById('settingsNewPassword').value = '';
    document.getElementById('settingsNewPasswordConfirm').value = '';
    setStatus('Password updated and keys re-encrypted');
  } catch (error) {
    setStatus(`Password update failed: ${error?.message || error}`);
  }
}

async function onExportPrivateKey() {
  const wallet = getActiveWallet();
  if (!wallet) {
    setStatus('No active wallet');
    return;
  }

  try {
    const password = requireExportPassword();
    const privateKeyHex = await decryptPrivateKey(wallet.encryptedKey, password);
    setExportOutput(privateKeyHex);
    setStatus('Private key exported to output');
  } catch (error) {
    setStatus(`Export failed: ${error?.message || error}`);
  }
}

async function onExportMnemonic() {
  const wallet = getActiveWallet();
  if (!wallet || !wallet.encryptedMnemonic) {
    setStatus('No seed phrase stored for this wallet');
    return;
  }

  try {
    const password = requireExportPassword();
    const mnemonic = await decryptPrivateKey(wallet.encryptedMnemonic, password);
    setExportOutput(mnemonic);
    setStatus('Seed phrase exported to output');
  } catch (error) {
    setStatus(`Export failed: ${error?.message || error}`);
  }
}

async function onExportKeystore() {
  const wallet = getActiveWallet();
  if (!wallet) {
    setStatus('No active wallet');
    return;
  }

  try {
    const password = requireExportPassword();
    const privateKeyHex = await decryptPrivateKey(wallet.encryptedKey, password);
    const privateKeyBytes = hexToBytes(privateKeyHex);
    const publicKeyBytes = hexToBytes(wallet.publicKey);
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

async function onDownloadPrivateKey() {
  const wallet = getActiveWallet();
  if (!wallet) {
    setStatus('No active wallet');
    return;
  }

  try {
    const password = requireExportPassword();
    const privateKeyHex = await decryptPrivateKey(wallet.encryptedKey, password);
    const content = `MoltWallet Private Key\nWallet: ${wallet.name}\nAddress: ${wallet.address}\nExported: ${new Date().toISOString()}\n\nPrivate Key (Hex):\n${privateKeyHex}\n`;
    downloadTextFile(`molt-wallet-private-key-${wallet.name}-${Date.now()}.txt`, content);
    setStatus('Private key file downloaded');
  } catch (error) {
    setStatus(`Download failed: ${error?.message || error}`);
  }
}

async function onDownloadMnemonic() {
  const wallet = getActiveWallet();
  if (!wallet || !wallet.encryptedMnemonic) {
    setStatus('No seed phrase stored for this wallet');
    return;
  }

  try {
    const password = requireExportPassword();
    const mnemonic = await decryptPrivateKey(wallet.encryptedMnemonic, password);
    const content = `MoltWallet Seed Phrase\nWallet: ${wallet.name}\nAddress: ${wallet.address}\nExported: ${new Date().toISOString()}\n\nSeed Phrase (12 words):\n${mnemonic}\n`;
    downloadTextFile(`molt-wallet-seed-${wallet.name}-${Date.now()}.txt`, content);
    setStatus('Seed phrase file downloaded');
  } catch (error) {
    setStatus(`Download failed: ${error?.message || error}`);
  }
}

async function onCopyExportOutput() {
  const value = document.getElementById('settingsExportOutput').value;
  if (!value) {
    setStatus('Nothing to copy');
    return;
  }

  await navigator.clipboard.writeText(value);
  setStatus('Output copied');
}

async function loadApprovedOrigins() {
  const response = await chrome.runtime.sendMessage({ type: 'MOLT_PROVIDER_LIST_ORIGINS' }).catch(() => null);
  if (!response?.ok) {
    setOriginsHtml('<div>Failed to load approved origins</div>');
    return;
  }

  const origins = Array.isArray(response.result) ? response.result : [];
  if (!origins.length) {
    setOriginsHtml('<div>No approved origins</div>');
    return;
  }

  setOriginsHtml(origins.map((origin) => `
    <div class="settings-origin-item">
      <span class="mono">${origin}</span>
      <button class="btn btn-secondary btn-small" data-action="revokeOrigin" data-origin="${origin}">Revoke</button>
    </div>
  `).join(''));
}

async function onOriginsClick(event) {
  const target = event.target;
  if (!(target instanceof HTMLElement)) return;
  if (target.dataset?.action !== 'revokeOrigin') return;

  const origin = String(target.dataset.origin || '').trim();
  if (!origin) return;

  const confirmRevoke = prompt(`Type REVOKE to remove access for ${origin}:`, '');
  if (confirmRevoke !== 'REVOKE') {
    setStatus('Origin revoke cancelled');
    return;
  }

  const response = await chrome.runtime.sendMessage({
    type: 'MOLT_PROVIDER_REVOKE_ORIGIN',
    origin
  }).catch(() => null);

  if (!response?.ok) {
    setStatus(`Revoke failed: ${response?.error || 'Unknown error'}`);
    return;
  }

  setStatus('Origin access revoked');
  await loadApprovedOrigins();
}

function wireEvents() {
  document.getElementById('refreshSettings')?.addEventListener('click', refresh);
  document.getElementById('settingsWalletSelector')?.addEventListener('change', onSwitchWallet);
  document.getElementById('renameWalletBtn')?.addEventListener('click', onRenameWallet);
  document.getElementById('removeWalletBtn')?.addEventListener('click', onRemoveWallet);
  document.getElementById('lockNowBtn')?.addEventListener('click', onLockNow);
  document.getElementById('saveNetworkBtn')?.addEventListener('click', onSaveNetwork);
  document.getElementById('saveDisplaySecurityBtn')?.addEventListener('click', onSaveDisplaySecurity);
  document.getElementById('changePasswordBtn')?.addEventListener('click', onChangePassword);
  document.getElementById('exportPrivateKeyBtn')?.addEventListener('click', onExportPrivateKey);
  document.getElementById('exportMnemonicBtn')?.addEventListener('click', onExportMnemonic);
  document.getElementById('exportKeystoreBtn')?.addEventListener('click', onExportKeystore);
  document.getElementById('downloadPrivateKeyBtn')?.addEventListener('click', onDownloadPrivateKey);
  document.getElementById('downloadMnemonicBtn')?.addEventListener('click', onDownloadMnemonic);
  document.getElementById('copyExportOutputBtn')?.addEventListener('click', onCopyExportOutput);
  document.getElementById('refreshOriginsBtn')?.addEventListener('click', loadApprovedOrigins);
  document.getElementById('approvedOriginsList')?.addEventListener('click', onOriginsClick);
  document.getElementById('backHome')?.addEventListener('click', () => {
    location.href = chrome.runtime.getURL('src/pages/home.html');
  });
}

wireEvents();
refresh();
setStatus('Ready');
