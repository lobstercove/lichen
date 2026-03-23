// Lichen Market — Home Page
// Wallet-gated: Create CTA hidden when no wallet, Buy buttons require wallet

(function () {
    'use strict';

    var RPC_URL = (window.lichenMarketConfig && window.lichenMarketConfig.rpcUrl) || 'http://localhost:8899';
    var dataSource = window.marketplaceDataSource;
    var currentWallet = null;

    // Inline toast notification (avoids alert() for user feedback)
    function showMarketToast(msg, type) {
        var existing = document.getElementById('mkt-toast');
        if (existing) existing.remove();
        var el = document.createElement('div');
        el.id = 'mkt-toast';
        el.textContent = msg;
        el.style.cssText = 'position:fixed;top:20px;right:20px;padding:14px 24px;border-radius:8px;z-index:10000;font-size:14px;max-width:400px;box-shadow:0 4px 12px rgba(0,0,0,.3);transition:opacity .5s;'
            + (type === 'error' ? 'background:#FF4444;color:#fff;' : type === 'success' ? 'background:#00C853;color:#fff;' : 'background:#333;color:#fff;');
        document.body.appendChild(el);
        setTimeout(function () { el.style.opacity = '0'; setTimeout(function () { el.remove(); }, 600); }, 4000);
    }

    // ===== Nav Wallet Gating =====
    function updateNavForWallet() {
        // Hide Create link when no wallet
        var navMenuItems = document.querySelectorAll('.nav-menu li');
        navMenuItems.forEach(function (li) {
            var link = li.querySelector('a');
            if (link && link.getAttribute('href') === 'create.html') {
                li.style.display = currentWallet ? '' : 'none';
            }
        });
        // Profile link
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

    function updateHeroCTA() {
        var createCTA = document.querySelector('.hero-cta a[href="create.html"]');
        if (createCTA) {
            createCTA.style.display = currentWallet ? '' : 'none';
        }
    }

    // ===== Initialize =====
    document.addEventListener('DOMContentLoaded', function () {
        // Marketplace initializing

        loadFeaturedCollections();
        loadTrendingNFTs('24h');
        loadTopCreators();
        loadRecentSales();

        setupConnectWallet();
        setupSearch();
        setupFilterTabs();
        updateStats();
        updateNavForWallet();
        updateHeroCTA();

        if (typeof initMarketNetworkSelector === 'function') initMarketNetworkSelector();

        setInterval(updateStats, 15000);

        // Marketplace ready
    });

    // ===== Load Featured Collections =====
    async function loadFeaturedCollections() {
        var collections = [];
        try {
            if (dataSource) collections = await dataSource.getFeaturedCollections(6);
        } catch (err) {
            console.warn('Live collections unavailable:', err.message);
        }

        var container = document.getElementById('featuredCollections');
        if (!container) return;

        if (!collections || collections.length === 0) {
            container.innerHTML = '<div class="empty-state" style="grid-column: 1/-1; text-align: center; padding: 3rem; opacity: 0.6;"><i class="fas fa-images" style="font-size: 2rem; margin-bottom: 1rem; display: block;"></i>No collections yet. Be the first to create one!</div>';
            return;
        }

        container.innerHTML = collections.map(function (collection) {
            var collectionId = escapeHtml(collection.id);
            var banner = collection.banner ? escapeHtml(collection.banner) : escapeHtml(collection.image || '');
            var avatar = collection.avatar ? escapeHtml(collection.avatar) : '';
            return '<div class="collection-card" onclick="window.location.href=\'browse.html?collection=' + encodeURIComponent(collectionId) + '\'">' +
                '<div class="collection-banner" style="background: ' + banner + '"></div>' +
                '<div class="collection-avatar">' + avatar + '</div>' +
                '<div class="collection-info">' +
                '<div class="collection-name">' + escapeHtml(collection.name) + '</div>' +
                '<div class="collection-stats">' +
                '<div class="collection-stat"><div class="collection-stat-value">' + formatNumber(collection.items) + '</div><div class="collection-stat-label">Items</div></div>' +
                '<div class="collection-stat"><div class="collection-stat-value">' + escapeHtml(collection.floor) + '</div><div class="collection-stat-label">Floor</div></div>' +
                '<div class="collection-stat"><div class="collection-stat-value">' + formatNumber(collection.volume) + '</div><div class="collection-stat-label">Volume</div></div>' +
                '</div></div></div>';
        }).join('');
    }

    // ===== Load Trending NFTs =====
    async function loadTrendingNFTs(period) {
        var nfts = [];
        try {
            if (dataSource) nfts = await dataSource.getTrendingNFTs(8, period);
        } catch (err) {
            console.warn('Live trending NFTs unavailable:', err.message);
        }

        var container = document.getElementById('trendingNFTs');
        if (!container) return;

        if (!nfts || nfts.length === 0) {
            container.innerHTML = '<div class="empty-state" style="grid-column: 1/-1; text-align: center; padding: 3rem; opacity: 0.6;"><i class="fas fa-fire" style="font-size: 2rem; margin-bottom: 1rem; display: block;"></i>No trending NFTs yet</div>';
            return;
        }

        container.innerHTML = nfts.map(function (nft) {
            var buyBtnHtml = currentWallet
                ? '<button class="nft-action" onclick="event.stopPropagation(); window._homeBuyNFT(\'' + escapeJsAttr(nft.id) + '\')">Buy Now</button>'
                : '';
            return '<div class="nft-card" onclick="window.location.href=\'item.html?id=' + encodeURIComponent(nft.id) + '\'">' +
                '<div class="nft-image" style="background: ' + escapeHtml(nft.image) + '"></div>' +
                '<div class="nft-info">' +
                '<div class="nft-collection">' + escapeHtml(nft.collection) + '</div>' +
                '<div class="nft-name">' + escapeHtml(nft.name) + '</div>' +
                '<div class="nft-footer">' +
                '<div class="nft-price">Price <span class="nft-price-value">' + escapeHtml(nft.price) + ' LICN</span></div>' +
                buyBtnHtml +
                '</div></div></div>';
        }).join('');
    }

    // ===== Load Top Creators =====
    async function loadTopCreators() {
        var creators = [];
        try {
            if (dataSource) creators = await dataSource.getTopCreators(5);
        } catch (err) {
            console.warn('Live creators unavailable:', err.message);
        }

        var container = document.getElementById('topCreators');
        if (!container) return;

        if (!creators || creators.length === 0) {
            container.innerHTML = '<div class="empty-state" style="text-align: center; padding: 2rem; opacity: 0.6;">No creators yet</div>';
            return;
        }

        container.innerHTML = creators.slice(0, 5).map(function (creator) {
            var creatorId = creator.id ? escapeHtml(creator.id) : escapeHtml(creator.address || '');
            return '<div class="creator-card" onclick="window.location.href=\'profile.html?id=' + encodeURIComponent(creatorId) + '\'">' +
                '<div class="creator-avatar">' + escapeHtml(creator.avatar) + '</div>' +
                '<div class="creator-name">' + escapeHtml(creator.name) + '</div>' +
                '<div class="creator-sales">' + formatNumber(creator.sales) + ' sales</div>' +
                '</div>';
        }).join('');
    }

    // ===== Load Recent Sales =====
    async function loadRecentSales() {
        var sales = [];
        try {
            if (dataSource) sales = await dataSource.getRecentSales(10);
        } catch (err) {
            console.warn('Live sales unavailable:', err.message);
        }

        var tbody = document.getElementById('recentSales');
        if (!tbody) return;

        if (!sales || sales.length === 0) {
            tbody.innerHTML = '<tr><td colspan="6" style="text-align: center; padding: 2rem; opacity: 0.6;">No recent sales</td></tr>';
            return;
        }

        tbody.innerHTML = sales.map(function (sale) {
            var saleId = escapeHtml(sale.id);
            return '<tr onclick="window.location.href=\'item.html?id=' + encodeURIComponent(saleId) + '\'" style="cursor:pointer;">' +
                '<td><div class="sale-nft"><div class="sale-nft-image" style="background: ' + escapeHtml(sale.image) + '"></div><div><div class="sale-nft-name">' + escapeHtml(sale.nft) + '</div><div class="sale-nft-collection">' + escapeHtml(sale.collection) + '</div></div></div></td>' +
                '<td>' + escapeHtml(sale.collection) + '</td>' +
                '<td class="sale-price">' + escapeHtml(sale.price) + ' LICN</td>' +
                '<td><span class="sale-address" data-from="' + escapeHtml(sale.from) + '">' + formatHash(sale.from, 8) + '</span></td>' +
                '<td><span class="sale-address" data-to="' + escapeHtml(sale.to) + '">' + formatHash(sale.to, 8) + '</span></td>' +
                '<td class="sale-time">' + timeAgo(sale.timestamp) + '</td>' +
                '</tr>';
        }).join('');
    }

    // ===== Update Stats =====
    async function updateStats() {
        var stats = null;
        try {
            if (dataSource) stats = await dataSource.getStats();
        } catch (_) {}
        if (!stats) stats = { totalNFTs: 0, totalCollections: 0, totalVolume: 0, totalCreators: 0 };

        animateNumber('totalNFTs', stats.totalNFTs || 0);
        animateNumber('totalCollections', stats.totalCollections || 0);
        animateNumber('totalVolume', stats.totalVolume || 0);
        animateNumber('totalCreators', stats.totalCreators || 0);
    }

    // ===== Connect Wallet =====
    function setupConnectWallet() {
        if (window.LichenWallet) {
            window.lichenWallet = window.lichenWallet || new LichenWallet({ rpcUrl: RPC_URL, storageKey: 'marketWallets' });
            window.lichenWallet.bindConnectButton('#connectWallet');
            window.lichenWallet.onConnect(function (info) {
                currentWallet = info;
                updateNavForWallet();
                updateHeroCTA();
                // Reload trending to show/hide buy buttons
                loadTrendingNFTs('24h');
            });
            window.lichenWallet.onDisconnect(function () {
                currentWallet = null;
                updateNavForWallet();
                updateHeroCTA();
                loadTrendingNFTs('24h');
            });
            return;
        }
        console.warn('LichenWallet not loaded');
    }

    // ===== Search =====
    function setupSearch() {
        var searchInput = document.getElementById('searchInput');
        if (searchInput) {
            searchInput.addEventListener('keypress', function (e) {
                if (e.key === 'Enter') {
                    var query = searchInput.value.trim();
                    if (query) window.location.href = 'browse.html?q=' + encodeURIComponent(query);
                }
            });
        }
    }

    // ===== Filter Tabs =====
    function setupFilterTabs() {
        var tabs = document.querySelectorAll('.filter-tab');
        tabs.forEach(function (tab) {
            tab.addEventListener('click', function () {
                tabs.forEach(function (t) { t.classList.remove('active'); });
                tab.classList.add('active');
                loadTrendingNFTs(tab.dataset.period);
            });
        });
    }

    // ===== Buy from home page =====
    window._homeBuyNFT = function (id) {
        if (!currentWallet) {
            showMarketToast('Please connect your wallet first', 'error');
            return;
        }
        window.location.href = 'item.html?id=' + encodeURIComponent(id);
    };

    // ===== Utility =====
    function animateNumber(elementId, target, decimals) {
        var element = document.getElementById(elementId);
        if (!element) return;
        var current = parseInt(element.textContent.replace(/,/g, '')) || 0;
        var increment = (target - current) / 20;
        var value = current;
        var timer = setInterval(function () {
            value += increment;
            if ((increment > 0 && value >= target) || (increment < 0 && value <= target)) {
                value = target;
                clearInterval(timer);
            }
            element.textContent = (decimals || 0) > 0 ? value.toFixed(decimals) : Math.floor(value).toLocaleString();
        }, 50);
    }

    // ===== Mobile Menu Toggle =====
    var navToggle = document.getElementById('navToggle');
    if (navToggle) {
        navToggle.addEventListener('click', function () {
            var navMenu = document.querySelector('.nav-menu');
            if (navMenu) navMenu.classList.toggle('active');
        });
    }
})();
