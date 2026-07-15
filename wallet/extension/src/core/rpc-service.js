import { DEFAULT_NETWORK, STORAGE_KEY } from './state-store.js';

const NETWORKS = {
  mainnet: 'https://rpc.lichen.network',
  testnet: 'https://testnet-api.lichen.network',
  'local-testnet': 'http://localhost:8899',
  'local-mainnet': 'http://localhost:9899'
};

const WS_ENDPOINTS = {
  mainnet: 'wss://rpc.lichen.network/ws',
  testnet: 'wss://testnet-api.lichen.network/ws',
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

function toWsEndpoint(rpcEndpoint, fallbackNetwork = DEFAULT_NETWORK) {
  const raw = String(rpcEndpoint || '').trim();
  if (!raw) {
    return WS_ENDPOINTS[fallbackNetwork] || WS_ENDPOINTS[DEFAULT_NETWORK];
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
    return WS_ENDPOINTS[fallbackNetwork] || WS_ENDPOINTS[DEFAULT_NETWORK];
  }
}

export class LichenRPC {
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

  simulateTransaction(txData) {
    return this.call('simulateTransaction', [txData]);
  }

  async sendTransactionWithPreflight(txData) {
    const simulation = await this.simulateTransaction(txData);
    if (!simulation?.success) {
      const error = simulation?.error || 'Transaction simulation failed';
      const returnCode = simulation?.returnCode === undefined || simulation?.returnCode === null
        ? ''
        : `, returnCode=${simulation.returnCode}`;
      throw new Error(`Preflight failed: ${error}${returnCode}`);
    }
    return this.sendTransaction(txData);
  }

  async getRecentBlockhash() {
    const result = await this.call('getRecentBlockhash');
    const blockhash = typeof result === 'string' ? result : result?.blockhash;
    if (!/^[0-9a-fA-F]{64}$/.test(String(blockhash || ''))) {
      throw new Error('RPC returned invalid recent blockhash');
    }
    return blockhash;
  }

  async getChainId() {
    const info = await this.call('getNetworkInfo');
    const chainId = String(info?.chain_id || '').trim();
    if (!chainId) throw new Error('RPC returned no chain id');
    return chainId;
  }

  getLatestBlock() {
    return this.call('getLatestBlock');
  }

  getTransactionsByAddress(address, options = {}) {
    return this.call('getTransactionsByAddress', [address, options]);
  }
}

export function getRpcEndpoint(network = DEFAULT_NETWORK, settings = null) {
  if (settings && typeof settings === 'object') {
    const custom = endpointFromSettings(network, settings);
    if (custom) return custom;
  }
  return getTrustedRpcEndpoint(network);
}

export function getTrustedRpcEndpoint(network = DEFAULT_NETWORK) {
  return NETWORKS[network] || NETWORKS[DEFAULT_NETWORK];
}

export async function getConfiguredRpcEndpoint(network = DEFAULT_NETWORK) {
  const result = await chrome.storage.local.get(STORAGE_KEY).catch(() => ({}));
  const settings = result?.[STORAGE_KEY]?.settings || {};
  return getRpcEndpoint(network, settings);
}

export async function getConfiguredWsEndpoint(network = DEFAULT_NETWORK) {
  const rpcEndpoint = await getConfiguredRpcEndpoint(network);
  return toWsEndpoint(rpcEndpoint, network);
}
