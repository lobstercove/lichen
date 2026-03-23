import { LichenRPC, getConfiguredRpcEndpoint } from './rpc-service.js';
import { isValidAddress } from './crypto-service.js';

const SUPPORTED_CHAINS = ['solana', 'ethereum', 'bsc', 'bnb'];
const SUPPORTED_ASSETS = ['usdc', 'usdt', 'sol', 'eth', 'bnb'];

export async function loadBridgeSnapshot(address, network) {
  if (!address) return null;

  const rpc = new LichenRPC(await getConfiguredRpcEndpoint(network));
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
  const canonicalChain = normalizedChain === 'bnb' ? 'bsc' : normalizedChain;
  const normalizedAsset = String(asset || '').trim().toLowerCase();
  if (!SUPPORTED_CHAINS.includes(normalizedChain)) {
    throw new Error('Unsupported bridge chain');
  }
  if (!SUPPORTED_ASSETS.includes(normalizedAsset)) {
    throw new Error('Unsupported bridge asset');
  }

  // Route through RPC bridge proxy — custody auth handled server-side
  const rpc = new LichenRPC(await getConfiguredRpcEndpoint(network));
  return rpc.call('createBridgeDeposit', [{
    user_id: userAddress,
    chain: canonicalChain,
    asset: normalizedAsset
  }]);
}

export async function getBridgeDepositStatus(depositId, network) {
  if (!depositId) {
    throw new Error('Missing deposit id');
  }

  // Route through RPC bridge proxy — custody auth handled server-side
  const rpc = new LichenRPC(await getConfiguredRpcEndpoint(network));
  return rpc.call('getBridgeDeposit', [depositId]);
}
