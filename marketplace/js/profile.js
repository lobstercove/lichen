// Molt Market — User Profile Page
// Loads user NFTs, activity, stats from RPC; supports tabs and sorting

(function () {
    'use strict';

    const RPC_URL = (window.moltMarketConfig && window.moltMarketConfig.rpcUrl) || 'http://localhost:8899';

    // XSS prevention utility
    function escapeHtml(str) {
        return String(str ?? '')
            .replace(/&/g, '&amp;')
            .replace(/</g, '&lt;')
            .replace(/>/g, '&gt;')
            .replace(/"/g, '&quot;')
            .replace(/'/g, '&#39;');
    }

    // Safe image URL — only allow http(s) and ipfs protocols
    function safeImageUrl(url) {
        if (!url) return null;
        if (url.startsWith('ipfs://')) return url.replace('ipfs://', 'https://ipfs.io/ipfs/');
        if (url.startsWith('http://') || url.startsWith('https://')) return url;
        if (url.startsWith('linear-gradient')) return url;
        return null;
    }

    let currentWallet = null;
    let profileAddress = null;
    let isOwnProfile = false;
    let currentTab = 'collected';

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
        var half = Math.floor(length / 2);
        return hash.slice(0, half) + '...' + hash.slice(-half);
    }

    function timeAgo(ts) {
        var seconds = Math.floor((Date.now() - ts) / 1000);
        if (seconds < 60) return seconds + 's ago';
        if (seconds < 3600) return Math.floor(seconds / 60) + 'm ago';
        if (seconds < 86400) return Math.floor(seconds / 3600) + 'h ago';
        return Math.floor(seconds / 86400) + 'd ago';
    }

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
        return ts < 1_000_000_000_000 ? ts * 1000 : ts;
    }

    function priceToMolt(shells) {
        if (!shells) return '0.00';
        return (shells / 1_000_000_000).toFixed(2);
    }

    // ===== Profile Loading =====
    function getProfileId() {
        var params = new URLSearchParams(window.location.search);
        return params.get('id') || params.get('address');
    }

    async function loadProfile() {
        profileAddress = getProfileId();

        if (!profileAddress) {
            showEmptyProfile();
            return;
        }

        // Check if own profile
        isOwnProfile = currentWallet && currentWallet.address === profileAddress;

        // Load account info from RPC
        var account = null;
        try {
            account = await rpcCall('getAccountInfo', [profileAddress]);
        } catch (err) {
            console.warn('Failed to load account:', err);
        }

        // Render header
        renderProfileHeader(profileAddress, account);

        // Load stats
        loadStats(profileAddress);

        // Load initial tab
        switchTab('collected');

        // Show/hide edit buttons
        toggleEditButtons(isOwnProfile);
    }

    function renderProfileHeader(address, account) {
        // Banner
        var bannerImage = document.getElementById('bannerImage');
        if (bannerImage) {
            bannerImage.style.background = gradientFromHash(address + '-banner');
        }

        // Avatar
        var profileAvatar = document.getElementById('profileAvatar');
        if (profileAvatar) {
            var h = hashString(address);
            var emojis = ['🦞', '🦀', '🦐', '🐙', '🦑', '🐚', '🦈', '🐡'];
            profileAvatar.innerHTML = '<div style="display:flex;align-items:center;justify-content:center;width:100%;height:100%;font-size:48px;background:' +
                gradientFromHash(address + '-avatar') + ';border-radius:50%;">' + emojis[h % emojis.length] + '</div>';
        }

        // Name
        setText('profileName', account && account.name ? account.name : formatHash(address, 16));

        // Address
        var walletAddress = document.getElementById('walletAddress');
        if (walletAddress) walletAddress.textContent = address;

        // Bio
        setText('profileBio', account && account.bio
            ? account.bio
            : 'MoltChain NFT collector and creator.');
    }

    async function loadStats(address) {
        var owned = 0;
        var created = 0;
        var sold = 0;
        var volume = 0;

        // Load owned NFTs count
        try {
            var ownedResult = await rpcCall('getNFTsByOwner', [address, { limit: 1000 }]);
            owned = ownedResult && ownedResult.nfts ? ownedResult.nfts.length : (Array.isArray(ownedResult) ? ownedResult.length : 0);
        } catch (_) {
            // ignore
        }

        // Load sales for volume and sold count
        try {
            var salesResult = await rpcCall('getMarketSales', [{ limit: 500 }]);
            var sales = salesResult && salesResult.sales ? salesResult.sales : (Array.isArray(salesResult) ? salesResult : []);
            var mySales = sales.filter(function (s) { return s.seller === address; });
            sold = mySales.length;
            volume = mySales.reduce(function (acc, s) {
                return acc + (s.price_molt !== undefined ? Number(s.price_molt) : (s.price ? s.price / 1_000_000_000 : 0));
            }, 0);
        } catch (_) {
            // ignore
        }

        // Estimate created count from owned + sold activity
        created = owned; // approximation

        setText('nftCount', formatNumber(owned));
        setText('createdCount', formatNumber(created));
        setText('soldCount', formatNumber(sold));
        setText('volumeCount', formatNumber(Math.floor(volume)) + ' MOLT');
    }

    function toggleEditButtons(show) {
        var editProfile = document.getElementById('editProfileBtn');
        var editBanner = document.getElementById('editBanner');
        var editAvatar = document.getElementById('editAvatar');

        if (editProfile) editProfile.style.display = show ? '' : 'none';
        if (editBanner) editBanner.style.display = show ? '' : 'none';
        if (editAvatar) editAvatar.style.display = show ? '' : 'none';
    }

    // ===== Tabs =====
    function switchTab(tab) {
        currentTab = tab;

        // Update tab button states
        var tabButtons = document.querySelectorAll('[data-tab]');
        tabButtons.forEach(function (btn) {
            btn.classList.toggle('active', btn.dataset.tab === tab);
        });

        // Show/hide tab contents
        var tabs = ['collected', 'created', 'favorited', 'activity'];
        tabs.forEach(function (t) {
            var el = document.getElementById(t + '-tab');
            if (el) el.style.display = t === tab ? '' : 'none';
        });

        // Load tab data
        switch (tab) {
            case 'collected':
                loadCollectedNFTs();
                break;
            case 'created':
                loadCreatedNFTs();
                break;
            case 'favorited':
                loadFavoritedNFTs();
                break;
            case 'activity':
                loadActivity();
                break;
        }
    }

    // ===== Tab: Collected =====
    async function loadCollectedNFTs() {
        var grid = document.getElementById('collectedGrid');
        if (!grid) return;

        grid.innerHTML = '<div style="grid-column:1/-1;text-align:center;padding:40px;color:var(--text-secondary);"><i class="fas fa-spinner fa-spin" style="font-size:24px;"></i><p>Loading...</p></div>';

        var nfts = [];
        try {
            var result = await rpcCall('getNFTsByOwner', [profileAddress, { limit: 50 }]);
            nfts = result && result.nfts ? result.nfts : (Array.isArray(result) ? result : []);
        } catch (err) {
            console.warn('Failed to load collected NFTs:', err);
        }

        if (nfts.length === 0) {
            setText('collectedCount', 0);
            grid.innerHTML = '<div style="grid-column:1/-1;text-align:center;padding:40px;color:var(--text-secondary);">' +
                '<i class="fas fa-image" style="font-size:48px;margin-bottom:12px;opacity:0.3;"></i>' +
                '<p>No NFTs collected yet</p></div>';
            return;
        }

        // Sort
        var sortSelect = document.getElementById('collectedSort');
        var sortBy = sortSelect ? sortSelect.value : 'recent';
        nfts = sortNFTs(nfts, sortBy);

        setText('collectedCount', nfts.length);
        grid.innerHTML = renderNFTGrid(nfts);
    }

    // ===== Tab: Created =====
    async function loadCreatedNFTs() {
        var grid = document.getElementById('createdGrid');
        if (!grid) return;

        grid.innerHTML = '<div style="grid-column:1/-1;text-align:center;padding:40px;color:var(--text-secondary);"><i class="fas fa-spinner fa-spin" style="font-size:24px;"></i><p>Loading...</p></div>';

        // Query NFTs where creator matches
        var nfts = [];
        try {
            // Try fetching all market listings and filter by creator
            var listingsResult = await rpcCall('getMarketListings', [{ limit: 500 }]);
            var listings = listingsResult && listingsResult.listings ? listingsResult.listings : (Array.isArray(listingsResult) ? listingsResult : []);
            nfts = listings.filter(function (l) { return l.creator === profileAddress || l.owner === profileAddress; });
        } catch (_) {
            // ignore
        }

        if (nfts.length === 0) {
            setText('createdCountTab', 0);
            grid.innerHTML = '<div style="grid-column:1/-1;text-align:center;padding:40px;color:var(--text-secondary);">' +
                '<i class="fas fa-paint-brush" style="font-size:48px;margin-bottom:12px;opacity:0.3;"></i>' +
                '<p>No NFTs created yet</p></div>';
            return;
        }

        var sortSelect = document.getElementById('createdSort');
        var sortBy = sortSelect ? sortSelect.value : 'recent';
        nfts = sortNFTs(nfts, sortBy);

        setText('createdCountTab', nfts.length);
        grid.innerHTML = renderNFTGrid(nfts);
    }

    // ===== Tab: Favorited =====
    async function loadFavoritedNFTs() {
        var grid = document.getElementById('favoritedGrid');
        if (!grid) return;

        // Favorited is client-side only (no on-chain favorites yet)
        setText('favoritedCount', 0);
        grid.innerHTML = '<div style="grid-column:1/-1;text-align:center;padding:40px;color:var(--text-secondary);">' +
            '<i class="fas fa-heart" style="font-size:48px;margin-bottom:12px;opacity:0.3;"></i>' +
            '<p>No favorited NFTs yet</p></div>';
    }

    // ===== Tab: Activity =====
    async function loadActivity(filter) {
        var tbody = document.getElementById('activityTable');
        if (!tbody) return;

        tbody.innerHTML = '<tr><td colspan="6" style="text-align:center;padding:40px;color:var(--text-secondary);"><i class="fas fa-spinner fa-spin"></i> Loading...</td></tr>';

        var activity = [];

        // Load sales where user is buyer or seller
        try {
            var salesResult = await rpcCall('getMarketSales', [{ limit: 200 }]);
            var sales = salesResult && salesResult.sales ? salesResult.sales : (Array.isArray(salesResult) ? salesResult : []);
            var userSales = sales.filter(function (s) {
                return s.seller === profileAddress || s.buyer === profileAddress;
            });
            activity = activity.concat(userSales.map(function (s) {
                return {
                    type: s.seller === profileAddress ? 'sale' : 'purchase',
                    item: s.token_id !== undefined ? '#' + s.token_id : '#0',
                    collection: s.collection_name || s.collection || 'Unknown',
                    price: s.price_molt !== undefined ? Number(s.price_molt).toFixed(2) : priceToMolt(s.price || 0),
                    from: s.seller || '-',
                    to: s.buyer || '-',
                    timestamp: normalizeTimestamp(s.timestamp),
                    token: s.token,
                };
            }));
        } catch (_) {
            // ignore
        }

        // Load transfer history
        try {
            var txResult = await rpcCall('getTransactionsByAddress', [profileAddress, { limit: 50 }]);
            var txs = Array.isArray(txResult) ? txResult : (txResult && txResult.transactions ? txResult.transactions : []);
            txs.forEach(function (tx) {
                if (tx.type === 'transfer' || tx.method === 'transfer') {
                    activity.push({
                        type: tx.from === profileAddress ? 'sent' : 'received',
                        item: 'Transfer',
                        collection: '-',
                        price: priceToMolt(tx.amount || 0),
                        from: tx.from || '-',
                        to: tx.to || '-',
                        timestamp: normalizeTimestamp(tx.timestamp || tx.slot_time),
                    });
                }
            });
        } catch (_) {
            // ignore
        }

        if (activity.length === 0) {
            tbody.innerHTML = '<tr><td colspan="6" style="text-align:center;padding:40px;color:var(--text-secondary);">No activity yet</td></tr>';
            return;
        }

        // Apply filter
        if (filter && filter !== 'all') {
            activity = activity.filter(function (a) { return a.type === filter; });
        }

        // Sort by time descending
        activity.sort(function (a, b) { return (b.timestamp || 0) - (a.timestamp || 0); });

        if (activity.length === 0) {
            tbody.innerHTML = '<tr><td colspan="6" style="text-align:center;padding:40px;color:var(--text-secondary);">No activity found</td></tr>';
            return;
        }

        var eventIcons = { sale: '🔄', purchase: '🛒', sent: '➡️', received: '⬅️', listing: '📋', mint: '✨', transfer: '↔️' };

        tbody.innerHTML = activity.slice(0, 50).map(function (event) {
            var icon = eventIcons[event.type] || '📋';
            return '<tr onclick="' + (event.token ? "window._profileViewNFT('" + escapeHtml(event.token) + "')" : '') + '" style="cursor:pointer;">' +
                '<td>' + icon + ' ' + escapeHtml(event.type || '-') + '</td>' +
                '<td>' + escapeHtml(event.item || '-') + '</td>' +
                '<td>' + (event.price ? escapeHtml(event.price) + ' MOLT' : '-') + '</td>' +
                '<td><span title="' + escapeHtml(event.from || '') + '">' + formatHash(event.from, 8) + '</span></td>' +
                '<td><span title="' + escapeHtml(event.to || '') + '">' + formatHash(event.to, 8) + '</span></td>' +
                '<td>' + (event.timestamp ? timeAgo(event.timestamp) : '-') + '</td>' +
                '</tr>';
        }).join('');
    }

    // ===== NFT Grid Rendering =====
    function renderNFTGrid(nfts) {
        if (!nfts || nfts.length === 0) {
            return '<div style="grid-column:1/-1;text-align:center;padding:40px;color:var(--text-secondary);">' +
                '<i class="fas fa-image" style="font-size:48px;margin-bottom:12px;opacity:0.3;"></i>' +
                '<p>No NFTs found</p></div>';
        }

        return nfts.map(function (nft) {
            var imageUrl = nft.metadata_uri || nft.image;
            var safeUrl = safeImageUrl(imageUrl);
            var imageStyle = '';

            if (safeUrl && safeUrl.startsWith('linear-gradient')) {
                imageStyle = 'background: ' + safeUrl;
            } else if (safeUrl) {
                imageStyle = 'background-image: url(' + encodeURI(safeUrl) + '); background-size: cover; background-position: center;';
            } else {
                imageStyle = 'background: ' + gradientFromHash(nft.id || nft.token || 'x');
            }

            var price = nft.price_molt !== undefined ? Number(nft.price_molt).toFixed(2)
                : nft.price || priceToMolt(nft.price_shells || 0);
            var name = nft.name || (nft.token_id !== undefined ? '#' + nft.token_id : 'NFT');

            return '<div class="nft-card" onclick="window._profileViewNFT(\'' + escapeHtml(nft.id || nft.token || '') + '\')" style="cursor:pointer;">' +
                '<div class="nft-image" style="height:200px;border-radius:8px;' + imageStyle + '"></div>' +
                '<div class="nft-info" style="padding: 8px 0;">' +
                '<div class="nft-collection" style="font-size:12px;color:var(--text-secondary);">' + escapeHtml(nft.collection || nft.collection_name || 'Unknown') + '</div>' +
                '<div class="nft-name">' + escapeHtml(name) + '</div>' +
                '<div class="nft-price-value" style="font-size:14px;color:var(--accent-color);">' + escapeHtml(price) + ' MOLT</div>' +
                '</div>' +
                '</div>';
        }).join('');
    }

    function sortNFTs(nfts, sortBy) {
        var sorted = nfts.slice();
        switch (sortBy) {
            case 'price_low':
                sorted.sort(function (a, b) { return (parseFloat(a.price) || 0) - (parseFloat(b.price) || 0); });
                break;
            case 'price_high':
                sorted.sort(function (a, b) { return (parseFloat(b.price) || 0) - (parseFloat(a.price) || 0); });
                break;
            case 'name':
                sorted.sort(function (a, b) { return (a.name || '').localeCompare(b.name || ''); });
                break;
            case 'recent':
            default:
                sorted.sort(function (a, b) { return (normalizeTimestamp(b.timestamp) || 0) - (normalizeTimestamp(a.timestamp) || 0); });
                break;
        }
        return sorted;
    }

    // ===== Empty Profile =====
    function showEmptyProfile() {
        setText('profileName', 'Profile Not Found');
        setText('profileBio', 'No address specified. Please connect your wallet or visit a profile link.');
        var grid = document.getElementById('collectedGrid');
        if (grid) grid.innerHTML = '<div style="grid-column:1/-1;text-align:center;padding:40px;color:var(--text-secondary);">No profile loaded</div>';
    }

    // ===== Helpers =====
    function setText(id, value) {
        var el = document.getElementById(id);
        if (el) el.textContent = value;
    }

    function showToast(msg) {
        var toast = document.createElement('div');
        toast.style.cssText = 'position:fixed;bottom:24px;left:50%;transform:translateX(-50%);background:#333;color:#fff;padding:12px 24px;border-radius:8px;z-index:9999;font-size:14px;max-width:400px;text-align:center;box-shadow:0 4px 12px rgba(0,0,0,0.3);';
        toast.textContent = msg;
        document.body.appendChild(toast);
        setTimeout(function () { toast.remove(); }, 4000);
    }

    // ===== Copy Address =====
    function copyAddressToClipboard() {
        if (!profileAddress) return;
        if (navigator.clipboard) {
            navigator.clipboard.writeText(profileAddress).then(function () {
                var btn = document.querySelector('[onclick*="copyAddress"]');
                if (btn) {
                    var original = btn.innerHTML;
                    btn.innerHTML = '<i class="fas fa-check"></i>';
                    setTimeout(function () { btn.innerHTML = original; }, 2000);
                }
            });
        }
    }

    // ===== Event Setup =====
    function setupEvents() {
        // Tab buttons
        var tabButtons = document.querySelectorAll('[data-tab]');
        tabButtons.forEach(function (btn) {
            btn.addEventListener('click', function () {
                switchTab(btn.dataset.tab);
            });
        });

        // Activity filter buttons
        var filterButtons = document.querySelectorAll('[data-filter]');
        filterButtons.forEach(function (btn) {
            btn.addEventListener('click', function () {
                filterButtons.forEach(function (b) { b.classList.remove('active'); });
                btn.classList.add('active');
                loadActivity(btn.dataset.filter);
            });
        });

        // Sort selects
        ['collectedSort', 'createdSort', 'favoritedSort'].forEach(function (id) {
            var select = document.getElementById(id);
            if (select) {
                select.addEventListener('change', function () {
                    switchTab(currentTab);
                });
            }
        });

        // Edit buttons
        var editProfile = document.getElementById('editProfileBtn');
        if (editProfile) {
            editProfile.addEventListener('click', function () {
                showToast('Profile editing requires wallet signature');
            });
        }
        var editBanner = document.getElementById('editBanner');
        if (editBanner) {
            editBanner.addEventListener('click', function () {
                showToast('Banner editing requires wallet signature');
            });
        }
        var editAvatar = document.getElementById('editAvatar');
        if (editAvatar) {
            editAvatar.addEventListener('click', function () {
                showToast('Avatar editing requires wallet signature');
            });
        }

        // Use shared wallet manager
        if (window.MoltWallet) {
            window.moltWallet = window.moltWallet || new MoltWallet({ rpcUrl: RPC_URL });
            window.moltWallet.bindConnectButton('#connectWallet');
            window.moltWallet.onConnect(function(info) {
                currentWallet = info;

                // Check if now viewing own profile
                isOwnProfile = profileAddress === currentWallet.address;
                toggleEditButtons(isOwnProfile);

                // If no profile loaded, redirect to own profile
                if (!profileAddress) {
                    window.location.href = 'profile.html?id=' + encodeURIComponent(currentWallet.address);
                }
            });
            window.moltWallet.onDisconnect(function() {
                currentWallet = null;
                isOwnProfile = false;
                toggleEditButtons(false);
            });
        }

        // Search
        var searchInput = document.getElementById('searchInput');
        if (searchInput) {
            searchInput.addEventListener('keypress', function (e) {
                if (e.key === 'Enter') {
                    var q = searchInput.value.trim();
                    if (q) window.location.href = 'browse.html?q=' + encodeURIComponent(q);
                }
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
    }

    // ===== Public API =====
    window.copyAddress = copyAddressToClipboard;
    window._profileViewNFT = function (id) {
        window.location.href = 'item.html?id=' + encodeURIComponent(id);
    };

    // ===== Init =====
    document.addEventListener('DOMContentLoaded', function () {
        console.log('🦞 Molt Market Profile loading...');
        if (typeof initMarketNetworkSelector === 'function') initMarketNetworkSelector();
        setupEvents();
        loadProfile();
        console.log('✅ Molt Market Profile ready');
    });
})();
