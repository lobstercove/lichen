// Lichen Market — Profile Page
// Wallet-gated, sell/list buttons on owned NFTs, collections management, activity

(function () {
    'use strict';

    var RPC_URL = (window.lichenMarketConfig && window.lichenMarketConfig.rpcUrl) || (typeof LICHEN_CONFIG !== 'undefined' && typeof LICHEN_CONFIG.rpc === 'function' ? LICHEN_CONFIG.rpc() : 'https://rpc.lichen.network');
    var CONTRACT_PROGRAM_ID = null;
    var dataSource = window.marketplaceDataSource;
    var currentWallet = null;
    var profileAddress = '';
    var isOwnProfile = false;
    var ownedNFTs = [];
    var createdNFTs = [];
    var favoritedNFTs = [];
    var allListings = [];
    var marketplaceProgram = null;
    var FAVORITES_STORAGE_KEY = 'lichenmarket_favorites_v1';
    var marketTrustedRpcCall = window.marketTrustedRpcCall || rpcCall;

    var fmp = (window.marketplaceUtils && window.marketplaceUtils.formatLicnPrice) || function (v, isLicn) { var n = Number(isLicn ? v : v / 1e9); if (n >= 0.01) return n.toFixed(2); if (n >= 0.0001) return n.toFixed(4); if (n >= 0.000001) return n.toFixed(6); if (n > 0) return n.toFixed(9); return '0'; };

    function lazyAddresses() {
        return;
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

    // Generate a multi-stop gradient banner from wallet address (richer than single gradient)
    function bannerGradientFromHash(seed) {
        var base = hashString(seed);
        function c(n) { return '#' + ((n & 0xff0000) >> 16).toString(16).padStart(2, '0') + ((n & 0x00ff00) >> 8).toString(16).padStart(2, '0') + (n & 0x0000ff).toString(16).padStart(2, '0'); }
        var angle = (base % 360);
        var c1 = c(hashString(base + '-b1'));
        var c2 = c(hashString(base + '-b2'));
        var c3 = c(hashString(base + '-b3'));
        return 'linear-gradient(' + angle + 'deg, ' + c1 + ' 0%, ' + c2 + ' 50%, ' + c3 + ' 100%)';
    }

    // Generate a deterministic SVG identicon (blockie-style 5x5 grid) from wallet address
    function generateIdenticon(address) {
        var h = hashString(address);
        function hc(n) { return '#' + ((n & 0xff0000) >> 16).toString(16).padStart(2, '0') + ((n & 0x00ff00) >> 8).toString(16).padStart(2, '0') + (n & 0x0000ff).toString(16).padStart(2, '0'); }
        var bgColor = hc(hashString(h + '-bg'));
        var fgColor = hc(hashString(h + '-fg'));
        var accentColor = hc(hashString(h + '-ac'));

        // 5x5 grid, mirrored horizontally (only need 3 columns)
        var grid = [];
        for (var row = 0; row < 5; row++) {
            grid[row] = [];
            for (var col = 0; col < 3; col++) {
                var bit = hashString(h + '-' + row + '-' + col);
                grid[row][col] = bit % 3; // 0=bg, 1=fg, 2=accent
            }
            // Mirror: col 3 = col 1, col 4 = col 0
            grid[row][3] = grid[row][1];
            grid[row][4] = grid[row][0];
        }

        var size = 5;
        var cellSize = 20;
        var svgSize = size * cellSize;
        var rects = '';
        for (var r = 0; r < size; r++) {
            for (var c = 0; c < size; c++) {
                var color = grid[r][c] === 0 ? bgColor : (grid[r][c] === 1 ? fgColor : accentColor);
                rects += '<rect x="' + (c * cellSize) + '" y="' + (r * cellSize) + '" width="' + cellSize + '" height="' + cellSize + '" fill="' + color + '"/>';
            }
        }

        return '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ' + svgSize + ' ' + svgSize + '" style="width:100%;height:100%;border-radius:50%;">' + rects + '</svg>';
    }

    function safeImageUrl(imageUrl) {
        if (!imageUrl || typeof imageUrl !== 'string') return null;
        var url = imageUrl.trim();
        if (!url) return null;
        if (url.indexOf('ipfs://') === 0) {
            return 'https://ipfs.io/ipfs/' + url.slice('ipfs://'.length);
        }
        if (url.indexOf('http://') === 0 || url.indexOf('https://') === 0) {
            return url;
        }
        if (url.indexOf('linear-gradient') === 0) {
            return url;
        }
        return null;
    }

    function showToast(msg, type) {
        var bg = type === 'error' ? '#ef4444' : type === 'success' ? '#22c55e' : '#3b82f6';
        var toast = document.createElement('div');
        toast.style.cssText = 'position:fixed;bottom:24px;left:50%;transform:translateX(-50%);background:' + bg + ';color:#fff;padding:12px 24px;border-radius:8px;z-index:9999;font-size:14px;max-width:500px;text-align:center;box-shadow:0 4px 12px rgba(0,0,0,0.3);';
        toast.textContent = msg;
        document.body.appendChild(toast);
        setTimeout(function () { toast.remove(); }, 5000);
    }

    function getItemHref(nft) {
        return 'item.html?id=' + encodeURIComponent(nft.id || '') +
            '&contract=' + encodeURIComponent(nft.collection || nft.contract_id || '') +
            '&token=' + encodeURIComponent(nft.token_id || '');
    }

    function buildContractCallData(functionName, args, value) {
        var argBytes = Array.from(new TextEncoder().encode(JSON.stringify(args || [])));
        return JSON.stringify({ Call: { function: functionName, args: argBytes, value: value || 0 } });
    }

    function readFavoriteStore() {
        try {
            var raw = localStorage.getItem(FAVORITES_STORAGE_KEY);
            return raw ? (JSON.parse(raw) || {}) : {};
        } catch (_) {
            return {};
        }
    }

    async function resolveMarketplaceProgram() {
        if (marketplaceProgram) return marketplaceProgram;
        try {
            var entry = await marketTrustedRpcCall('getSymbolRegistry', ['LICHENMARKET']);
            marketplaceProgram = entry && (entry.program || entry.program_id) ? (entry.program || entry.program_id) : null;
            if (marketplaceProgram) CONTRACT_PROGRAM_ID = marketplaceProgram;
        } catch (_) { }
        return marketplaceProgram;
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
                li.innerHTML = '<a href="profile.html?id=' + encodeURIComponent(currentWallet.address) + '" class="active">Profile</a>';
                navMenu.appendChild(li);
            } else {
                existing.querySelector('a').href = 'profile.html?id=' + encodeURIComponent(currentWallet.address);
            }
        } else {
            if (existing) existing.remove();
        }
    }

    // ===== Load Profile =====
    async function loadProfile() {
        var params = new URLSearchParams(window.location.search);
        profileAddress = params.get('id') || (currentWallet ? currentWallet.address : '');
        isOwnProfile = currentWallet && profileAddress === currentWallet.address;

        if (!profileAddress) {
            var nameEl = document.getElementById('profileName');
            if (nameEl) nameEl.textContent = 'Connect Wallet to View Profile';
            return;
        }

        // Update header
        var nameEl = document.getElementById('profileName');
        if (nameEl) nameEl.textContent = formatHash(profileAddress, 14);

        var addrEl = document.getElementById('walletAddress');
        if (addrEl) addrEl.textContent = profileAddress;

        var bannerEl = document.getElementById('bannerImage');
        if (bannerEl) bannerEl.style.background = bannerGradientFromHash(profileAddress);

        var avatarEl = document.getElementById('profileAvatar');
        if (avatarEl) {
            avatarEl.innerHTML = generateIdenticon(profileAddress);
        }

        // Balance if own profile
        if (isOwnProfile && dataSource) {
            var balance = await dataSource.getWalletBalance(profileAddress);
            var bioEl = document.getElementById('profileBio');
            if (bioEl) bioEl.textContent = 'Balance: ' + balance.toFixed(4) + ' LICN';
        }

        // Load listings for sell button matching
        try {
            allListings = await dataSource.getAllListings(500);
        } catch (_) { allListings = []; }

        loadCollectedNFTs();
        loadCreatedNFTs();
        loadFavoritedNFTs();
        loadProfileOffers();
        loadActivity();
    }

    // ===== Collected NFTs =====
    async function loadCollectedNFTs() {
        if (!profileAddress) return;
        var grid = document.getElementById('collectedGrid');
        if (!grid) return;

        grid.innerHTML = '<div style="grid-column:1/-1;text-align:center;padding:32px;opacity:0.5;"><i class="fas fa-spinner fa-spin" style="font-size:24px;"></i></div>';

        try {
            if (dataSource) {
                ownedNFTs = await dataSource.getNFTsByOwner(profileAddress);
            }
        } catch (_) {
            ownedNFTs = [];
        }

        var countEl = document.getElementById('collectedCount');
        if (countEl) countEl.textContent = ownedNFTs.length;
        var nftCountEl = document.getElementById('nftCount');
        if (nftCountEl) nftCountEl.textContent = ownedNFTs.length;

        if (ownedNFTs.length === 0) {
            grid.innerHTML = '<div style="grid-column:1/-1;text-align:center;padding:48px;opacity:0.5;">' +
                '<i class="fas fa-images" style="font-size:48px;margin-bottom:16px;display:block;"></i>' +
                '<h3>No NFTs collected yet</h3></div>';
            return;
        }

        grid.innerHTML = ownedNFTs.map(function (nft, idx) {
            var imageUrl = nft.image || '';
            var safeUrl = safeImageUrl(imageUrl);
            var imgHtml = safeUrl
                ? '<img src="' + encodeURI(safeUrl) + '" style="width:100%;height:100%;object-fit:cover;" alt="">'
                : '<div style="width:100%;height:100%;background:' + gradientFromHash(nft.id || nft.name || '') + ';display:flex;align-items:center;justify-content:center;font-size:36px;opacity:0.5;">\uD83D\uDDBC\uFE0F</div>';

            // Check of this NFT is currently listed
            var listing = allListings.find(function (l) {
                return (l.nft_contract === (nft.collection || nft.contract_id) && String(l.token_id) === String(nft.token_id)) ||
                    l.nft_id === nft.id;
            });
            var isListed = listing && listing.active !== false;

            var price = nft.price ? fmp(nft.price > 1e6 ? nft.price / 1e9 : nft.price, true) + ' LICN' : (isListed ? fmp(listing.price > 1e6 ? listing.price / 1e9 : listing.price, true) + ' LICN' : 'Not Listed');
            var name = nft.name || ('NFT #' + (nft.token_id || ''));

            var actionHtml = '';
            if (isOwnProfile) {
                if (isListed) {
                    actionHtml = '<div style="display:flex;gap:6px;">' +
                        '<button class="nft-action" data-profile-action="update-price" data-profile-index="' + idx + '" style="background:var(--accent-color);color:white;border:none;padding:6px 10px;border-radius:6px;font-size:12px;cursor:pointer;">Update Price</button>' +
                        '<button class="nft-action" data-profile-action="cancel-listing" data-profile-index="' + idx + '" style="background:#ef4444;color:white;border:none;padding:6px 10px;border-radius:6px;font-size:12px;cursor:pointer;">Cancel Listing</button>' +
                        '</div>';
                } else {
                    actionHtml = '<div style="display:flex;gap:6px;">' +
                        '<button class="nft-action" data-profile-action="list-nft" data-profile-index="' + idx + '" style="background:var(--accent-color);color:white;border:none;padding:6px 12px;border-radius:6px;font-size:12px;cursor:pointer;">List for Sale</button>' +
                        '<button class="nft-action" data-profile-action="create-auction" data-profile-index="' + idx + '" style="background:var(--bg-tertiary);color:var(--text-primary);border:none;padding:6px 10px;border-radius:6px;font-size:12px;cursor:pointer;">Create Auction</button>' +
                        '</div>';
                }
            } else {
                actionHtml = '<button class="nft-action" data-profile-action="place-bid" data-profile-index="' + idx + '" style="background:var(--accent-color);color:white;border:none;padding:6px 10px;border-radius:6px;font-size:12px;cursor:pointer;">Place Bid</button>';
            }

            return '<div class="nft-card" data-profile-href="' + escapeHtml(getItemHref(nft)) + '">' +
                '<div class="nft-image">' + imgHtml + '</div>' +
                '<div class="nft-info">' +
                '<div class="nft-collection">' + escapeHtml(nft.collection || nft.collection_name || 'Unknown') + '</div>' +
                '<div class="nft-name">' + escapeHtml(name) + '</div>' +
                '<div class="nft-footer">' +
                '<div class="nft-price"><span class="nft-price-value">' + escapeHtml(price) + '</span></div>' +
                actionHtml +
                '</div></div></div>';
        }).join('');
    }

    // ===== Created NFTs =====
    async function loadCreatedNFTs() {
        if (!profileAddress) return;
        var grid = document.getElementById('createdGrid');
        if (!grid) return;

        grid.innerHTML = '<div style="grid-column:1/-1;text-align:center;padding:32px;opacity:0.5;"><i class="fas fa-spinner fa-spin" style="font-size:24px;"></i></div>';

        try {
            if (dataSource) {
                var collections = await dataSource.getUserCollections(profileAddress);
                createdNFTs = [];
                for (var i = 0; i < collections.length; i++) {
                    var colNFTs = await dataSource.getNFTsByCollection(collections[i].id, 50);
                    createdNFTs = createdNFTs.concat(colNFTs);
                }
            }
        } catch (_) {
            createdNFTs = [];
        }

        var createdCountEl = document.getElementById('createdCount');
        if (createdCountEl) createdCountEl.textContent = createdNFTs.length;
        var createdCountTab = document.getElementById('createdCountTab');
        if (createdCountTab) createdCountTab.textContent = createdNFTs.length;

        if (createdNFTs.length === 0) {
            grid.innerHTML = '<div style="grid-column:1/-1;text-align:center;padding:48px;opacity:0.5;">' +
                '<i class="fas fa-palette" style="font-size:48px;margin-bottom:16px;display:block;"></i>' +
                '<h3>No NFTs created yet</h3>' +
                (isOwnProfile ? '<p style="margin-top:8px;"><a href="create.html" style="color:var(--accent-color);">Create your first NFT</a></p>' : '') +
                '</div>';
            return;
        }

        grid.innerHTML = createdNFTs.map(function (nft, idx) {
            var imageUrl = nft.image || '';
            var safeUrl = safeImageUrl(imageUrl);
            var imgHtml = safeUrl
                ? '<img src="' + encodeURI(safeUrl) + '" style="width:100%;height:100%;object-fit:cover;" alt="">'
                : '<div style="width:100%;height:100%;background:' + gradientFromHash(nft.id || nft.name || '') + ';display:flex;align-items:center;justify-content:center;font-size:36px;opacity:0.5;">\uD83D\uDDBC\uFE0F</div>';
            var price = nft.price ? fmp(nft.price > 1e6 ? nft.price / 1e9 : nft.price, true) + ' LICN' : 'Not Listed';
            var name = nft.name || ('NFT #' + (nft.token_id || ''));

            return '<div class="nft-card" data-profile-href="' + escapeHtml(getItemHref(nft)) + '">' +
                '<div class="nft-image">' + imgHtml + '</div>' +
                '<div class="nft-info">' +
                '<div class="nft-collection">' + escapeHtml(nft.collection || nft.collection_name || 'Unknown') + '</div>' +
                '<div class="nft-name">' + escapeHtml(name) + '</div>' +
                '<div class="nft-footer">' +
                '<div class="nft-price"><span class="nft-price-value">' + escapeHtml(price) + '</span></div>' +
                (isOwnProfile ? '<button class="nft-action" data-profile-action="collection-offer" data-profile-index="' + idx + '" style="background:var(--bg-tertiary);color:var(--text-primary);border:none;padding:6px 10px;border-radius:6px;font-size:12px;cursor:pointer;">Collection Offer</button>' : '') +
                '</div></div></div>';
        }).join('');
    }

    // ===== Favorited NFTs =====
    function loadFavoritedNFTs() {
        var grid = document.getElementById('favoritedGrid');
        if (!grid) return;
        favoritedNFTs = [];

        if (!currentWallet || !isOwnProfile) {
            var countElNone = document.getElementById('favoritedCount');
            if (countElNone) countElNone.textContent = '0';
            grid.innerHTML = '<div style="grid-column:1/-1;text-align:center;padding:48px;opacity:0.5;">' +
                '<i class="fas fa-heart" style="font-size:48px;margin-bottom:16px;display:block;"></i>' +
                '<h3>Connect your wallet to view favorites</h3></div>';
            return;
        }

        grid.innerHTML = '<div style="grid-column:1/-1;text-align:center;padding:32px;opacity:0.5;"><i class="fas fa-spinner fa-spin" style="font-size:24px;"></i></div>';

        (async function () {
            var store = readFavoriteStore();
            var list = Array.isArray(store[currentWallet.address]) ? store[currentWallet.address] : [];

            for (var i = 0; i < list.length; i++) {
                var fav = list[i] || {};
                var nft = null;
                try {
                    if (fav.collection && fav.token_id !== undefined && fav.token_id !== null && String(fav.token_id) !== '') {
                        nft = await rpcCall('getNFT', [fav.collection, String(fav.token_id)]);
                    }
                    if (!nft && fav.id) {
                        nft = await rpcCall('getNFT', [fav.id]);
                    }
                } catch (_) { }

                if (!nft) continue;
                if (fav.added_at) nft.favorited_at = fav.added_at;
                favoritedNFTs.push(nft);
            }

            var countEl = document.getElementById('favoritedCount');
            if (countEl) countEl.textContent = favoritedNFTs.length;

            if (favoritedNFTs.length === 0) {
                grid.innerHTML = '<div style="grid-column:1/-1;text-align:center;padding:48px;opacity:0.5;">' +
                    '<i class="fas fa-heart" style="font-size:48px;margin-bottom:16px;display:block;"></i>' +
                    '<h3>No favorites yet</h3>' +
                    '<p style="margin-top:8px;opacity:0.7;">Favorite NFTs from item pages to see them here.</p></div>';
                return;
            }

            renderFavoritedGrid();
        })();
    }

    function renderFavoritedGrid() {
        var grid = document.getElementById('favoritedGrid');
        if (!grid) return;

        grid.innerHTML = favoritedNFTs.map(function (nft) {
            var imageUrl = nft.image || '';
            var safeUrl = safeImageUrl(imageUrl);
            var imgHtml = safeUrl
                ? '<img src="' + encodeURI(safeUrl) + '" style="width:100%;height:100%;object-fit:cover;" alt="">'
                : '<div style="width:100%;height:100%;background:' + gradientFromHash(nft.id || nft.name || '') + ';display:flex;align-items:center;justify-content:center;font-size:36px;opacity:0.5;">\uD83D\uDDBC\uFE0F</div>';

            var price = nft.price ? fmp(nft.price > 1e6 ? nft.price / 1e9 : nft.price, true) + ' LICN' : 'Not Listed';
            var name = nft.name || ('NFT #' + (nft.token_id || ''));

            return '<div class="nft-card" data-profile-href="' + escapeHtml(getItemHref(nft)) + '">' +
                '<div class="nft-image">' + imgHtml + '</div>' +
                '<div class="nft-info">' +
                '<div class="nft-collection">' + escapeHtml(nft.collection || nft.collection_name || 'Unknown') + '</div>' +
                '<div class="nft-name">' + escapeHtml(name) + '</div>' +
                '<div class="nft-footer">' +
                '<div class="nft-price"><span class="nft-price-value">' + escapeHtml(price) + '</span></div>' +
                '</div></div></div>';
        }).join('');
    }

    // ===== Offers =====
    async function loadProfileOffers() {
        var tbody = document.getElementById('offersTable');
        if (!tbody || !profileAddress) return;

        tbody.innerHTML = '<tr><td colspan="6" style="text-align:center;padding:32px;opacity:0.5;"><i class="fas fa-spinner fa-spin"></i> Loading...</td></tr>';

        try {
            var result = await rpcCall('getMarketOffers', [{ include_collection_offers: true, limit: 200 }]);
            var offerList = result && result.offers ? result.offers : (Array.isArray(result) ? result : []);

            // Split into incoming (to my NFTs) and outgoing (I made)
            var incoming = offerList.filter(function (o) {
                return o.nft_owner === profileAddress || o.seller === profileAddress;
            });
            var outgoing = offerList.filter(function (o) {
                return o.offerer === profileAddress || o.buyer === profileAddress;
            });
            var allOffers = incoming.map(function (o) { o._dir = 'incoming'; return o; })
                .concat(outgoing.map(function (o) { o._dir = 'outgoing'; return o; }));

            // Dedup by id
            var seen = {};
            allOffers = allOffers.filter(function (o) {
                var k = o.id || (o.offerer + '-' + o.token_id);
                if (seen[k]) return false;
                seen[k] = true;
                return true;
            });

            if (allOffers.length === 0) {
                tbody.innerHTML = '<tr><td colspan="6" style="text-align:center;padding:32px;opacity:0.5;">No offers yet</td></tr>';
                return;
            }

            tbody.innerHTML = allOffers.map(function (offer) {
                var dir = offer._dir;
                var dirLabel = dir === 'incoming' ? 'Incoming' : 'Outgoing';
                var dirColor = dir === 'incoming' ? '#22c55e' : '#3b82f6';
                var nftName = offer.name || formatHash(offer.token_id || offer.nft_id || '', 8);
                var price = offer.price_licn !== undefined ? fmp(offer.price_licn, true) : fmp(offer.price || 0, false);
                var from = offer.offerer || offer.buyer || '';
                var expired = offer.expires_at && Date.now() > (offer.expires_at < 1e12 ? offer.expires_at * 1000 : offer.expires_at);
                var statusLabel = expired ? 'Expired' : (offer.status || 'Active');
                var statusColor = expired ? '#ef4444' : '#22c55e';

                var actionHtml = '';
                if (dir === 'incoming' && isOwnProfile && !expired) {
                    actionHtml = '<button data-profile-action="accept-offer" data-offerer="' + escapeHtml(offer.offerer || offer.buyer || '') + '" data-nft-contract="' + escapeHtml(offer.nft_contract || offer.collection || '') + '" data-token-id="' + escapeHtml(String(offer.token_id || '')) + '" style="background:var(--accent-color);color:white;border:none;padding:4px 10px;border-radius:4px;font-size:12px;cursor:pointer;">Accept</button>';
                } else if (dir === 'outgoing' && isOwnProfile && !expired) {
                    actionHtml = '<button data-profile-action="cancel-offer" data-nft-contract="' + escapeHtml(offer.nft_contract || offer.collection || '') + '" data-token-id="' + escapeHtml(String(offer.token_id || '')) + '" style="background:#ef4444;color:white;border:none;padding:4px 10px;border-radius:4px;font-size:12px;cursor:pointer;">Cancel</button>';
                }

                return '<tr data-dir="' + dir + '">' +
                    '<td><span style="padding:3px 8px;border-radius:4px;background:' + dirColor + '22;color:' + dirColor + ';font-size:12px;">' + dirLabel + '</span></td>' +
                    '<td>' + escapeHtml(nftName) + '</td>' +
                    '<td>' + price + ' LICN</td>' +
                    '<td><a href="profile.html?id=' + encodeURIComponent(from) + '" style="color:var(--accent-color);">' + formatHash(from, 8) + '</a></td>' +
                    '<td><span style="color:' + statusColor + ';">' + statusLabel + '</span></td>' +
                    '<td>' + actionHtml + '</td>' +
                    '</tr>';
            }).join('');

        } catch (err) {
            tbody.innerHTML = '<tr><td colspan="6" style="text-align:center;padding:32px;opacity:0.5;">Failed to load offers</td></tr>';
        }
    }

    // ===== Accept Offer (from profile) =====
    window._profileAcceptOffer = async function (offerer, nftContract, tokenId) {
        lazyAddresses();
        try {
            var mp = await resolveMarketplaceProgram();
            if (!mp) throw new Error('Cannot resolve marketplace program');

            var callData = buildContractCallData('accept_offer', [
                currentWallet.address,
                nftContract,
                tokenId,
                offerer
            ], 0);

            await window.lichenWallet.sendTransaction([{
                program_id: CONTRACT_PROGRAM_ID,
                accounts: [currentWallet.address, mp, nftContract],
                data: callData,
            }]);

            showToast('Offer accepted!', 'success');
            loadProfileOffers();
            loadCollectedNFTs();
        } catch (err) {
            showToast('Accept failed: ' + err.message, 'error');
        }
    };

    // ===== Cancel Offer (from profile) =====
    window._profileCancelOffer = async function (nftContract, tokenId) {
        lazyAddresses();
        try {
            var mp = await resolveMarketplaceProgram();
            if (!mp) throw new Error('Cannot resolve marketplace program');

            var callData = buildContractCallData('cancel_offer', [
                currentWallet.address,
                nftContract,
                tokenId
            ], 0);

            await window.lichenWallet.sendTransaction([{
                program_id: CONTRACT_PROGRAM_ID,
                accounts: [currentWallet.address, mp, nftContract],
                data: callData,
            }]);

            showToast('Offer cancelled', 'success');
            loadProfileOffers();
        } catch (err) {
            showToast('Cancel failed: ' + err.message, 'error');
        }
    };

    // ===== Activity =====
    async function loadActivity() {
        var tbody = document.getElementById('activityTable');
        if (!tbody || !profileAddress) return;

        tbody.innerHTML = '<tr><td colspan="6" style="text-align:center;padding:32px;opacity:0.5;"><i class="fas fa-spinner fa-spin"></i> Loading...</td></tr>';

        try {
            var sales = await rpcCall('getMarketSales', [{ limit: 200 }]);
            var saleList = sales && sales.sales ? sales.sales : (Array.isArray(sales) ? sales : []);

            var activity = saleList.filter(function (s) {
                return s.seller === profileAddress || s.buyer === profileAddress;
            });

            var soldCount = 0;
            var totalVolume = 0;
            activity.forEach(function (s) {
                if (s.seller === profileAddress) {
                    soldCount++;
                    totalVolume += s.price_licn !== undefined ? Number(s.price_licn) : (s.price ? s.price / 1e9 : 0);
                }
            });

            var soldEl = document.getElementById('soldCount');
            if (soldEl) soldEl.textContent = soldCount;
            var volEl = document.getElementById('volumeCount');
            if (volEl) volEl.textContent = fmp(totalVolume, true);

            if (activity.length === 0) {
                tbody.innerHTML = '<tr><td colspan="6" style="text-align:center;padding:32px;opacity:0.5;">No activity yet</td></tr>';
                return;
            }

            tbody.innerHTML = activity.slice(0, 50).map(function (event) {
                var type = event.seller === profileAddress ? 'Sale' : 'Purchase';
                var tokenRef = event.token_id || event.nft_id || event.token || event.id || '';
                var eventItem = event.name || ('NFT #' + String(tokenRef || '-'));
                var eventFrom = event.seller || '';
                var eventTo = event.buyer || '';
                var price = event.price_licn !== undefined ? fmp(event.price_licn, true) : fmp(event.price || 0, false);
                var ts = event.timestamp ? new Date(event.timestamp < 1e12 ? event.timestamp * 1000 : event.timestamp).toLocaleDateString() : '-';

                return '<tr data-type="' + type.toLowerCase() + '">' +
                    '<td><span style="padding:4px 8px;border-radius:4px;background:' + (type === 'Sale' ? '#22c55e22' : '#3b82f622') + ';color:' + (type === 'Sale' ? '#22c55e' : '#3b82f6') + ';font-size:12px;">' + type + '</span></td>' +
                    '<td><a href="item.html?id=' + encodeURIComponent(tokenRef) + '">' + escapeHtml(String(tokenRef || '-')) + '</a> ' + escapeHtml(eventItem) + '</td>' +
                    '<td>' + price + ' LICN</td>' +
                    '<td><a href="profile.html?id=' + encodeURIComponent(eventFrom) + '" style="color:var(--accent-color);">' + formatHash(eventFrom, 8) + '</a></td>' +
                    '<td><a href="profile.html?id=' + encodeURIComponent(eventTo) + '" style="color:var(--accent-color);">' + formatHash(eventTo, 8) + '</a></td>' +
                    '<td style="display:none;">' + escapeHtml(type) + '</td>' +
                    '<td>' + ts + '</td>' +
                    '</tr>';
            }).join('');

        } catch (err) {
            tbody.innerHTML = '<tr><td colspan="6" style="text-align:center;padding:32px;opacity:0.5;">Failed to load activity</td></tr>';
        }
    }

    // ===== List NFT for Sale (from profile) =====
    window._profileListNFT = function (index) {
        var nft = ownedNFTs[index];
        if (!nft || !isOwnProfile) return;

        var price = prompt('Enter listing price in LICN:');
        if (!price || isNaN(parseFloat(price)) || parseFloat(price) <= 0) return;

        listNFTForSale(nft, parseFloat(price));
    };

    window._profileMakeCollectionOffer = function (index) {
        var nft = createdNFTs[index];
        if (!nft || !currentWallet) return;

        var collectionId = nft.collection || nft.contract_id || '';
        if (!collectionId) {
            showToast('Collection not found for this NFT', 'error');
            return;
        }

        var amount = prompt('Enter collection offer amount in LICN:');
        if (!amount || isNaN(parseFloat(amount)) || parseFloat(amount) <= 0) return;

        var expiryHours = prompt('Enter expiry in hours (optional, leave blank for no expiry):');
        var expiryTs = 0;
        if (expiryHours && expiryHours.trim() !== '') {
            var h = Number(expiryHours);
            if (!Number.isFinite(h) || h < 0) {
                showToast('Expiry must be a non-negative number', 'error');
                return;
            }
            expiryTs = Math.floor(Date.now() / 1000) + Math.floor(h * 3600);
        }

        makeCollectionOffer(collectionId, Math.round(parseFloat(amount) * 1e9), expiryTs);
    };

    window._profileCreateAuction = function (index) {
        var nft = ownedNFTs[index];
        if (!nft || !isOwnProfile || !currentWallet) return;

        var startPriceInput = prompt('Start price in LICN:');
        if (!startPriceInput || isNaN(parseFloat(startPriceInput)) || parseFloat(startPriceInput) <= 0) return;
        var reserveInput = prompt('Reserve price in LICN (optional, default 0):');
        var durationInput = prompt('Duration in hours (default 24):');

        var startSpores = Math.round(parseFloat(startPriceInput) * 1e9);
        var reserveSpores = reserveInput && !isNaN(parseFloat(reserveInput)) ? Math.round(parseFloat(reserveInput) * 1e9) : 0;
        var durationHours = durationInput && !isNaN(parseFloat(durationInput)) ? Math.max(1, Math.floor(parseFloat(durationInput))) : 24;
        var now = Math.floor(Date.now() / 1000);
        var endTs = now + (durationHours * 3600);

        submitAuctionCreate(nft, startSpores, reserveSpores, now, endTs);
    };

    window._profilePlaceBid = function (index) {
        var nft = ownedNFTs[index];
        if (!nft || !currentWallet || isOwnProfile) return;

        var bidInput = prompt('Bid amount in LICN:');
        if (!bidInput || isNaN(parseFloat(bidInput)) || parseFloat(bidInput) <= 0) return;
        var bidSpores = Math.round(parseFloat(bidInput) * 1e9);
        submitAuctionBid(nft, bidSpores);
    };

    async function submitAuctionCreate(nft, startSpores, reserveSpores, startTs, endTs) {
        try {
            var mp = await resolveMarketplaceProgram();
            if (!mp) throw new Error('Cannot resolve marketplace program');
            var nftContract = nft.collection || nft.contract_id || '';
            var tokenId = Number(nft.token_id || nft.id || 0);

            var callData = buildContractCallData('create_auction', [
                currentWallet.address,
                nftContract,
                tokenId,
                startSpores,
                reserveSpores,
                '',
                startTs,
                endTs
            ], 0);

            await window.lichenWallet.sendTransaction([{
                program_id: CONTRACT_PROGRAM_ID,
                accounts: [currentWallet.address, mp, nftContract],
                data: callData,
            }]);

            showToast('Auction created', 'success');
            loadActivity();
        } catch (err) {
            showToast('Create auction failed: ' + err.message, 'error');
        }
    }

    async function submitAuctionBid(nft, bidSpores) {
        try {
            var mp = await resolveMarketplaceProgram();
            if (!mp) throw new Error('Cannot resolve marketplace program');
            var nftContract = nft.collection || nft.contract_id || '';
            var tokenId = Number(nft.token_id || nft.id || 0);

            var callData = buildContractCallData('place_bid', [
                currentWallet.address,
                nftContract,
                tokenId,
                bidSpores
            ], bidSpores);

            await window.lichenWallet.sendTransaction([{
                program_id: CONTRACT_PROGRAM_ID,
                accounts: [currentWallet.address, mp, nftContract],
                data: callData,
            }]);

            showToast('Bid placed', 'success');
            loadActivity();
        } catch (err) {
            showToast('Place bid failed: ' + err.message, 'error');
        }
    }

    async function makeCollectionOffer(collectionId, priceSpores, expiryTs) {
        try {
            var mp = await resolveMarketplaceProgram();
            if (!mp) throw new Error('Cannot resolve marketplace program');

            var callData = buildContractCallData('make_collection_offer', [
                currentWallet.address,
                collectionId,
                priceSpores,
                '',
                expiryTs || 0
            ], priceSpores);

            await window.lichenWallet.sendTransaction([{
                program_id: CONTRACT_PROGRAM_ID,
                accounts: [currentWallet.address, mp, collectionId],
                data: callData,
            }]);

            showToast('Collection offer submitted', 'success');
            loadProfileOffers();
        } catch (err) {
            showToast('Collection offer failed: ' + err.message, 'error');
        }
    }

    async function listNFTForSale(nft, priceLicn) {
        lazyAddresses();
        try {
            var mp = await resolveMarketplaceProgram();
            if (!mp) throw new Error('Cannot resolve marketplace program');

            var nftContract = nft.collection || nft.contract_id || '';
            var tokenId = String(nft.token_id || nft.id);
            var priceSpores = Math.round(priceLicn * 1e9);
            var royaltyPercent = Number(nft.royalty || 0);
            var royaltyBps = Math.max(0, Math.min(5000, Math.round(royaltyPercent * 100)));
            var royaltyRecipient = nft.creator || currentWallet.address;

            var callData;
            if (royaltyBps > 0) {
                callData = buildContractCallData('list_nft_with_royalty', [
                    currentWallet.address,
                    nftContract,
                    tokenId,
                    priceSpores,
                    '',
                    royaltyRecipient,
                    royaltyBps
                ], 0);
            } else {
                callData = buildContractCallData('list_nft', [
                    currentWallet.address,
                    nftContract,
                    tokenId,
                    priceSpores,
                    ''
                ], 0);
            }

            await window.lichenWallet.sendTransaction([{
                program_id: CONTRACT_PROGRAM_ID,
                accounts: [currentWallet.address, mp, nftContract],
                data: callData,
            }]);

            showToast('Listed for ' + priceLicn + ' LICN!', 'success');

            // Refresh
            allListings = await dataSource.getAllListings(500);
            loadCollectedNFTs();

        } catch (err) {
            showToast('Listing failed: ' + err.message, 'error');
        }
    }

    // ===== Cancel Listing (from profile) =====
    window._profileCancelListing = function (index) {
        var nft = ownedNFTs[index];
        if (!nft || !isOwnProfile) return;
        cancelListing(nft);
    };

    window._profileUpdatePrice = function (index) {
        var nft = ownedNFTs[index];
        if (!nft || !isOwnProfile) return;

        var listing = allListings.find(function (l) {
            return (l.nft_contract === (nft.collection || nft.contract_id) && String(l.token_id) === String(nft.token_id)) ||
                l.nft_id === nft.id;
        });
        if (!listing || listing.active === false) {
            showToast('Listing not found for this NFT', 'error');
            return;
        }

        var currentPriceLicn = listing.price_licn !== undefined
            ? Number(listing.price_licn || 0)
            : Number((listing.price || 0) / 1e9);
        var next = prompt('Enter new listing price in LICN:', currentPriceLicn > 0 ? String(currentPriceLicn) : '');
        if (!next || isNaN(parseFloat(next)) || parseFloat(next) <= 0) return;

        updateListingPrice(nft, parseFloat(next));
    };

    async function updateListingPrice(nft, newPriceLicn) {
        lazyAddresses();
        try {
            var mp = await resolveMarketplaceProgram();
            if (!mp) throw new Error('Cannot resolve marketplace program');

            var nftContract = nft.collection || nft.contract_id || '';
            var tokenId = Number(nft.token_id || nft.id || 0);
            var priceSpores = Math.round(newPriceLicn * 1e9);

            var callData = buildContractCallData('update_listing_price', [
                currentWallet.address,
                nftContract,
                tokenId,
                priceSpores
            ], 0);

            await window.lichenWallet.sendTransaction([{
                program_id: CONTRACT_PROGRAM_ID,
                accounts: [currentWallet.address, mp, nftContract],
                data: callData,
            }]);

            showToast('Price updated to ' + fmp(newPriceLicn, true) + ' LICN', 'success');
            allListings = await dataSource.getAllListings(500);
            loadCollectedNFTs();
        } catch (err) {
            showToast('Update failed: ' + err.message, 'error');
        }
    }

    async function cancelListing(nft) {
        lazyAddresses();
        try {
            var mp = await resolveMarketplaceProgram();
            if (!mp) throw new Error('Cannot resolve marketplace program');

            var nftContract = nft.collection || nft.contract_id || '';
            var tokenId = String(nft.token_id || nft.id);

            var callData = buildContractCallData('cancel_listing', [
                currentWallet.address,
                nftContract,
                tokenId
            ], 0);

            await window.lichenWallet.sendTransaction([{
                program_id: CONTRACT_PROGRAM_ID,
                accounts: [currentWallet.address, mp, nftContract],
                data: callData,
            }]);

            showToast('Listing cancelled', 'success');

            allListings = await dataSource.getAllListings(500);
            loadCollectedNFTs();

        } catch (err) {
            showToast('Cancel failed: ' + err.message, 'error');
        }
    }

    // ===== Copy Address =====
    function copyAddress() {
        var addr = profileAddress || '';
        if (navigator.clipboard) {
            navigator.clipboard.writeText(addr).then(function () { showToast('Address copied!', 'success'); });
        }
    }

    function bindRenderedControls() {
        var copyProfileAddressBtn = document.getElementById('copyProfileAddressBtn');
        if (copyProfileAddressBtn) {
            copyProfileAddressBtn.addEventListener('click', copyAddress);
        }

        function bindGrid(gridId, actionHandler) {
            var grid = document.getElementById(gridId);
            if (!grid) return;
            grid.addEventListener('click', function (event) {
                var actionButton = event.target.closest('[data-profile-action]');
                if (actionButton) {
                    event.preventDefault();
                    event.stopPropagation();
                    actionHandler(actionButton);
                    return;
                }

                var card = event.target.closest('[data-profile-href]');
                if (!card) return;
                window.location.href = card.getAttribute('data-profile-href');
            });
        }

        bindGrid('collectedGrid', function (actionButton) {
            var index = parseInt(actionButton.getAttribute('data-profile-index'), 10);
            if (!Number.isFinite(index)) return;

            var action = actionButton.getAttribute('data-profile-action');
            if (action === 'update-price') {
                window._profileUpdatePrice(index);
            } else if (action === 'cancel-listing') {
                window._profileCancelListing(index);
            } else if (action === 'list-nft') {
                window._profileListNFT(index);
            } else if (action === 'create-auction') {
                window._profileCreateAuction(index);
            } else if (action === 'place-bid') {
                window._profilePlaceBid(index);
            }
        });

        bindGrid('createdGrid', function (actionButton) {
            var index = parseInt(actionButton.getAttribute('data-profile-index'), 10);
            if (!Number.isFinite(index)) return;
            if (actionButton.getAttribute('data-profile-action') === 'collection-offer') {
                window._profileMakeCollectionOffer(index);
            }
        });

        bindGrid('favoritedGrid', function () { });

        var offersTable = document.getElementById('offersTable');
        if (offersTable) {
            offersTable.addEventListener('click', function (event) {
                var actionButton = event.target.closest('[data-profile-action]');
                if (!actionButton) return;

                var action = actionButton.getAttribute('data-profile-action');
                if (action === 'accept-offer') {
                    window._profileAcceptOffer(
                        actionButton.getAttribute('data-offerer') || '',
                        actionButton.getAttribute('data-nft-contract') || '',
                        actionButton.getAttribute('data-token-id') || ''
                    );
                } else if (action === 'cancel-offer') {
                    window._profileCancelOffer(
                        actionButton.getAttribute('data-nft-contract') || '',
                        actionButton.getAttribute('data-token-id') || ''
                    );
                }
            });
        }
    }

    // ===== Events =====
    function setupEvents() {
        if (window.LichenWallet) {
            window.lichenWallet = window.lichenWallet || new LichenWallet({ rpcUrl: RPC_URL });
            window.lichenWallet.bindConnectButton('#connectWallet');
            window.lichenWallet.onConnect(function (info) {
                var previousWalletAddress = currentWallet && currentWallet.address ? currentWallet.address : '';
                currentWallet = info;
                updateNav();

                var switchedWallet = !!previousWalletAddress && previousWalletAddress !== info.address;
                var viewingConnectedWallet = !!profileAddress && profileAddress === previousWalletAddress;
                if (!profileAddress || switchedWallet || viewingConnectedWallet) {
                    var nextUrl = 'profile.html?id=' + encodeURIComponent(info.address);
                    if (window.location.search !== '?id=' + encodeURIComponent(info.address)) {
                        window.location.href = nextUrl;
                        return;
                    }
                    profileAddress = info.address;
                }

                isOwnProfile = profileAddress === info.address;
                loadProfile();
            });
            window.lichenWallet.onDisconnect(function () {
                currentWallet = null;
                isOwnProfile = false;
                updateNav();
                loadProfile();
            });
        }

        // Tabs
        var tabs = document.querySelectorAll('.profile-tab');
        tabs.forEach(function (tab) {
            tab.addEventListener('click', function () {
                tabs.forEach(function (t) { t.classList.remove('active'); });
                tab.classList.add('active');
                var tabName = tab.dataset.tab;
                document.querySelectorAll('.tab-content').forEach(function (tc) {
                    tc.classList.toggle('active', tc.id === tabName + '-tab');
                });
            });
        });

        // Activity filter buttons
        var filterBtns = document.querySelectorAll('#activity-tab .filter-btn');
        filterBtns.forEach(function (btn) {
            btn.addEventListener('click', function () {
                filterBtns.forEach(function (b) { b.classList.remove('active'); });
                btn.classList.add('active');
                var filterType = btn.dataset.filter;
                var rows = document.querySelectorAll('#activityTable tr[data-type]');
                rows.forEach(function (row) {
                    if (filterType === 'all') {
                        row.style.display = '';
                    } else {
                        var match = filterType.replace(/s$/, '');
                        row.style.display = row.dataset.type === match ? '' : 'none';
                    }
                });
            });
        });

        // Offers filter buttons
        var offerFilterBtns = document.querySelectorAll('#offers-tab .filter-btn');
        offerFilterBtns.forEach(function (btn) {
            btn.addEventListener('click', function () {
                offerFilterBtns.forEach(function (b) { b.classList.remove('active'); });
                btn.classList.add('active');
                var filterType = btn.dataset.filter;
                var rows = document.querySelectorAll('#offersTable tr[data-dir]');
                rows.forEach(function (row) {
                    if (filterType === 'all') {
                        row.style.display = '';
                    } else {
                        row.style.display = row.dataset.dir === filterType ? '' : 'none';
                    }
                });
            });
        });

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

        if (typeof initMarketNetworkSelector === 'function') initMarketNetworkSelector();

        // Sort selects
        var collectedSort = document.getElementById('collectedSort');
        if (collectedSort) {
            collectedSort.addEventListener('change', function () {
                sortNFTs(ownedNFTs, collectedSort.value);
                loadCollectedNFTs();
            });
        }

        var createdSort = document.getElementById('createdSort');
        if (createdSort) {
            createdSort.addEventListener('change', function () {
                sortNFTs(createdNFTs, createdSort.value);
                loadCreatedNFTs();
            });
        }

        var favoritedSort = document.getElementById('favoritedSort');
        if (favoritedSort) {
            favoritedSort.addEventListener('change', function () {
                sortNFTs(favoritedNFTs, favoritedSort.value);
                renderFavoritedGrid();
            });
        }
    }

    function sortNFTs(arr, sortBy) {
        if (sortBy === 'price_high' || sortBy === 'popular') {
            arr.sort(function (a, b) { return (b.price || 0) - (a.price || 0); });
        } else if (sortBy === 'price_low') {
            arr.sort(function (a, b) { return (a.price || 0) - (b.price || 0); });
        } else if (sortBy === 'oldest') {
            arr.sort(function (a, b) { return (a.created_at || 0) - (b.created_at || 0); });
        } else if (sortBy === 'sales') {
            arr.sort(function (a, b) {
                var salesA = Number(a.sales_count || a.sales || a.sale_count || a.total_sales || 0);
                var salesB = Number(b.sales_count || b.sales || b.sale_count || b.total_sales || 0);
                return salesB - salesA;
            });
        } else if (sortBy === 'recent') {
            arr.sort(function (a, b) { return (b.favorited_at || b.created_at || 0) - (a.favorited_at || a.created_at || 0); });
        } else {
            arr.sort(function (a, b) { return (b.created_at || 0) - (a.created_at || 0); });
        }
    }

    // ===== Init =====
    document.addEventListener('DOMContentLoaded', function () {
        console.log('Lichen Market Profile loading...');
        setupEvents();
        bindRenderedControls();
        updateNav();
        loadProfile();
        console.log('Lichen Market Profile ready');
    });
})();
