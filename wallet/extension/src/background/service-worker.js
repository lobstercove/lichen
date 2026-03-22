import { loadState, saveState, DEFAULT_STATE, STORAGE_KEY } from '../core/state-store.js';
import { registerLockAlarmHandler } from '../core/lock-service.js';
import { walletWsManager } from '../core/ws-service.js';
import {
  handleProviderRequest,
  getProviderStateSnapshot,
  getPendingRequest,
  listPendingRequests,
  consumeFinalizedResult,
  finalizePendingRequest,
  listApprovedOrigins,
  revokeApprovedOrigin
} from '../core/provider-router.js';

const APP_VERSION = '0.1.0';

async function broadcastProviderStateDirty() {
  try {
    const tabs = await chrome.tabs.query({});
    await Promise.allSettled(
      tabs
        .filter((tab) => Number.isInteger(tab.id))
        .map((tab) => chrome.tabs.sendMessage(tab.id, { type: 'MOLT_PROVIDER_STATE_DIRTY' }))
    );
  } catch {
    // ignore tabs broadcast errors
  }

  try {
    await chrome.runtime.sendMessage({ type: 'MOLT_PROVIDER_STATE_DIRTY' });
  } catch {
    // ignore if no runtime listeners
  }
}

walletWsManager.addListener(async (event) => {
  if (!event) return;
  try {
    await chrome.runtime.sendMessage({ type: 'MOLT_WS_EVENT', payload: event });
  } catch {
    // ignore if no listeners
  }

  if (event.type === 'account-change') {
    await broadcastProviderStateDirty();
  }
});

chrome.storage.onChanged.addListener((changes, areaName) => {
  if (areaName !== 'local') return;
  if (!changes || !changes[STORAGE_KEY]) return;
  broadcastProviderStateDirty();
});

function resolveSenderOrigin(sender) {
  const directOrigin = typeof sender?.origin === 'string' ? sender.origin.trim() : '';
  if (directOrigin && directOrigin !== 'null') {
    return directOrigin;
  }

  const sourceUrl = typeof sender?.url === 'string' ? sender.url : '';
  if (!sourceUrl) return null;

  try {
    return new URL(sourceUrl).origin;
  } catch {
    return null;
  }
}

registerLockAlarmHandler();

chrome.runtime.onInstalled.addListener(() => {
  console.log('[MoltWallet] Extension installed');
});

chrome.runtime.onStartup.addListener(async () => {
  const state = await loadState();
  if (!state?.schemaVersion) {
    await saveState(DEFAULT_STATE);
  }

  const activeWallet = state.wallets?.find((wallet) => wallet.id === state.activeWalletId) || null;
  if (activeWallet && !state.isLocked) {
    walletWsManager.connect(activeWallet.address, state?.network?.selected || 'local-testnet');
  }
});

