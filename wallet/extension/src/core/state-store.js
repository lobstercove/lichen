const STORAGE_KEY = 'lichenWalletState';
const DEFAULT_NETWORK = 'testnet';
const CURRENT_SCHEMA_VERSION = 2;

const DEFAULT_STATE = {
  schemaVersion: CURRENT_SCHEMA_VERSION,
  wallets: [],
  activeWalletId: null,
  isLocked: true,
  settings: {
    currency: 'USD',
    lockTimeout: 300000
  },
  network: {
    selected: DEFAULT_NETWORK
  }
};

function migrateState(raw) {
  const migrated = {
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

  const version = Number(raw.schemaVersion || 1);
  const selected = raw.network?.selected;
  const hasCustomLocalRpc = String(raw.settings?.localTestnetRPC || '').trim().length > 0;
  if (version < 2 && selected === 'local-testnet' && !hasCustomLocalRpc) {
    migrated.network.selected = DEFAULT_NETWORK;
  }
  migrated.schemaVersion = CURRENT_SCHEMA_VERSION;

  return migrated;
}

export async function loadState() {
  const result = await chrome.storage.local.get(STORAGE_KEY);
  const raw = result?.[STORAGE_KEY];

  if (!raw || typeof raw !== 'object') {
    return structuredClone(DEFAULT_STATE);
  }

  return migrateState(raw);
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

export { STORAGE_KEY, DEFAULT_STATE, DEFAULT_NETWORK };
