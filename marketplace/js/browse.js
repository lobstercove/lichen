// Molt Market — Browse / Explore Page
// Wallet-gated buy actions, filter/sort/search, loads from RPC

(function () {
    'use strict';

    var RPC_URL = (window.moltMarketConfig && window.moltMarketConfig.rpcUrl) || 'http://localhost:8899';
    var CONTRACT_PROGRAM_ID = null;
    var dataSource = window.marketplaceDataSource;
    var currentWallet = null;
    var allListings = [];
    var filteredListings = [];
    var allCollections = [];
    var currentPage = 1;
    var PAGE_SIZE = 20;
    var currentView = 'grid';
    var currentSort = 'recent';
    var currentFilter = '';
    var searchQuery = '';
    var priceMin = null;
    var priceMax = null;
    var selectedRarities = [];
    var statusBuyNow = true;
    var statusHasOffers = false;
    var urlFilterMode = '';

    var fmp = (window.marketplaceUtils && window.marketplaceUtils.formatMoltPrice) || function(v, isMolt) { return Number(isMolt ? v : v/1e9).toFixed(2); };

    function lazyAddresses() {
        if (!CONTRACT_PROGRAM_ID) CONTRACT_PROGRAM_ID = bs58encode(new Uint8Array(32).fill(0xFF));
    }

    function hashString(input) {
        var hash = 0;
        for (var i = 0; i < input.length; i++) { hash = (hash << 5) - hash + input.charCodeAt(i); hash |= 0; }
        return Math.abs(hash);
    }

    function gradientFromHash(seed) {
        var base = hashString(seed);
        function c(n) { return '#' + ((n & 0xff0000) >> 16).toString(16).padStart(2, '0') + ((n & 0x00ff00) >> 8).toString(16).padStart(2, '0') + (n & 0x0000ff).toString(16).padStart(2, '0'); }
        return 'linear-gradient(135deg, ' + c(hashString(base + '-a')) + ', ' + c(hashString(base + '-b')) + ')';
    }

    function normalizeImage(uri, seed) {
        if (!uri || typeof uri !== 'string') return gradientFromHash(seed || 'nft');
        uri = uri.trim();
        if (!uri) return gradientFromHash(seed || 'nft');
        if (uri.startsWith('linear-gradient')) return uri;
        if (uri.startsWith('ipfs://')) return 'https://ipfs.io/ipfs/' + uri.slice('ipfs://'.length);
        if (uri.startsWith('http://') || uri.startsWith('https://')) return uri;
        return gradientFromHash(seed || 'nft');
    }

    function showToast(msg, type) {
        var bg = type === 'error' ? '#ef4444' : type === 'success' ? '#22c55e' : '#3b82f6';
        var toast = document.createElement('div');
        toast.style.cssText = 'position:fixed;bottom:24px;left:50%;transform:translateX(-50%);background:' + bg + ';color:#fff;padding:12px 24px;border-radius:8px;z-index:9999;font-size:14px;max-width:500px;text-align:center;box-shadow:0 4px 12px rgba(0,0,0,0.3);';
        toast.textContent = msg;
        document.body.appendChild(toast);
        setTimeout(function () { toast.remove(); }, 5000);
    }

    // ===== Nav Wallet Gating =====
    function updateNav() {
        var navMenuItems = document.querySelectorAll('.nav-menu li');
        navMenuItems.forEach(function (li) {
            var link = li.querySelector('a');
            if (link && link.getAttribute('href') === 'create.html') {
                li.style.display = currentWallet ? '' : 'none';
            }
        });
        var navMenu = document.querySelector('.nav-menu');
        if (!navMenu) return;
        var existing = document.getElementById('navProfileItem');
        if (currentWallet) {
            if (!existing) {
                var li = document.createElement('li');
                li.id = 'navProfileItem';
                li.innerHTML = '<a href="profile.html?id=' + encodeURIComponent(currentWallet.address) + '">Profile</a>';
                navMenu.appendChild(li);
            } else {
                existing.querySelector('a').href = 'profile.html?id=' + encodeURIComponent(currentWallet.address);
            }
        } else {
            if (existing) existing.remove();
        }
    }

    // ===== Build RPC filter params =====
    function buildFilterParams() {
        var params = { limit: 500 };
        if (currentFilter) params.collection = currentFilter;
        if (priceMin !== null && priceMin > 0) params.price_min = priceMin;
        if (priceMax !== null && priceMax > 0) params.price_max = priceMax;
        if (statusHasOffers) params.has_offers = true;
        if (currentSort === 'price_low') params.sort_by = 'price_asc';
        else if (currentSort === 'price_high') params.sort_by = 'price_desc';
        else if (currentSort === 'oldest') params.sort_by = 'oldest';
        else params.sort_by = 'newest';
        if (selectedRarities.length > 0) params.rarity = parseInt(selectedRarities[0], 10);
        return params;
    }

    // ===== Load All Data =====
    async function loadListings() {
        try {
            if (dataSource) {
                var params = buildFilterParams();
                allListings = await dataSource.getAllListings(params);
            }
        } catch (err) {
            console.warn('Failed to load listings:', err);
            allListings = [];
        }
        applyFilters();
        renderListings();
        updateResultCount();
    }

    async function loadCollections() {
        try {
            if (dataSource) {
                allCollections = await dataSource.getAllCollections();
            }
        } catch (_) {
            allCollections = [];
        }
        renderCollectionFilter();
    }

    // ===== Filtering =====
    function applyFilters() {
        filteredListings = allListings.slice();

        if (!statusBuyNow) {
            filteredListings = [];
        }

        if (statusHasOffers) {
            filteredListings = filteredListings.filter(function (item) {
                var hasOffers = Boolean(item.has_offers || item.hasOffers);
                if (!hasOffers) {
                    var offerCount = Number(item.offer_count || item.offers_count || item.offers || 0);
                    hasOffers = Number.isFinite(offerCount) && offerCount > 0;
                }
                return hasOffers;
            });
        }

        if (urlFilterMode === 'featured') {
            filteredListings = filteredListings.filter(function (item) {
                if (item.featured === true || item.is_featured === true || item.verified === true) return true;
                var rarityValue = Number(item.rarity_value);
                if (Number.isFinite(rarityValue) && rarityValue >= 3) return true;
                var rarity = String(item.rarity || '').toLowerCase();
                return rarity === 'epic' || rarity === 'legendary';
            });
        } else if (urlFilterMode === 'creators') {
            filteredListings = filteredListings.filter(function (item) {
                var creator = String(item.creator || item.seller || item.owner || '').trim();
                return creator.length > 0;
            });
        }

        // Search
        if (searchQuery) {
            var q = searchQuery.toLowerCase();
            filteredListings = filteredListings.filter(function (item) {
                var name = (item.name || ('NFT #' + (item.token_id || ''))).toLowerCase();
                var collection = (item.collection_name || item.collection || item.program || '').toLowerCase();
                var id = (item.id || '').toLowerCase();
                var tokenId = String(item.token_id || '').toLowerCase();
                return name.indexOf(q) >= 0 || collection.indexOf(q) >= 0 || id.indexOf(q) >= 0 || tokenId.indexOf(q) >= 0;
            });
        }

        // Collection filter
        if (currentFilter) {
            filteredListings = filteredListings.filter(function (item) {
                return (item.collection || item.program || item.contract_id || '') === currentFilter;
            });
        }

        // Rarity filter (client-side for multi-select)
        if (selectedRarities.length > 0) {
            filteredListings = filteredListings.filter(function (item) {
                var itemRarity = item.rarity_value !== undefined ? String(item.rarity_value) : null;
                if (itemRarity === null) {
                    var rarityMap = { 'common': '0', 'uncommon': '1', 'rare': '2', 'epic': '3', 'legendary': '4' };
                    itemRarity = rarityMap[(item.rarity || 'common').toLowerCase()] || '0';
                }
                return selectedRarities.indexOf(itemRarity) >= 0;
            });
        }

        // Sort
        if (currentSort === 'price_low') {
            filteredListings.sort(function (a, b) { return (a.price || 0) - (b.price || 0); });
        } else if (currentSort === 'price_high') {
            filteredListings.sort(function (a, b) { return (b.price || 0) - (a.price || 0); });
        } else if (currentSort === 'oldest') {
            filteredListings.sort(function (a, b) { return (a.timestamp || 0) - (b.timestamp || 0); });
        } else {
            // recent by default
            filteredListings.sort(function (a, b) { return (b.timestamp || 0) - (a.timestamp || 0); });
        }

        currentPage = 1;
    }

    function updateResultCount() {
        var countEl = document.getElementById('nftCount');
        if (countEl) countEl.textContent = filteredListings.length;
    }

    // ===== View Toggle =====
    function setView(view) {
        currentView = view;
        var grid = document.getElementById('nftsGrid');
        if (grid) {
            if (view === 'list') {
                grid.classList.add('list-view');
            } else {
                grid.classList.remove('list-view');
            }
        }
        var btns = document.querySelectorAll('.view-btn');
        btns.forEach(function (btn) {
            if (btn.getAttribute('data-view') === view) {
                btn.classList.add('active');
            } else {
                btn.classList.remove('active');
            }
        });
        renderListings();
    }

    // ===== Render =====
    function renderListings() {
        var grid = document.getElementById('nftsGrid');
        if (!grid) return;

        var start = (currentPage - 1) * PAGE_SIZE;
        var pageItems = filteredListings.slice(start, start + PAGE_SIZE);

        if (pageItems.length === 0) {
            grid.innerHTML = '<div class="browse-empty">' +
                '<i class="fas fa-search" style="font-size:48px;margin-bottom:16px;display:block;"></i>' +
                '<h3>No NFTs found</h3><p>' + (searchQuery ? 'Try a different search term' : 'Check back later for new listings') + '</p></div>';
            return;
        }

        if (currentView === 'grid') {
            grid.innerHTML = pageItems.map(function (nft) {
                var priceInMolt = nft.price_molt !== undefined ? fmp(nft.price_molt, true) : fmp(nft.price || 0, false);
                var normalizedImage = normalizeImage(nft.image, nft.id || nft.name || '');
                var imgHtml = normalizedImage && normalizedImage.indexOf('http') === 0
                    ? '<img src="' + escapeHtml(normalizedImage) + '" style="width:100%;height:100%;object-fit:cover;" alt="' + escapeHtml(nft.name || '') + '">'
                    : '<div style="width:100%;height:100%;background:' + gradientFromHash(nft.id || nft.name || '') + ';display:flex;align-items:center;justify-content:center;font-size:48px;opacity:0.5;">\uD83D\uDDBC\uFE0F</div>';

                var actionHtml = '';
                if (currentWallet) {
                    actionHtml = '<button class="nft-action" onclick="event.stopPropagation();window._browseBuyNFT(\'' + escapeJsAttr(nft.id) + '\')">Buy Now</button>';
                } else {
                    actionHtml = '<button class="nft-action" onclick="event.stopPropagation();window._browseConnect()" style="opacity:0.7;">Connect to Buy</button>';
                }

                return '<div class="nft-card" onclick="window.location.href=\'item.html?id=' + encodeURIComponent(nft.id || '') + '&contract=' + encodeURIComponent(nft.collection || nft.program || nft.contract_id || '') + '&token=' + encodeURIComponent(nft.token_id || '') + '\'">' +
                    '<div class="nft-image">' + imgHtml + '</div>' +
                    '<div class="nft-info">' +
                    '<div class="nft-collection">' + escapeHtml(nft.collection || 'Unknown') + '</div>' +
                    '<div class="nft-name">' + escapeHtml(nft.name || 'NFT #' + (nft.token_id || nft.id || '?')) + '</div>' +
                    '<div class="nft-footer">' +
                    '<div class="nft-price">Price <span class="nft-price-value">' + priceInMolt + ' MOLT</span></div>' +
                    actionHtml +
                    '</div></div></div>';
            }).join('');
        } else {
            // List view
            grid.innerHTML = '<div class="nft-list-header">' +
                '<div class="nft-list-col nft-list-col-image"></div>' +
                '<div class="nft-list-col nft-list-col-name">Item</div>' +
                '<div class="nft-list-col nft-list-col-collection">Collection</div>' +
                '<div class="nft-list-col nft-list-col-rarity">Rarity</div>' +
                '<div class="nft-list-col nft-list-col-price">Price</div>' +
                '<div class="nft-list-col nft-list-col-action"></div>' +
                '</div>' +
                pageItems.map(function (nft) {
                var priceInMolt = nft.price_molt !== undefined ? fmp(nft.price_molt, true) : fmp(nft.price || 0, false);
                var normalizedImage = normalizeImage(nft.image, nft.id || nft.name || '');
                var imgStyle = normalizedImage && normalizedImage.indexOf('http') === 0
                    ? 'background-image:url(' + encodeURI(normalizedImage) + ');background-size:cover;background-position:center;'
                    : 'background:' + gradientFromHash(nft.id || nft.name || '') + ';';

                var rarityLabel = escapeHtml(nft.rarity || 'Common');
                var rarityClass = escapeHtml((nft.rarity || 'Common').toLowerCase());

                var actionHtml = '';
                if (currentWallet) {
                    actionHtml = '<button class="btn btn-primary btn-small" onclick="event.stopPropagation();window._browseBuyNFT(\'' + escapeJsAttr(nft.id) + '\')">Buy</button>';
                } else {
                    actionHtml = '<button class="btn btn-secondary btn-small" onclick="event.stopPropagation();window._browseConnect()">Connect</button>';
                }

                return '<div class="nft-list-item" onclick="window.location.href=\'item.html?id=' + encodeURIComponent(nft.id || '') + '&contract=' + encodeURIComponent(nft.collection || nft.program || nft.contract_id || '') + '&token=' + encodeURIComponent(nft.token_id || '') + '\'">' +
                    '<div class="nft-list-col nft-list-col-image"><div class="nft-list-thumb" style="' + imgStyle + '"></div></div>' +
                    '<div class="nft-list-col nft-list-col-name">' + escapeHtml(nft.name || 'NFT #' + (nft.token_id || nft.id || '?')) + '</div>' +
                    '<div class="nft-list-col nft-list-col-collection">' + escapeHtml(nft.collection || 'Unknown') + '</div>' +
                    '<div class="nft-list-col nft-list-col-rarity"><span class="rarity ' + rarityClass + '">' + rarityLabel + '</span></div>' +
                    '<div class="nft-list-col nft-list-col-price">' + escapeHtml(priceInMolt) + ' MOLT</div>' +
                    '<div class="nft-list-col nft-list-col-action">' + actionHtml + '</div>' +
                    '</div>';
            }).join('');
        }

        renderPagination();
    }

    function renderPagination() {
        var pagEl = document.getElementById('pagination');
        if (!pagEl) return;
        var totalPages = Math.ceil(filteredListings.length / PAGE_SIZE);
        if (totalPages <= 1) { pagEl.innerHTML = ''; return; }

        var html = '';
        if (currentPage > 1) {
            html += '<button class="btn btn-secondary" onclick="window._browseSetPage(' + (currentPage - 1) + ')"><i class="fas fa-chevron-left"></i></button>';
        }

        function pageBtn(i) {
            return '<button class="btn ' + (i === currentPage ? 'btn-primary' : 'btn-secondary') + '" onclick="window._browseSetPage(' + i + ')">' + i + '</button>';
        }

        if (totalPages <= 10) {
            for (var i = 1; i <= totalPages; i++) html += pageBtn(i);
        } else {
            var startPage = Math.max(1, currentPage - 2);
            var endPage = Math.min(totalPages, currentPage + 2);

            if (startPage > 1) {
                html += pageBtn(1);
                if (startPage > 2) html += '<span class="pagination-ellipsis">…</span>';
            }

            for (var p = startPage; p <= endPage; p++) html += pageBtn(p);

            if (endPage < totalPages) {
                if (endPage < totalPages - 1) html += '<span class="pagination-ellipsis">…</span>';
                html += pageBtn(totalPages);
            }
        }

        if (currentPage < totalPages) {
            html += '<button class="btn btn-secondary" onclick="window._browseSetPage(' + (currentPage + 1) + ')"><i class="fas fa-chevron-right"></i></button>';
        }
        pagEl.innerHTML = html;
    }

    function renderCollectionFilter() {
        var filterList = document.getElementById('collectionsList');
        if (!filterList) return;

        if (allCollections.length === 0) {
            filterList.innerHTML = '<p style="padding:12px;opacity:0.5;font-size:13px;">No collections found</p>';
            return;
        }

        filterList.innerHTML = '<label style="display:block;padding:8px 12px;cursor:pointer;">' +
            '<input type="radio" name="collectionFilter" value="" ' + (!currentFilter ? 'checked' : '') + ' onchange="window._browseFilterCollection(\'\')" style="margin-right:8px;"> All Collections</label>' +
            allCollections.map(function (c) {
                var colId = c.id || c.program_id || '';
                var colName = c.name || c.symbol || formatHash(colId, 10);
                return '<label style="display:block;padding:8px 12px;cursor:pointer;">' +
                    '<input type="radio" name="collectionFilter" value="' + escapeHtml(c.id || c.program_id || '') + '" ' +
                    (currentFilter === colId ? 'checked' : '') +
                    ' onchange="window._browseFilterCollection(\'' + escapeHtml(colId) + '\')" style="margin-right:8px;"> ' +
                    escapeHtml(c.name || c.symbol || formatHash(colId, 10)) + '</label>';
            }).join('');
    }

    // ===== Public API =====
    window._browseBuyNFT = function (id) {
        if (!currentWallet) {
            showToast('Connect wallet to buy', 'error');
            return;
        }
        window.location.href = 'item.html?id=' + encodeURIComponent(id);
    };

    window._browseConnect = function () {
        if (window.moltWallet) window.moltWallet._openWalletModal();
    };

    window._browseSetPage = function (page) {
        currentPage = page;
        renderListings();
        window.scrollTo({ top: 0, behavior: 'smooth' });
    };

    window._browseFilterCollection = function (colId) {
        currentFilter = colId;
        loadListings();
    };

    window.clearFilters = function () {
        currentFilter = '';
        currentSort = 'recent';
        searchQuery = '';
        priceMin = null;
        priceMax = null;
        selectedRarities = [];
        statusBuyNow = true;
        statusHasOffers = false;
        urlFilterMode = '';
        currentPage = 1;

        var searchInput = document.getElementById('searchInput');
        var browseSearch = document.getElementById('browseSearch');
        var minEl = document.getElementById('minPrice');
        var maxEl = document.getElementById('maxPrice');
        var sortSelect = document.getElementById('sortSelect');
        var buyNowBox = document.getElementById('filterBuyNow');
        var hasOffersBox = document.getElementById('filterHasOffers');

        if (searchInput) searchInput.value = '';
        if (browseSearch) browseSearch.value = '';
        if (minEl) minEl.value = '';
        if (maxEl) maxEl.value = '';
        if (sortSelect) sortSelect.value = 'recent';
        if (buyNowBox) buyNowBox.checked = true;
        if (hasOffersBox) hasOffersBox.checked = false;
        document.querySelectorAll('.rarityFilter').forEach(function (box) { box.checked = false; });
        var allCollectionsRadio = document.querySelector('input[name="collectionFilter"][value=""]');
        if (allCollectionsRadio) allCollectionsRadio.checked = true;

        loadListings();
    };

    // ===== Events =====
    function setupEvents() {
        if (window.MoltWallet) {
            window.moltWallet = window.moltWallet || new MoltWallet({ rpcUrl: RPC_URL });
            window.moltWallet.bindConnectButton('#connectWallet');
            window.moltWallet.onConnect(function (info) {
                currentWallet = info;
                updateNav();
                loadListings(); // reload + re-render to refresh wallet-dependent content
            });
            window.moltWallet.onDisconnect(function () {
                currentWallet = null;
                updateNav();
                loadListings();
            });
        }

        // Search
        var searchInput = document.getElementById('searchInput');
        var browseSearch = document.getElementById('browseSearch');

        function doSearch(q) {
            searchQuery = q;
            applyFilters();
            renderListings();
            updateResultCount();
        }

        if (searchInput) {
            searchInput.addEventListener('keypress', function (e) {
                if (e.key === 'Enter') doSearch(searchInput.value.trim());
            });
        }
        if (browseSearch) {
            browseSearch.addEventListener('input', function () {
                doSearch(browseSearch.value.trim());
            });
        }

        // Sort
        var sortSelect = document.getElementById('sortSelect');
        if (sortSelect) {
            sortSelect.addEventListener('change', function () {
                currentSort = sortSelect.value;
                loadListings();
            });
        }

        // View toggle
        var viewBtns = document.querySelectorAll('.view-btn');
        viewBtns.forEach(function (btn) {
            btn.addEventListener('click', function () {
                var view = btn.getAttribute('data-view');
                if (view) setView(view);
            });
        });

        // Price range filter
        var applyPriceBtn = document.getElementById('applyPriceBtn');
        if (applyPriceBtn) {
            applyPriceBtn.addEventListener('click', function () {
                var minEl = document.getElementById('minPrice');
                var maxEl = document.getElementById('maxPrice');
                priceMin = minEl && minEl.value ? parseFloat(minEl.value) : null;
                priceMax = maxEl && maxEl.value ? parseFloat(maxEl.value) : null;
                loadListings();
            });
        }

        var buyNowBox = document.getElementById('filterBuyNow');
        if (buyNowBox) {
            buyNowBox.addEventListener('change', function () {
                statusBuyNow = !!buyNowBox.checked;
                applyFilters();
                renderListings();
                updateResultCount();
            });
        }

        var hasOffersBox = document.getElementById('filterHasOffers');
        if (hasOffersBox) {
            hasOffersBox.addEventListener('change', function () {
                statusHasOffers = !!hasOffersBox.checked;
                loadListings();
            });
        }

        // Rarity filter
        var rarityBoxes = document.querySelectorAll('.rarityFilter');
        rarityBoxes.forEach(function (box) {
            box.addEventListener('change', function () {
                selectedRarities = [];
                document.querySelectorAll('.rarityFilter:checked').forEach(function (cb) {
                    selectedRarities.push(cb.value);
                });
                applyFilters();
                renderListings();
                updateResultCount();
            });
        });

        // Collection search (filter the radio list)
        var collectionSearch = document.getElementById('collectionSearch');
        if (collectionSearch) {
            collectionSearch.addEventListener('input', function () {
                var q = collectionSearch.value.trim().toLowerCase();
                var labels = document.querySelectorAll('#collectionsList label');
                labels.forEach(function (label) {
                    var text = label.textContent.toLowerCase();
                    label.style.display = (!q || text.indexOf(q) >= 0) ? '' : 'none';
                });
            });
        }

        // Mobile nav
        var navToggle = document.getElementById('navToggle');
        if (navToggle) {
            navToggle.addEventListener('click', function () {
                var navMenu = document.querySelector('.nav-menu');
                if (navMenu) navMenu.classList.toggle('active');
            });
        }

        if (typeof initMarketNetworkSelector === 'function') initMarketNetworkSelector();

        // Parse URL query params
        var params = new URLSearchParams(window.location.search);
        if (params.get('q')) {
            searchQuery = params.get('q');
            if (searchInput) searchInput.value = searchQuery;
            if (browseSearch) browseSearch.value = searchQuery;
        }
        if (params.get('collection')) {
            currentFilter = params.get('collection');
        }
        var filterMode = String(params.get('filter') || '').trim().toLowerCase();
        if (filterMode === 'featured' || filterMode === 'creators') {
            urlFilterMode = filterMode;
        }
    }

    // ===== Init =====
    document.addEventListener('DOMContentLoaded', function () {
        console.log('Molt Market Browse loading...');
        setupEvents();
        updateNav();
        loadCollections();
        loadListings();
        console.log('Molt Market Browse ready');
    });
})();
