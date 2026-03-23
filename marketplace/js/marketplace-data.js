// Lichen Market data adapter — RPC-backed, zero mock data
// All functions return real chain data or empty results (never fake/generated)

(function () {
    'use strict';

    var EMOJIS = ['\u{1F99E}', '\u{1F980}', '\u{1F990}', '\u{1F419}', '\u{1F991}', '\u{1F41A}', '\u{1F988}', '\u{1F421}'];
    var CREATOR_EMOJIS = ['\u{1F3A8}', '\u2728', '\u{1F680}', '\u{1F48E}', '\u{1F525}', '\u26A1', '\u{1F31F}', '\u{1F3AF}', '\u{1F3C6}', '\u{1F451}'];
    var collectionNameCache = {};
    var SPORES_PER_LICN = 1000000000;

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

    function normalizeTimestamp(ts) {
        if (!ts) return Date.now();
        return ts < 1000000000000 ? ts * 1000 : ts;
    }

    function priceToLicn(spores) {
        if (!spores) return '0.00';
        var licn = spores / SPORES_PER_LICN;
        if (licn >= 1) return licn.toFixed(2);
        if (licn >= 0.01) return licn.toFixed(4);
        if (licn >= 0.0001) return licn.toFixed(6);
        if (licn >= 0.0000001) return licn.toFixed(9);
        if (licn > 0) return '< 0.000000001';
        return '0.00';
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
        } catch (err) { console.warn("marketplace-data:", err.message || err);
            return formatHash(contractId, 12);
        }
    }

    // ===== Data Source Methods =====

    async function getFeaturedCollections(limit) {
        try {
            var contracts = await rpcCall('getAllContracts', []);
            if (!Array.isArray(contracts) || contracts.length === 0) return [];
            var maxItems = Math.max(1, Number(limit) || 6);
            return contracts.slice(0, maxItems).map(function (c) {
                return {
                    id: c.id || c.program_id || '',
                    name: c.name || c.symbol || formatHash(c.id || '', 12),
                    creator: c.owner || c.deployer || '-',
                    avatar: EMOJIS[hashString(c.id || 'x') % EMOJIS.length],
                    banner: gradientFromHash(c.id || 'col'),
                    image: gradientFromHash(c.id || 'col'),
                    items: c.token_count || 0,
                    floor: c.floor_price ? priceToLicn(c.floor_price) : '0.00',
                    volume: c.total_volume ? priceToLicn(c.total_volume) : '0.00',
                };
            });
        } catch (err) { console.warn("marketplace-data:", err.message || err);
            return [];
        }
    }

    async function getTrendingNFTs(limit, period) {
        try {
            var maxItems = Math.max(1, Number(limit) || 12);
            var listings = await rpcCall('getMarketListings', [{ limit: maxItems }]);
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
                    collectionId: item.collection || item.contract_id || '',
                    creator: item.creator || item.owner || '-',
                    seller: item.seller || item.owner || '-',
                    image: item.metadata_uri || item.image || gradientFromHash(item.id || item.token || 'nft-' + i),
                    price: item.price_licn !== undefined ? formatLicnPrice(item.price_licn, true) : priceToLicn(item.price || 0),
                    rarity: item.rarity || null,
                    period: period || null,
                    lastSale: item.last_sale ? priceToLicn(item.last_sale) : null,
                });
            }
            return results;
        } catch (err) { console.warn("marketplace-data:", err.message || err);
            return [];
        }
    }

    async function getTopCreators(limit) {
        try {
            var maxItems = Math.max(1, Number(limit) || 8);
            var sales = await rpcCall('getMarketSales', [{ limit: 200 }]);
            var saleList = sales && sales.sales ? sales.sales : (Array.isArray(sales) ? sales : []);
            if (saleList.length === 0) return [];
            var creatorMap = {};
            saleList.forEach(function (s) {
                var creator = s.seller || s.creator;
                if (!creator) return;
                if (!creatorMap[creator]) creatorMap[creator] = { address: creator, volume: 0, sales: 0 };
                creatorMap[creator].volume += s.price_licn !== undefined ? Number(s.price_licn) : (s.price ? s.price / SPORES_PER_LICN : 0);
                creatorMap[creator].sales += 1;
            });
            var sorted = Object.values(creatorMap).sort(function (a, b) { return b.volume - a.volume; });
            return sorted.slice(0, maxItems).map(function (c, i) {
                var h = hashString(c.address);
                return {
                    id: c.address,
                    address: c.address,
                    name: formatHash(c.address, 10),
                    avatar: CREATOR_EMOJIS[h % CREATOR_EMOJIS.length],
                    volume: formatLicnPrice(c.volume, true),
                    sales: c.sales,
                    rank: i + 1,
                };
            });
        } catch (err) { console.warn("marketplace-data:", err.message || err);
            return [];
        }
    }

    async function getRecentSales(limit) {
        try {
            var maxItems = Math.max(1, Number(limit) || 10);
            var sales = await rpcCall('getMarketSales', [{ limit: maxItems }]);
            var saleList = sales && sales.sales ? sales.sales : (Array.isArray(sales) ? sales : []);
            if (saleList.length === 0) return [];
            var results = [];
            for (var i = 0; i < saleList.length; i++) {
                var s = saleList[i];
                var colName = await getCollectionName(s.collection || s.contract_id);
                results.push({
                    id: s.id || s.token || '',
                    nft: s.name || (s.token_id !== undefined ? '#' + s.token_id : 'Sale #' + (i + 1)),
                    collection: colName,
                    image: s.metadata_uri || s.image || gradientFromHash(s.id || s.token || 'sale-' + i),
                    price: s.price_licn !== undefined ? formatLicnPrice(s.price_licn, true) : priceToLicn(s.price || 0),
                    from: s.seller || '-',
                    to: s.buyer || '-',
                    timestamp: normalizeTimestamp(s.timestamp),
                });
            }
            return results;
        } catch (err) { console.warn("marketplace-data:", err.message || err);
            return [];
        }
    }

    async function getStats() {
        var stats = { totalNFTs: 0, totalCollections: 0, totalVolume: 0, totalCreators: 0 };
        try {
            var metrics = await rpcCall('getMetrics', []);
            if (metrics) {
                stats.totalCollections = metrics.total_contracts || 0;
            }
        } catch (err) { console.warn("marketplace-data:", err.message || err); }
        try {
            var marketStats = await rpcCall('getLichenMarketStats', []);
            if (marketStats && typeof marketStats === 'object') {
                stats.totalNFTs = Number(marketStats.listing_count || 0);
                stats.totalVolume = Number(marketStats.sale_volume || 0) / SPORES_PER_LICN;
                if (marketStats.creator_count !== undefined) {
                    stats.totalCreators = Number(marketStats.creator_count || 0);
                }
            }
        } catch (err) { console.warn("marketplace-data:", err.message || err); }
        return stats;
    }

    // ===== User-specific Data Methods =====

    async function getWalletBalance(address) {
        if (!address) return 0;
        try {
            var result = await rpcCall('getBalance', [address]);
            if (result && typeof result === 'object') {
                return (result.balance || result.value || 0) / SPORES_PER_LICN;
            }
            return (Number(result) || 0) / SPORES_PER_LICN;
        } catch (err) { console.warn("marketplace-data:", err.message || err); }
        return 0;
    }

    async function getUserCollections(address) {
        if (!address) return [];
        try {
            var contracts = await rpcCall('getAllContracts', []);
            if (!Array.isArray(contracts)) return [];
            return contracts.filter(function (c) {
                var owner = c.owner || c.deployer || c.creator || '';
                return owner === address;
            }).map(function (c) {
                return {
                    id: c.id || c.program_id || '',
                    name: c.name || c.symbol || formatHash(c.id || '', 12),
                    symbol: c.symbol || '',
                    items: c.token_count || 0,
                    floor: c.floor_price ? priceToLicn(c.floor_price) : '0.00',
                    volume: c.total_volume ? priceToLicn(c.total_volume) : '0.00',
                };
            });
        } catch (err) { console.warn("marketplace-data:", err.message || err);
            return [];
        }
    }

    async function getNFTsByOwner(address) {
        if (!address) return [];
        try {
            var result = await rpcCall('getNFTsByOwner', [address, { limit: 200 }]);
            return result && result.nfts ? result.nfts : (Array.isArray(result) ? result : []);
        } catch (err) { console.warn("marketplace-data:", err.message || err);
            return [];
        }
    }

    async function getNFTDetail(tokenId) {
        if (!tokenId) return null;
        try {
            return await rpcCall('getNFT', [tokenId]);
        } catch (err) { console.warn("marketplace-data:", err.message || err);
            return null;
        }
    }

    async function getNFTsByCollection(collectionId, limit) {
        if (!collectionId) return [];
        try {
            var result = await rpcCall('getNFTsByCollection', [collectionId, { limit: limit || 20 }]);
            return result && result.nfts ? result.nfts : (Array.isArray(result) ? result : []);
        } catch (err) { console.warn("marketplace-data:", err.message || err);
            return [];
        }
    }

    async function getNFTActivity(collectionId, limit) {
        if (!collectionId) return [];
        try {
            var result = await rpcCall('getNFTActivity', [collectionId, { limit: limit || 20 }]);
            return result && result.activity ? result.activity : (Array.isArray(result) ? result : []);
        } catch (err) { console.warn("marketplace-data:", err.message || err);
            return [];
        }
    }

    async function getAllListings(limitOrOpts) {
        try {
            var params = {};
            if (typeof limitOrOpts === 'object' && limitOrOpts !== null) {
                params = limitOrOpts;
                if (!params.limit) params.limit = 500;
            } else {
                params.limit = limitOrOpts || 500;
            }
            var result = await rpcCall('getMarketListings', [params]);
            return (result && result.listings) ? result.listings : (Array.isArray(result) ? result : []);
        } catch (err) { console.warn("marketplace-data:", err.message || err);
            return [];
        }
    }

    async function getAllCollections() {
        try {
            var result = await rpcCall('getAllContracts', []);
            return Array.isArray(result) ? result : [];
        } catch (err) { console.warn("marketplace-data:", err.message || err);
            return [];
        }
    }

    async function resolveMarketplaceProgram() {
        try {
            var entry = await rpcCall('getSymbolRegistry', ['LICHENMARKET']);
            return entry && (entry.program || entry.program_id) ? (entry.program || entry.program_id) : null;
        } catch (err) { console.warn("marketplace-data:", err.message || err);
            return null;
        }
    }

    // ===== Export =====
    window.marketplaceDataSource = {
        getFeaturedCollections: getFeaturedCollections,
        getTrendingNFTs: getTrendingNFTs,
        getTopCreators: getTopCreators,
        getRecentSales: getRecentSales,
        getStats: getStats,
        getWalletBalance: getWalletBalance,
        getUserCollections: getUserCollections,
        getNFTsByOwner: getNFTsByOwner,
        getNFTDetail: getNFTDetail,
        getNFTsByCollection: getNFTsByCollection,
        getNFTActivity: getNFTActivity,
        getAllListings: getAllListings,
        getAllCollections: getAllCollections,
        resolveMarketplaceProgram: resolveMarketplaceProgram,
    };

    // Smart price formatting — always displays in LICN (matches explorer)
    function formatLicnPrice(value, isLicn) {
        var licn = isLicn ? Number(value || 0) : (Number(value || 0) / SPORES_PER_LICN);
        if (licn === 0) return '0.00';
        if (licn >= 1) return licn.toFixed(2);
        if (licn >= 0.01) return licn.toFixed(4);
        if (licn >= 0.0001) return licn.toFixed(6);
        if (licn >= 0.0000001) return licn.toFixed(9);
        if (licn > 0) return '< 0.000000001';
        return '0.00';
    }

    window.marketplaceUtils = {
        hashString: hashString,
        gradientFromHash: gradientFromHash,
        normalizeTimestamp: normalizeTimestamp,
        priceToLicn: priceToLicn,
        formatLicnPrice: formatLicnPrice,
        EMOJIS: EMOJIS,
        CREATOR_EMOJIS: CREATOR_EMOJIS,
        SPORES_PER_LICN: SPORES_PER_LICN,
    };

    console.log('Lichen Market data source initialized (RPC-backed, zero mock data)');
})();
