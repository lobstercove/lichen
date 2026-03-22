import { STORAGE_KEY } from './state-store.js';

const NETWORKS = {
  mainnet: 'https://rpc.moltchain.network',
  testnet: 'https://testnet-rpc.moltchain.network',
  'local-testnet': 'http://localhost:8899',
  'local-mainnet': 'http://localhost:9899'
};

const WS_ENDPOINTS = {
  mainnet: 'wss://ws.moltchain.network',
  testnet: 'wss://testnet-ws.moltchain.network',
  'local-testnet': 'ws://localhost:8900',
  'local-mainnet': 'ws://localhost:9900'
};

function endpointFromSettings(network, settings = {}) {
  const map = {
    mainnet: settings.mainnetRPC,
    testnet: settings.testnetRPC,
    'local-testnet': settings.localTestnetRPC,
    'local-mainnet': settings.localMainnetRPC
  };

  const value = String(map[network] || '').trim();
  return value || null;
}

function toWsEndpoint(rpcEndpoint, fallbackNetwork = 'local-testnet') {
  const raw = String(rpcEndpoint || '').trim();
  if (!raw) {
    return WS_ENDPOINTS[fallbackNetwork] || WS_ENDPOINTS['local-testnet'];
  }

  try {
    const url = new URL(raw);
    if (url.protocol === 'https:') url.protocol = 'wss:';
    if (url.protocol === 'http:') url.protocol = 'ws:';
    if (!url.pathname || url.pathname === '/') {
      url.pathname = '/ws';
    }
    return url.toString().replace(/\/$/, '');
  } catch {
    return WS_ENDPOINTS[fallbackNetwork] || WS_ENDPOINTS['local-testnet'];
  }
}

export class MoltChainRPC {
  constructor(url) {
    this.url = url;
  }

  async call(method, params = []) {
    const response = await fetch(this.url, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        jsonrpc: '2.0',
        id: Date.now(),
        method,
        params
      })
    });

    const data = await response.json();
    if (data.error) {
      throw new Error(data.error.message || 'RPC Error');
    }

    return data.result;
  }

  getBalance(address) {
    return this.call('getBalance', [address]);
  }

  getAccount(address) {
    return this.call('getAccount', [address]);
  }

  sendTransaction(txData) {
    return this.call('sendTransaction', [txData]);
  }

  getLatestBlock() {
    return this.call('getLatestBlock');
  }

  getTransactionsByAddress(address, options = {}) {
    return this.call('getTransactionsByAddress', [address, options]);
  }
}

export function getRpcEndpoint(network = 'local-testnet', settings = null) {
  if (settings && typeof settings === 'object') {
    const custom = endpointFromSettings(network, settings);
    if (custom) return custom;
  }
  return NETWORKS[network] || NETWORKS['local-testnet'];
}

export async function getConfiguredRpcEndpoint(network = 'local-testnet') {
  const result = await chrome.storage.local.get(STORAGE_KEY).catch(() => ({}));
  const settings = result?.[STORAGE_KEY]?.settings || {};
  return getRpcEndpoint(network, settings);
}

export async function getConfiguredWsEndpoint(network = 'local-testnet') {
  const rpcEndpoint = await getConfiguredRpcEndpoint(network);
  return toWsEndpoint(rpcEndpoint, network);
}
