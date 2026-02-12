// Molt Market data adapter — RPC-backed, zero mock data
// All functions return real chain data or empty results (never fake/generated)

(function () {
    'use strict';

    var DEFAULT_RPC_URL = (window.moltMarketConfig && window.moltMarketConfig.rpcUrl) || 'http://localhost:8899';
    var EMOJIS = ['\u{1F99E}', '\u{1F980}', '\u{1F990}', '\u{1F419}', '\u{1F991}', '\u{1F41A}', '\u{1F988}', '\u{1F421}'];
    var CREATOR_EMOJIS = ['\u{1F3A8}', '\u2728', '\u{1F680}', '\u{1F48E}', '\u{1F525}', '\u26A1', '\u{1F31F}', '\u{1F3AF}', '\u{1F3C6}', '\u{1F451}'];
    var collectionNameCache = {};

    var PERIODS = {
        '24h': 24 * 60 * 60 * 1000,
        '7d': 7 * 24 * 60 * 60 * 1000,
        '30d': 30 * 24 * 60 * 60 * 1000,
    };

    // ===== Utility Helpers =====

    function hashString(input) {
        var hash = 0;
        for (var i = 0; i < input.length; i++) {
            hash = (hash << 5) - hash + input.charCodeAt(i);
            hash |= 0;
        }
        return Math.abs(hash);
    }

    function gradientFromHash(seed) {
        var base = hashString(seed);
        function colorFromNum(n) {
            var r = (n & 0xff0000) >> 16;
            var g = (n & 0x00ff00) >> 8;
            var b = n & 0x0000ff;
            return '#' + r.toString(16).padStart(2, '0') + g.toString(16).padStart(2, '0') + b.toString(16).padStart(2, '0');
        }
        return 'linear-gradient(135deg, ' + colorFromNum(hashString(base + '-a')) + ', ' + colorFromNum(hashString(base + '-b')) + ')';
    }

    function formatHash(hash, length) {
        length = length || 16;
        if (!hash) return '-';
        if (hash.length <= length) return hash;
        var half = Math.floor(length / 2);
        return hash.slice(0, half) + '...' + hash.slice(-half);
    }

    function normalizeTimestamp(ts) {
        if (!ts) return Date.now();
        return ts < 1000000000000 ? ts * 1000 : ts;
    }

    function priceToMolt(shells) {
        if (!shells) return '0.00';
        return (shells / 1000000000).toFixed(2);
    }

    // ===== RPC Call =====
    async function rpcCall(method, params) {
        var res = await fetch(DEFAULT_RPC_URL, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ jsonrpc: '2.0', id: 1, method: method, params: params }),
        });
        var data = await res.json();
        if (data.error) throw new Error(data.error.message || 'RPC error');
        return data.result;
    }

    // ===== Collection Name Lookup =====
    async function getCollectionName(contractId) {
        if (!contractId) return 'Unknown';
        if (collectionNameCache[contractId]) return collectionNameCache[contractId];
        try {
            var info = await rpcCall('getContractInfo', [contractId]);
            var name = (info && info.name) || formatHash(contractId, 12);
            collectionNameCache[contractId] = name;
            return name;
        } catch (_) {
            return formatHash(contractId, 12);
        }
    }

    // ===== Data Source Methods =====

    async function getFeaturedCollections() {
        try {
            var contracts = await rpcCall('getAllContracts', []);
            if (!Array.isArray(contracts) || contracts.length === 0) return [];
            return contracts.slice(0, 6).map(function (c) {
                return {
                    id: c.id || c.program_id || '',
                    name: c.name || c.symbol || formatHash(c.id || '', 12),
                    creator: c.owner || c.deployer || '-',
                    image: gradientFromHash(c.id || 'col'),
                    items: c.token_count || 0,
                    floor: c.floor_price ? priceToMolt(c.floor_price) : '0.00',
                    volume: c.total_volume ? priceToMolt(c.total_volume) : '0.00',
                };
            });
        } catch (_) {
            return [];
        }
    }

    async function getTrendingNFTs() {
        try {
            var listings = await rpcCall('getMarketListings', [{ limit: 12 }]);
            var items = listings && listings.listings ? listings.listings : (Array.isArray(listings) ? listings : []);
            if (items.length === 0) return [];
            var results = [];
            for (var i = 0; i < items.length; i++) {
                var item = items[i];
                var colName = await getCollectionName(item.collection || item.contract_id);
                results.push({
                    id: item.id || item.token || '',
                    name: item.name || (item.token_id !== undefined ? '#' + item.token_id : 'NFT #' + (i + 1)),
                    collection: colName,
                    creator: item.creator || item.owner || '-',
                    image: item.metadata_uri || item.image || gradientFromHash(item.id || item.token || 'nft-' + i),
                    price: item.price_molt !== undefined ? Number(item.price_molt).toFixed(2) : priceToMolt(item.price || 0),
                    rarity: item.rarity || null,
                    lastSale: item.last_sale ? priceToMolt(item.last_sale) : null,
                });
            }
            return results;
        } catch (_) {
            return [];
        }
    }

    async function getTopCreators() {
        try {
            var sales = await rpcCall('getMarketSales', [{ limit: 200 }]);
            var saleList = sales && sales.sales ? sales.sales : (Array.isArray(sales) ? sales : []);
            if (saleList.length === 0) return [];
            var creatorMap = {};
            saleList.forEach(function (s) {
                var creator = s.seller || s.creator;
                if (!creator) return;
                if (!creatorMap[creator]) creatorMap[creator] = { address: creator, volume: 0, sales: 0 };
                creatorMap[creator].volume += s.price_molt !== undefined ? Number(s.price_molt) : (s.price ? s.price / 1000000000 : 0);
                creatorMap[creator].sales += 1;
            });
            var sorted = Object.values(creatorMap).sort(function (a, b) { return b.volume - a.volume; });
            return sorted.slice(0, 8).map(function (c, i) {
                var h = hashString(c.address);
                return {
                    address: c.address,
                    name: formatHash(c.address, 10),
                    avatar: CREATOR_EMOJIS[h % CREATOR_EMOJIS.length],
                    volume: c.volume.toFixed(2),
                    sales: c.sales,
                    rank: i + 1,
                };
            });
        } catch (_) {
            return [];
        }
    }

    async function getRecentSales() {
        try {
            var sales = await rpcCall('getMarketSales', [{ limit: 10 }]);
            var saleList = sales && sales.sales ? sales.sales : (Array.isArray(sales) ? sales : []);
            if (saleList.length === 0) return [];
            var results = [];
            for (var i = 0; i < saleList.length; i++) {
                var s = saleList[i];
                var colName = await getCollectionName(s.collection || s.contract_id);
                results.push({
                    id: s.id || s.token || '',
                    name: s.name || (s.token_id !== undefined ? '#' + s.token_id : 'Sale #' + (i + 1)),
                    collection: colName,
                    image: s.metadata_uri || s.image || gradientFromHash(s.id || s.token || 'sale-' + i),
                    price: s.price_molt !== undefined ? Number(s.price_molt).toFixed(2) : priceToMolt(s.price || 0),
                    seller: s.seller || '-',
                    buyer: s.buyer || '-',
                    timestamp: normalizeTimestamp(s.timestamp),
                });
            }
            return results;
        } catch (_) {
            return [];
        }
    }

    async function getStats() {
        var stats = { nfts: 0, collections: 0, volume: 0, creators: 0 };
        try {
            var metrics = await rpcCall('getMetrics', []);
            if (metrics) {
                stats.collections = metrics.total_contracts || 0;
            }
        } catch (_) {}
        try {
            var listings = await rpcCall('getMarketListings', [{ limit: 1 }]);
            stats.nfts = listings && listings.total !== undefined ? listings.total : 0;
        } catch (_) {}
        try {
            var sales = await rpcCall('getMarketSales', [{ limit: 500 }]);
            var saleList = sales && sales.sales ? sales.sales : (Array.isArray(sales) ? sales : []);
            var creatorSet = {};
            var totalVolume = 0;
            saleList.forEach(function (s) {
                totalVolume += s.price_molt !== undefined ? Number(s.price_molt) : (s.price ? s.price / 1000000000 : 0);
                if (s.seller) creatorSet[s.seller] = true;
            });
            stats.volume = totalVolume;
            stats.creators = Object.keys(creatorSet).length;
        } catch (_) {}
        return stats;
    }

    async function getWalletBalance(address) {
        if (!address) return { balance: 0 };
        try {
            var result = await rpcCall('getTokenBalance', [address]);
            if (result && result.balance !== undefined) {
                return { balance: Number(result.balance) };
            }
            var result2 = await rpcCall('getBalance', [address]);
            if (result2 !== undefined) {
                return { balance: typeof result2 === 'number' ? result2 / 1000000000 : Number(result2) / 1000000000 };
            }
        } catch (_) {}
        return { balance: 0 };
    }

    // ===== Export =====
    window.marketplaceDataSource = {
        getFeaturedCollections: getFeaturedCollections,
        getTrendingNFTs: getTrendingNFTs,
        getTopCreators: getTopCreators,
        getRecentSales: getRecentSales,
        getStats: getStats,
        getWalletBalance: getWalletBalance,
        rpcCall: rpcCall,
    };

    window.marketplaceUtils = {
        hashString: hashString,
        gradientFromHash: gradientFromHash,
        formatHash: formatHash,
        normalizeTimestamp: normalizeTimestamp,
        priceToMolt: priceToMolt,
        EMOJIS: EMOJIS,
        CREATOR_EMOJIS: CREATOR_EMOJIS,
    };

    console.log('Molt Market data source initialized (RPC-backed, zero mock data)');
})();