chrome.runtime.onMessage.addListener((message, sender, sendResponse) => {
  if (message?.type === 'MOLT_PING') {
    sendResponse({ ok: true, version: APP_VERSION });
    return true;
  }

  if (message?.type === 'MOLT_GET_STATE') {
    loadState()
      .then((state) => sendResponse({ ok: true, result: state }))
      .catch((error) => sendResponse({ ok: false, error: String(error?.message || error) }));
    return true;
  }

  if (message?.type === 'MOLT_PROVIDER_REQUEST') {
    const origin = resolveSenderOrigin(sender);

    loadState()
      .then((state) => {
        const activeWallet = state.wallets?.find((wallet) => wallet.id === state.activeWalletId) || null;
        return handleProviderRequest(message?.payload, {
          origin,
          network: state?.network?.selected || 'local-testnet',
          activeAddress: activeWallet?.address || null,
          isLocked: Boolean(state?.isLocked),
          appVersion: APP_VERSION
        });
      })
      .then(async (result) => {
        if (result?.pending && result?.requestId) {
          const url = chrome.runtime.getURL(`src/pages/approve.html?requestId=${encodeURIComponent(result.requestId)}`);
          await chrome.tabs.create({ url });
        }
        sendResponse(result);
      })
      .catch((error) => sendResponse({ ok: false, error: String(error?.message || error) }));
    return true;
  }

  if (message?.type === 'MOLT_PROVIDER_PENDING_GET') {
    const request = getPendingRequest(message?.requestId);
    if (!request) {
      sendResponse({ ok: false, error: 'Request not found' });
      return true;
    }

    loadState()
      .then(async (state) => {
        const activeWallet = state.wallets?.find((wallet) => wallet.id === state.activeWalletId) || null;
        const providerState = await getProviderStateSnapshot({
          origin: request.origin,
          network: state?.network?.selected || 'local-testnet',
          activeAddress: activeWallet?.address || null,
          isLocked: Boolean(state?.isLocked)
        });

        return {
          requestId: request.requestId,
          method: request.payload?.method,
          origin: request.origin,
          params: request.payload?.params || null,
          createdAt: request.createdAt,
          providerState
        };
      })
      .then((result) => sendResponse({ ok: true, result }))
      .catch((error) => sendResponse({ ok: false, error: String(error?.message || error) }));
    return true;
  }

  if (message?.type === 'MOLT_PROVIDER_LIST_PENDING') {
    const requests = listPendingRequests(Number(message?.limit || 20));
    sendResponse({ ok: true, result: requests });
    return true;
  }

  if (message?.type === 'MOLT_PROVIDER_PENDING_DECIDE') {
    loadState()
      .then((state) => {
        const activeWallet = state.wallets?.find((wallet) => wallet.id === state.activeWalletId) || null;
        return finalizePendingRequest(
          message?.requestId,
          Boolean(message?.approved),
          {
            activeAddress: activeWallet?.address || null,
            activeWallet: activeWallet || null,
            network: state?.network?.selected || 'local-testnet'
          },
          message?.approvalInput || {}
        );
      })
      .then(async (result) => {
        await broadcastProviderStateDirty();
        sendResponse(result);
      })
      .catch((error) => sendResponse({ ok: false, error: String(error?.message || error) }));
    return true;
  }

  if (message?.type === 'MOLT_PROVIDER_RESULT') {
    const finalized = consumeFinalizedResult(message?.requestId);
    if (!finalized) {
      sendResponse({ ok: true, pending: true });
      return true;
    }
    sendResponse(finalized);
    return true;
  }

  if (message?.type === 'MOLT_PROVIDER_LIST_ORIGINS') {
    listApprovedOrigins()
      .then((origins) => sendResponse({ ok: true, result: origins }))
      .catch((error) => sendResponse({ ok: false, error: String(error?.message || error) }));
    return true;
  }

  if (message?.type === 'MOLT_PROVIDER_REVOKE_ORIGIN') {
    const origin = String(message?.origin || '').trim();
    if (!origin) {
      sendResponse({ ok: false, error: 'Origin is required' });
      return true;
    }

    revokeApprovedOrigin(origin)
      .then(async () => {
        await broadcastProviderStateDirty();
        sendResponse({ ok: true });
      })
      .catch((error) => sendResponse({ ok: false, error: String(error?.message || error) }));
    return true;
  }

  if (message?.type === 'MOLT_NOTIFY') {
    const title = message?.payload?.title || 'MoltWallet';
    const body = message?.payload?.message || '';

    chrome.notifications.create({
      type: 'basic',
      iconUrl: 'MoltWallet_Logo_256.png',
      title,
      message: body
    });

    sendResponse({ ok: true });
    return true;
  }

  if (message?.type === 'MOLT_WS_STATUS') {
    sendResponse({ ok: true, result: walletWsManager.status() });
    return true;
  }

  if (message?.type === 'MOLT_WS_SYNC') {
    loadState()
      .then((state) => {
        const activeWallet = state.wallets?.find((wallet) => wallet.id === state.activeWalletId) || null;
        if (!activeWallet || state.isLocked) {
          walletWsManager.disconnect();
          return { state: walletWsManager.status() };
        }

        walletWsManager.connect(activeWallet.address, state?.network?.selected || 'local-testnet');
        return { state: walletWsManager.status() };
      })
      .then(async (result) => {
        await broadcastProviderStateDirty();
        sendResponse({ ok: true, result });
      })
      .catch((error) => sendResponse({ ok: false, error: String(error?.message || error) }));
    return true;
  }

  return false;
});
