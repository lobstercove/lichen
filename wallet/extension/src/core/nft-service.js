import { LichenRPC, getConfiguredRpcEndpoint } from './rpc-service.js';

function requireNftEnvelope(response) {
  if (!response || typeof response !== 'object' || Array.isArray(response) || !Array.isArray(response.nfts)) {
    throw new Error('Invalid RPC response: expected nfts array');
  }
  return response.nfts;
}

export async function loadNftSnapshot(address, network) {
  if (!address) return null;

  const rpc = new LichenRPC(await getConfiguredRpcEndpoint(network));
  const response = await rpc.call('getNFTsByOwner', [address, { limit: 20 }]);
  const items = requireNftEnvelope(response);

  return {
    count: items.length,
    standards: {
      mts721: items.filter((n) => n.standard === 'MTS-721').length,
      mts1155: items.filter((n) => n.standard === 'MTS-1155').length
    },
    raw: items
  };
}

export async function loadNftDetails(address, network, limit = 50) {
  if (!address) return [];

  const rpc = new LichenRPC(await getConfiguredRpcEndpoint(network));
  const response = await rpc.call('getNFTsByOwner', [address, { limit }]);
  const items = requireNftEnvelope(response);

  return items.map((item, idx) => ({
    mint: item.mint || item.id || `nft-${idx}`,
    standard: item.standard || item.token_standard || 'Unknown',
    name: item.name || item.metadata?.name || `NFT #${idx + 1}`,
    symbol: item.symbol || item.metadata?.symbol || 'NFT',
    amount: Number(item.amount ?? item.balance ?? 1),
    image: item.image || item.metadata?.image || '',
    collection: item.collection || item.metadata?.collection || '',
    raw: item
  }));
}
