const STORAGE_KEY = 'moltWalletState';

const DEFAULT_STATE = {
  schemaVersion: 1,
  wallets: [],
  activeWalletId: null,
  isLocked: true,
  settings: {
    currency: 'USD',
    lockTimeout: 300000
  },
  network: {
    selected: 'local-testnet'
  }
};

export async function loadState() {
  const result = await chrome.storage.local.get(STORAGE_KEY);
  const raw = result?.[STORAGE_KEY];

  if (!raw || typeof raw !== 'object') {
    return structuredClone(DEFAULT_STATE);
  }

  return {
    ...structuredClone(DEFAULT_STATE),
    ...raw,
    settings: {
      ...DEFAULT_STATE.settings,
      ...(raw.settings || {})
    },
    network: {
      ...DEFAULT_STATE.network,
      ...(raw.network || {})
    }
  };
}

export async function saveState(nextState) {
  await chrome.storage.local.set({
    [STORAGE_KEY]: nextState
  });
  return nextState;
}

export async function patchState(partial) {
  const state = await loadState();
  const merged = {
    ...state,
    ...partial,
    settings: {
      ...state.settings,
      ...(partial.settings || {})
    },
    network: {
      ...state.network,
      ...(partial.network || {})
    }
  };

  await saveState(merged);
  return merged;
}

export { STORAGE_KEY, DEFAULT_STATE };
