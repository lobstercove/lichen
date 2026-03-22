import { MoltChainRPC, getConfiguredRpcEndpoint } from './rpc-service.js';

export async function loadNftSnapshot(address, network) {
  if (!address) return null;

  const rpc = new MoltChainRPC(await getConfiguredRpcEndpoint(network));
  const response = await rpc.call('getNFTsByOwner', [address, { limit: 20 }]).catch(() => null);

  const items = response?.nfts || response?.items || (Array.isArray(response) ? response : []);

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

  const rpc = new MoltChainRPC(await getConfiguredRpcEndpoint(network));
  const response = await rpc.call('getNFTsByOwner', [address, { limit }]).catch(() => null);
  const items = response?.nfts || response?.items || (Array.isArray(response) ? response : []);

  return items.map((item, idx) => ({
    mint: item.mint || item.id || `nft-${idx}`,
    standard: item.standard || item.token_standard || 'Unknown',
    name: item.name || item.metadata?.name || `NFT #${idx + 1}`,
    symbol: item.symbol || item.metadata?.symbol || 'NFT',
    amount: Number(item.amount || item.balance || 1),
    image: item.image || item.metadata?.image || '',
    collection: item.collection || item.metadata?.collection || '',
    raw: item
  }));
}
