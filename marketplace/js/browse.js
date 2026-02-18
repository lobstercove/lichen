// Molt Market — Browse / Filter Page
// Loads NFT listings from RPC, supports filtering, sorting, pagination

(function () {
    'use strict';

    // AUDIT-FIX MK-4: XSS prevention utility
    function escapeHtml(str) {
        return String(str ?? '')
            .replace(/&/g, '&amp;')
            .replace(/</g, '&lt;')
            .replace(/>/g, '&gt;')
            .replace(/"/g, '&quot;')
            .replace(/'/g, '&#39;');
    }

    const RPC_URL = (window.moltMarketConfig && window.moltMarketConfig.rpcUrl) || 'http://localhost:8899';
    const PAGE_SIZE = 20;

    let allNFTs = [];
    let filteredNFTs = [];
    let currentPage = 1;
    let totalPages = 1;
    let currentSort = 'recent';
    let currentView = 'grid';
    let currentWallet = null;
    let collectionsLoaded = [];

    // ===== RPC Helper =====
    async function rpcCall(method, params) {
        const res = await fetch(RPC_URL, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ jsonrpc: '2.0', id: 1, method, params }),
        });
        const data = await res.json();
        if (data.error) throw new Error(data.error.message || 'RPC error');
        return data.result;
    }

    // ===== Utilities =====
    function formatNumber(num) {
        if (num === undefined || num === null) return '0';
        return Number(num).toLocaleString();
    }

    function formatHash(hash, length) {
        length = length || 16;
        if (!hash) return '-';
        if (hash.length <= length) return hash;
        const half = Math.floor(length / 2);
        return hash.slice(0, half) + '...' + hash.slice(-half);
    }

    function timeAgo(ts) {
        const seconds = Math.floor((Date.now() - ts) / 1000);
        if (seconds < 60) return seconds + 's ago';
        if (seconds < 3600) return Math.floor(seconds / 60) + 'm ago';
        if (seconds < 86400) return Math.floor(seconds / 3600) + 'h ago';
        return Math.floor(seconds / 86400) + 'd ago';
    }

    function hashString(input) {
        let hash = 0;
        for (let i = 0; i < input.length; i++) {
            hash = (hash << 5) - hash + input.charCodeAt(i);
            hash |= 0;
        }
        return Math.abs(hash);
    }

    function gradientFromHash(seed) {
        const base = hashString(seed);
        function colorFromNum(n) {
            const r = (n & 0xff0000) >> 16;
            const g = (n & 0x00ff00) >> 8;
            const b = n & 0x0000ff;
            return '#' + r.toString(16).padStart(2, '0') + g.toString(16).padStart(2, '0') + b.toString(16).padStart(2, '0');
        }
        return 'linear-gradient(135deg, ' + colorFromNum(hashString(base + '-a')) + ', ' + colorFromNum(hashString(base + '-b')) + ')';
    }

    function normalizeImage(uri, seed) {
        if (uri && uri.startsWith('ipfs://')) return uri.replace('ipfs://', 'https://ipfs.io/ipfs/');
        if (uri && (uri.startsWith('http://') || uri.startsWith('https://'))) return uri;
        return gradientFromHash(seed || 'default');
    }

    function priceToMolt(shells) {
        if (!shells) return '0.00';
        return (shells / 1_000_000_000).toFixed(2);
    }

    function normalizeTimestamp(ts) {
        if (!ts) return Date.now();
        return ts < 1_000_000_000_000 ? ts * 1000 : ts;
    }

    // ===== Data Loading =====
    async function fetchListings() {
        try {
            const result = await rpcCall('getMarketListings', [{ limit: 500 }]);
            return (result && result.listings) ? result.listings : (Array.isArray(result) ? result : []);
        } catch (err) {
            console.warn('Failed to fetch listings:', err);
            return [];
        }
    }

    async function fetchCollections() {
        try {
            const result = await rpcCall('getAllContracts', []);
            return Array.isArray(result) ? result : [];
        } catch (err) {
            console.warn('Failed to fetch collections:', err);
            return [];
        }
    }

    async function fetchNFTDetail(tokenId) {
        try {
            return await rpcCall('getNFT', [tokenId]);
        } catch (err) {
            return null;
        }
    }

    async function loadCollections() {
        const container = document.getElementById('collectionsList');
        if (!container) return;

        container.innerHTML = '<div style="padding: 8px; color: var(--text-secondary);">Loading...</div>';

        let collections = [];
        try {
            collections = await fetchCollections();
        } catch (_) {
            // ignore
        }

        if (!collections.length) {
            container.innerHTML = '<div style="padding: 8px; color: var(--text-secondary);">No collections available</div>';
            return;
        }

        collectionsLoaded = collections;
        container.innerHTML = collections.map(function (c, i) {
            const name = c.name || c.symbol || formatHash(c.id || c.program_id || ('col-' + i), 12);
            const id = c.id || c.program_id || ('collection-' + i);
            return '<label class="collection-filter-item">' +
                '<input type="checkbox" value="' + id + '" onchange="window._browseApplyFilters()">' +
                '<span>' + name + '</span>' +
                '</label>';
        }).join('');
    }

    async function loadAllNFTs() {
        showLoading(true);

        const listings = await fetchListings();

        // Also try getMarketSales for extra data
        let sales = [];
        try {
            const salesResult = await rpcCall('getMarketSales', [{ limit: 200 }]);
            sales = (salesResult && salesResult.sales) ? salesResult.sales : (Array.isArray(salesResult) ? salesResult : []);
        } catch (_) {
            // ignore
        }

        // Merge listings into NFT items
        const nftMap = new Map();

        for (const listing of listings) {
            const id = listing.token || listing.nft_id || listing.id;
            if (!id) continue;
            const seed = id;
            nftMap.set(id, {
                id: id,
                name: listing.name || (listing.token_id !== undefined ? '#' + listing.token_id : '#' + hashString(id) % 10000),
                collection: listing.collection || listing.collection_name || 'Unknown',
                collectionId: listing.collection || null,
                image: normalizeImage(listing.metadata_uri || listing.image, seed),
                price: listing.price_molt !== undefined ? Number(listing.price_molt).toFixed(2) : priceToMolt(listing.price || 0),
                priceRaw: listing.price || 0,
                lastSale: null,
                rarity: ['Common', 'Uncommon', 'Rare', 'Epic', 'Legendary'][hashString(seed) % 5],
                timestamp: normalizeTimestamp(listing.timestamp || listing.listed_at),
                seller: listing.seller || listing.owner || null,
                status: 'listed',
            });
        }

        for (const sale of sales) {
            const id = sale.token || sale.nft_id || sale.id;
            if (!id) continue;
            if (nftMap.has(id)) {
                nftMap.get(id).lastSale = sale.price_molt !== undefined ? Number(sale.price_molt).toFixed(2) : priceToMolt(sale.price || 0);
            }
        }

        allNFTs = Array.from(nftMap.values());

        // Read URL query params
        const params = new URLSearchParams(window.location.search);
        const q = params.get('q');
        const collectionParam = params.get('collection');

        if (q) {
            const searchInput = document.getElementById('searchInput');
            if (searchInput) searchInput.value = q;
        }

        applyFilters();
        showLoading(false);
    }

    // ===== Filtering & Sorting =====
    function applyFilters() {
        const params = new URLSearchParams(window.location.search);
        const searchQuery = (document.getElementById('searchInput') || {}).value || params.get('q') || '';
        const minPrice = parseFloat((document.getElementById('minPrice') || {}).value) || 0;
        const maxPrice = parseFloat((document.getElementById('maxPrice') || {}).value) || Infinity;
        const collectionParam = params.get('collection');

        // Gather checked collection filters
        const checkedCollections = new Set();
        const checkboxes = document.querySelectorAll('#collectionsList input[type="checkbox"]:checked');
        checkboxes.forEach(function (cb) { checkedCollections.add(cb.value); });

        // Status filter (listed / auction / has_offers)
        const statusChecks = document.querySelectorAll('.filter-group input[type="checkbox"][value]');
        const activeStatuses = new Set();
        statusChecks.forEach(function (cb) {
            if (cb.checked && ['listed', 'auction', 'has_offers'].indexOf(cb.value) !== -1) {
                activeStatuses.add(cb.value);
            }
        });

        // Rarity filter
        const rarityChecks = document.querySelectorAll('.filter-group input[type="checkbox"]');
        const activeRarities = new Set();
        rarityChecks.forEach(function (cb) {
            if (cb.checked && ['Common', 'Uncommon', 'Rare', 'Epic', 'Legendary'].indexOf(cb.value) !== -1) {
                activeRarities.add(cb.value);
            }
        });

        filteredNFTs = allNFTs.filter(function (nft) {
            // Search text
            if (searchQuery) {
                const q = searchQuery.toLowerCase();
                const matchName = (nft.name || '').toLowerCase().indexOf(q) !== -1;
                const matchCollection = (nft.collection || '').toLowerCase().indexOf(q) !== -1;
                const matchId = (nft.id || '').toLowerCase().indexOf(q) !== -1;
                if (!matchName && !matchCollection && !matchId) return false;
            }

            // Price range
            const price = parseFloat(nft.price) || 0;
            if (price < minPrice) return false;
            if (maxPrice !== Infinity && price > maxPrice) return false;

            // Collection filter (from URL or checkboxes)
            if (collectionParam && nft.collectionId !== collectionParam && nft.collection !== collectionParam) return false;
            if (checkedCollections.size > 0 && !checkedCollections.has(nft.collectionId) && !checkedCollections.has(nft.collection)) return false;

            // Status filter
            if (activeStatuses.size > 0 && !activeStatuses.has(nft.status)) return false;

            // Rarity filter
            if (activeRarities.size > 0 && !activeRarities.has(nft.rarity)) return false;

            return true;
        });

        // Sort
        sortNFTs();

        // Pagination
        totalPages = Math.max(1, Math.ceil(filteredNFTs.length / PAGE_SIZE));
        if (currentPage > totalPages) currentPage = totalPages;

        renderNFTGrid();
        updatePagination();
        updateCount();
    }

    function sortNFTs() {
        const sortSelect = document.getElementById('sortSelect');
        currentSort = sortSelect ? sortSelect.value : 'recent';

        filteredNFTs.sort(function (a, b) {
            switch (currentSort) {
                case 'price_low':
                    return (parseFloat(a.price) || 0) - (parseFloat(b.price) || 0);
                case 'price_high':
                    return (parseFloat(b.price) || 0) - (parseFloat(a.price) || 0);
                case 'popular':
                    return hashString(b.id) - hashString(a.id); // deterministic popularity proxy
                case 'ending':
                    return (a.timestamp || 0) - (b.timestamp || 0);
                case 'recent':
                default:
                    return (b.timestamp || 0) - (a.timestamp || 0);
            }
        });
    }

    // ===== Rendering =====
    function renderNFTGrid() {
        const container = document.getElementById('nftsGrid');
        if (!container) return;

        const start = (currentPage - 1) * PAGE_SIZE;
        const pageItems = filteredNFTs.slice(start, start + PAGE_SIZE);

        if (pageItems.length === 0) {
            container.innerHTML = '<div class="empty-state" style="grid-column: 1 / -1; text-align: center; padding: 60px 20px; color: var(--text-secondary);">' +
                '<i class="fas fa-search" style="font-size: 48px; margin-bottom: 16px; opacity: 0.3;"></i>' +
                '<h3>No NFTs found</h3>' +
                '<p>Try adjusting your filters or search query</p>' +
                '</div>';
            return;
        }

        if (currentView === 'grid') {
            container.innerHTML = pageItems.map(function (nft) {
                const isGradient = nft.image && nft.image.startsWith('linear-gradient');
                const imageStyle = isGradient
                    ? 'background: ' + nft.image
                    : 'background-image: url(' + nft.image + '); background-size: cover; background-position: center;';

                return '<div class="nft-card" onclick="window._browseViewNFT(\'' + escapeHtml(nft.id) + '\')">' +
                    '<div class="nft-image" style="' + imageStyle + '"></div>' +
                    '<div class="nft-info">' +
                    '<div class="nft-collection">' + escapeHtml(nft.collection || 'Unknown') + '</div>' +
                    '<div class="nft-name">' + escapeHtml(nft.name || 'Unnamed') + '</div>' +
                    '<div class="nft-footer">' +
                    '<div class="nft-price">Price <span class="nft-price-value">' + escapeHtml(nft.price) + ' MOLT</span></div>' +
                    '<div class="nft-rarity nft-rarity-' + escapeHtml((nft.rarity || 'Common').toLowerCase()) + '">' + escapeHtml(nft.rarity || 'Common') + '</div>' +
                    '</div>' +
                    '</div>' +
                    '</div>';
            }).join('');
        } else {
            // List view
            container.innerHTML = '<div class="nft-list">' + pageItems.map(function (nft) {
                const isGradient = nft.image && nft.image.startsWith('linear-gradient');
                const imageStyle = isGradient
                    ? 'background: ' + nft.image
                    : 'background-image: url(' + nft.image + '); background-size: cover; background-position: center;';

                return '<div class="nft-list-item" onclick="window._browseViewNFT(\'' + nft.id + '\')">' +
                    '<div class="nft-list-image" style="width:64px;height:64px;border-radius:8px;' + imageStyle + '"></div>' +
                    '<div class="nft-list-info" style="flex:1;padding:0 16px;">' +
                    '<div class="nft-collection">' + (nft.collection || 'Unknown') + '</div>' +
                    '<div class="nft-name">' + (nft.name || 'Unnamed') + '</div>' +
                    '</div>' +
                    '<div class="nft-list-price" style="text-align:right;">' +
                    '<div class="nft-price-value">' + nft.price + ' MOLT</div>' +
                    '<div class="nft-rarity nft-rarity-' + (nft.rarity || 'Common').toLowerCase() + '">' + (nft.rarity || 'Common') + '</div>' +
                    '</div>' +
                    '</div>';
            }).join('') + '</div>';
        }
    }

    function updatePagination() {
        const currentPageEl = document.getElementById('currentPage');
        const totalPagesEl = document.getElementById('totalPages');
        const prevBtn = document.getElementById('prevPage');
        const nextBtn = document.getElementById('nextPage');
        const paginationEl = document.getElementById('pagination');

        if (currentPageEl) currentPageEl.textContent = currentPage;
        if (totalPagesEl) totalPagesEl.textContent = totalPages;
        if (prevBtn) prevBtn.disabled = currentPage <= 1;
        if (nextBtn) nextBtn.disabled = currentPage >= totalPages;
        if (paginationEl) paginationEl.style.display = totalPages > 1 ? 'flex' : 'none';
    }

    function updateCount() {
        const countEl = document.getElementById('nftCount');
        if (countEl) countEl.textContent = formatNumber(filteredNFTs.length);
    }

    function showLoading(show) {
        const container = document.getElementById('nftsGrid');
        if (!container) return;
        if (show) {
            container.innerHTML = '<div style="grid-column: 1 / -1; text-align: center; padding: 60px 20px; color: var(--text-secondary);">' +
                '<i class="fas fa-spinner fa-spin" style="font-size: 32px; margin-bottom: 12px;"></i>' +
                '<p>Loading NFTs...</p></div>';
        }
    }

    // ===== Event Setup =====
    function setupEvents() {
        // Sort change
        const sortSelect = document.getElementById('sortSelect');
        if (sortSelect) {
            sortSelect.addEventListener('change', function () {
                currentPage = 1;
                applyFilters();
            });
        }

        // Pagination
        const prevBtn = document.getElementById('prevPage');
        const nextBtn = document.getElementById('nextPage');
        if (prevBtn) prevBtn.addEventListener('click', function () {
            if (currentPage > 1) { currentPage--; renderNFTGrid(); updatePagination(); }
        });
        if (nextBtn) nextBtn.addEventListener('click', function () {
            if (currentPage < totalPages) { currentPage++; renderNFTGrid(); updatePagination(); }
        });

        // View toggle buttons (grid / list)
        var viewButtons = document.querySelectorAll('[data-view]');
        viewButtons.forEach(function (btn) {
            btn.addEventListener('click', function () {
                currentView = btn.dataset.view;
                viewButtons.forEach(function (b) { b.classList.remove('active'); });
                btn.classList.add('active');
                renderNFTGrid();
            });
        });

        // Search
        var searchInput = document.getElementById('searchInput');
        if (searchInput) {
            searchInput.addEventListener('keypress', function (e) {
                if (e.key === 'Enter') {
                    currentPage = 1;
                    applyFilters();
                }
            });
        }

        // Collection search within filter panel
        var collectionSearch = document.getElementById('collectionSearch');
        if (collectionSearch) {
            collectionSearch.addEventListener('input', function () {
                var query = collectionSearch.value.toLowerCase();
                var items = document.querySelectorAll('#collectionsList .collection-filter-item');
                items.forEach(function (item) {
                    var text = item.textContent.toLowerCase();
                    item.style.display = text.indexOf(query) !== -1 ? '' : 'none';
                });
            });
        }

        // Use shared wallet manager
        if (window.MoltWallet) {
            window.moltWallet = window.moltWallet || new MoltWallet({ rpcUrl: RPC_URL });
            window.moltWallet.bindConnectButton('#connectWallet');
            window.moltWallet.onConnect(function(info) {
                currentWallet = info;
            });
            window.moltWallet.onDisconnect(function() {
                currentWallet = null;
            });
        }

        // Mobile nav toggle
        var navToggle = document.getElementById('navToggle');
        if (navToggle) {
            navToggle.addEventListener('click', function () {
                var navMenu = document.querySelector('.nav-menu');
                if (navMenu) navMenu.classList.toggle('active');
            });
        }
    }

    // ===== Public API (for onclick handlers in HTML) =====
    window._browseApplyFilters = function () { currentPage = 1; applyFilters(); };
    window._browseViewNFT = function (id) { window.location.href = 'item.html?id=' + encodeURIComponent(id); };

    // HTML references applyPriceFilter and clearFilters without module prefix
    window.applyPriceFilter = function () { currentPage = 1; applyFilters(); };
    window.clearFilters = function () {
        var minPrice = document.getElementById('minPrice');
        var maxPrice = document.getElementById('maxPrice');
        if (minPrice) minPrice.value = '';
        if (maxPrice) maxPrice.value = '';

        // Uncheck all filter checkboxes
        var checkboxes = document.querySelectorAll('.filter-group input[type="checkbox"]');
        checkboxes.forEach(function (cb) { cb.checked = false; });

        // Uncheck collection checkboxes
        var collCheckboxes = document.querySelectorAll('#collectionsList input[type="checkbox"]');
        collCheckboxes.forEach(function (cb) { cb.checked = false; });

        currentPage = 1;
        applyFilters();
    };

    // ===== Init =====
    document.addEventListener('DOMContentLoaded', function () {
        console.log('🦞 Molt Market Browse loading...');
        if (typeof initMarketNetworkSelector === 'function') initMarketNetworkSelector();
        setupEvents();
        loadCollections();
        loadAllNFTs();
        console.log('✅ Molt Market Browse ready');
    });
})();
