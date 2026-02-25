import { MoltChainRPC, getConfiguredRpcEndpoint } from './rpc-service.js';
import { isValidAddress } from './crypto-service.js';
import { STORAGE_KEY } from './state-store.js';

const CUSTODY_ENDPOINTS = {
  mainnet: 'https://custody.moltchain.network',
  testnet: 'https://testnet-custody.moltchain.network',
  'local-testnet': 'http://localhost:9105',
  'local-mainnet': 'http://localhost:9105'
};

const SUPPORTED_CHAINS = ['solana', 'ethereum', 'bsc', 'bnb'];
const SUPPORTED_ASSETS = ['usdc', 'usdt', 'sol', 'eth', 'bnb'];

function getCustodyEndpoint(network = 'local-testnet') {
  return CUSTODY_ENDPOINTS[network] || CUSTODY_ENDPOINTS['local-testnet'];
}

async function getConfiguredCustodyEndpoint(network = 'local-testnet') {
  const result = await chrome.storage.local.get(STORAGE_KEY).catch(() => ({}));
  const settings = result?.[STORAGE_KEY]?.settings || {};

  const custom = network === 'mainnet'
    ? settings.mainnetCustody
    : network === 'testnet'
      ? settings.testnetCustody
      : settings.localCustody;

  const endpoint = String(custom || '').trim();
  return endpoint || getCustodyEndpoint(network);
}

export async function loadBridgeSnapshot(address, network) {
  if (!address) return null;

  const rpc = new MoltChainRPC(await getConfiguredRpcEndpoint(network));
  const history = await rpc.call('getBridgeDepositsByRecipient', [address, { limit: 10 }]).catch(() => null);

  const deposits = history?.deposits || history?.items || (Array.isArray(history) ? history : []);
  const pending = deposits.filter((d) => {
    const s = String(d.status || '').toLowerCase();
    return s && s !== 'credited' && s !== 'completed';
  }).length;

  return {
    totalDeposits: deposits.length,
    pending,
    raw: deposits
  };
}

export async function requestBridgeDepositAddress({ userAddress, chain, asset, network }) {
  if (!userAddress) {
    throw new Error('Missing user address');
  }
  if (!isValidAddress(userAddress)) {
    throw new Error('Invalid user wallet address');
  }

  const normalizedChain = String(chain || '').trim().toLowerCase();
  const normalizedAsset = String(asset || '').trim().toLowerCase();
  const canonicalChain = normalizedChain === 'bnb' ? 'bsc' : normalizedChain;
  if (!SUPPORTED_CHAINS.includes(normalizedChain)) {
    throw new Error('Unsupported bridge chain');
  }
  if (!SUPPORTED_ASSETS.includes(normalizedAsset)) {
    throw new Error('Unsupported bridge asset');
  }

  const endpoint = await getConfiguredCustodyEndpoint(network);
  const response = await fetch(`${endpoint}/deposits`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      user_id: userAddress,
      chain: canonicalChain,
      asset: normalizedAsset
    })
  });

  if (!response.ok) {
    const error = await response.json().catch(() => ({ message: 'Deposit request failed' }));
    throw new Error(error?.message || 'Deposit request failed');
  }

  return response.json();
}

export async function getBridgeDepositStatus(depositId, network) {
  if (!depositId) {
    throw new Error('Missing deposit id');
  }

  const endpoint = await getConfiguredCustodyEndpoint(network);
  const response = await fetch(`${endpoint}/deposits/${encodeURIComponent(depositId)}`);
  if (!response.ok) {
    const error = await response.json().catch(() => ({ message: 'Status request failed' }));
    throw new Error(error?.message || 'Status request failed');
  }

  return response.json();
}
