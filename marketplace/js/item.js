// Molt Market — Single NFT Detail Page
// Loads NFT metadata, price history, activity, and related items from RPC

(function () {
    'use strict';

    const RPC_URL = (window.moltMarketConfig && window.moltMarketConfig.rpcUrl) || 'http://localhost:8899';
    let currentWallet = null;
    let currentNFT = null;

    // rpcCall, formatHash, timeAgo provided by shared/utils.js

    // ===== Utilities =====

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

    // Safe image URL — only allow http(s) and ipfs protocols
    function safeImageUrl(url) {
        if (!url) return null;
        if (url.startsWith('ipfs://')) return url.replace('ipfs://', 'https://ipfs.io/ipfs/');
        if (url.startsWith('http://') || url.startsWith('https://')) return url;
        if (url.startsWith('linear-gradient')) return url;
        return null;
    }

    function normalizeImage(uri, seed) {
        if (uri && uri.startsWith('ipfs://')) return uri.replace('ipfs://', 'https://ipfs.io/ipfs/');
        if (uri && (uri.startsWith('http://') || uri.startsWith('https://'))) return uri;
        return null; // Return null to signal "use gradient"
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
    function getNFTId() {
        var params = new URLSearchParams(window.location.search);
        return params.get('id');
    }

    async function loadNFTDetail() {
        var nftId = getNFTId();
        if (!nftId) {
            showError('No NFT ID specified');
            return;
        }

        showPageLoading(true);

        var nft = null;
        try {
            nft = await rpcCall('getNFT', [nftId]);
        } catch (err) {
            console.warn('Failed to load NFT from RPC:', err);
        }

        if (!nft) {
            showError('NFT not found');
            showPageLoading(false);
            return;
        }

        currentNFT = nft;
        renderNFTDetail(nft);
        loadActivity(nftId);
        loadMoreFromCollection(nft.collection || nft.collection_id);
        showPageLoading(false);
    }

    function renderNFTDetail(nft) {
        // Image
        var nftImage = document.getElementById('nftImage');
        if (nftImage) {
            var imageUrl = normalizeImage(nft.metadata_uri || nft.image, nft.id);
            if (imageUrl) {
                nftImage.innerHTML = '<img src="' + escapeHtml(imageUrl) + '" alt="' + escapeHtml(nft.name || 'NFT') + '" style="width:100%;height:100%;object-fit:cover;border-radius:12px;">';
            } else {
                nftImage.style.background = gradientFromHash(nft.id || 'default');
                nftImage.innerHTML = '<div style="display:flex;align-items:center;justify-content:center;height:100%;font-size:64px;">🦞</div>';
            }
        }

        // Name
        setText('nftName', nft.name || '#' + (nft.token_id || '0'));

        // Description
        setText('nftDescription', nft.description || 'No description available.');

        // Stats
        setText('viewCount', nft.views || 0);
        setText('likeCount', nft.likes || 0);

        // Contract info
        var contractAddr = document.getElementById('contractAddress');
        if (contractAddr) {
            contractAddr.textContent = formatHash(nft.contract_address || nft.contract || nft.collection_id || '-', 16);
            contractAddr.href = '../explorer/address.html?address=' + encodeURIComponent(nft.contract_address || nft.contract || '');
        }

        setText('tokenId', nft.token_id !== undefined ? '#' + nft.token_id : nft.id || '-');

        setText('royalty', (nft.royalty || 0) + '%');

        // Collection info
        var collectionLink = document.getElementById('collectionLink');
        if (collectionLink) {
            collectionLink.href = 'browse.html?collection=' + encodeURIComponent(nft.collection_id || nft.collection || '');
        }
        var collectionAvatar = document.getElementById('collectionAvatar');
        if (collectionAvatar) {
            var colSeed = nft.collection || nft.collection_id || 'col';
            collectionAvatar.style.background = gradientFromHash(colSeed);
            collectionAvatar.style.width = '24px';
            collectionAvatar.style.height = '24px';
            collectionAvatar.style.borderRadius = '50%';
        }
        setText('collectionName', nft.collection || 'Unknown Collection');

        // Owner
        var ownerLink = document.getElementById('ownerLink');
        if (ownerLink) ownerLink.href = 'profile.html?id=' + encodeURIComponent(nft.owner || '');
        setText('ownerName', formatHash(nft.owner || '-', 12));
        var ownerAvatar = document.getElementById('ownerAvatar');
        if (ownerAvatar) ownerAvatar.textContent = '👤';

        // Creator
        var creatorLink = document.getElementById('creatorLink');
        if (creatorLink) creatorLink.href = 'profile.html?id=' + encodeURIComponent(nft.creator || '');
        setText('creatorName', formatHash(nft.creator || '-', 12));
        var creatorAvatar = document.getElementById('creatorAvatar');
        if (creatorAvatar) creatorAvatar.textContent = '🎨';

        // Price
        var priceStr = nft.price_molt !== undefined ? Number(nft.price_molt).toFixed(2)
            : nft.price || priceToMolt(nft.price_shells || 0);
        setText('priceValue', priceStr + ' MOLT');
        // TODO: USD price should come from a price oracle, not a hardcoded multiplier
        setText('priceUSD', '≈ $' + (parseFloat(priceStr) * 0.10).toFixed(2) + ' USD');

        // ATH / Last Sale
        setText('athPrice', (nft.ath_price || priceStr) + ' MOLT');
        setText('lastSale', (nft.last_sale || '0.00') + ' MOLT');

        // Properties
        renderProperties(nft.properties || nft.traits || []);

        // Price chart (placeholder — real chart requires Chart.js)
        renderPriceChart(nft);
    }

    function renderProperties(properties) {
        var container = document.getElementById('propertiesGrid');
        if (!container) return;

        if (!properties || properties.length === 0) {
            container.innerHTML = '<div style="color: var(--text-secondary); padding: 12px;">No properties</div>';
            return;
        }

        container.innerHTML = properties.map(function (prop) {
            var traitType = escapeHtml(prop.trait_type || prop.key || 'Unknown');
            var value = escapeHtml(prop.value || '-');
            return '<div class="property-badge" style="background: var(--bg-secondary); border: 1px solid var(--border-color); border-radius: 8px; padding: 12px; text-align: center;">' +
                '<div style="font-size: 11px; color: var(--accent-color); text-transform: uppercase; margin-bottom: 4px;">' + traitType + '</div>' +
                '<div style="font-weight: 600;">' + value + '</div>' +
                '</div>';
        }).join('');
    }

    function renderPriceChart(nft) {
        var canvas = document.getElementById('chartCanvas');
        if (!canvas) return;

        // Show empty price history — real chart requires price oracle data
        var chartContainer = document.getElementById('priceChart');
        if (chartContainer) {
            chartContainer.innerHTML = '<div style="text-align:center; padding: 40px; color: var(--text-secondary);">' +
                '<i class="fas fa-chart-line" style="font-size: 32px; margin-bottom: 8px; opacity: 0.3;"></i>' +
                '<p>No price history available</p></div>';
        }
    }

    // ===== Activity =====
    async function loadActivity(nftId) {
        var container = document.getElementById('activityList');
        if (!container) return;

        container.innerHTML = '<div style="padding: 16px; color: var(--text-secondary);">Loading activity...</div>';

        var activity = [];
        try {
            var result = await rpcCall('getNFTActivity', [nftId, { limit: 20 }]);
            activity = result && result.activity ? result.activity : (Array.isArray(result) ? result : []);
        } catch (err) {
            console.warn('Failed to load NFT activity:', err);
        }

        if (activity.length === 0) {
            container.innerHTML = '<div style="padding: 16px; text-align: center; color: var(--text-secondary);">' +
                '<i class="fas fa-history" style="font-size: 32px; margin-bottom: 8px; opacity: 0.3;"></i>' +
                '<p>No activity yet</p></div>';
            return;
        }

        container.innerHTML = activity.map(function (event) {
            var eventTypes = { sale: '🔄', listing: '📋', transfer: '➡️', mint: '✨', offer: '💰', cancel: '❌' };
            var icon = eventTypes[event.type || event.kind] || '📋';
            var ts = normalizeTimestamp(event.timestamp);
            var price = event.price_molt !== undefined ? Number(event.price_molt).toFixed(2)
                : event.price ? priceToMolt(event.price) : '-';

            return '<div class="activity-item" style="display:flex; align-items:center; padding: 12px 0; border-bottom: 1px solid var(--border-color);">' +
                '<div style="font-size: 24px; margin-right: 12px;">' + icon + '</div>' +
                '<div style="flex: 1;">' +
                '<div style="font-weight: 600; text-transform: capitalize;">' + escapeHtml(event.type || event.kind || 'Event') + '</div>' +
                '<div style="font-size: 13px; color: var(--text-secondary);">' +
                (event.from ? 'From ' + formatHash(event.from, 8) : '') +
                (event.to ? ' → ' + formatHash(event.to, 8) : '') +
                '</div>' +
                '</div>' +
                '<div style="text-align: right;">' +
                (price !== '-' ? '<div style="font-weight: 600;">' + escapeHtml(price) + ' MOLT</div>' : '') +
                '<div style="font-size: 12px; color: var(--text-secondary);">' + timeAgo(ts) + '</div>' +
                '</div>' +
                '</div>';
        }).join('');
    }

    // ===== More From Collection =====
    async function loadMoreFromCollection(collectionId) {
        var container = document.getElementById('moreFromCollection');
        if (!container || !collectionId) return;

        container.innerHTML = '<div style="padding: 16px; color: var(--text-secondary);">Loading...</div>';

        var nfts = [];
        try {
            var result = await rpcCall('getNFTsByCollection', [collectionId, { limit: 6 }]);
            nfts = result && result.nfts ? result.nfts : (Array.isArray(result) ? result : []);
        } catch (err) {
            console.warn('Failed to load collection NFTs:', err);
        }

        // Filter out current NFT
        var currentId = getNFTId();
        nfts = nfts.filter(function (n) { return n.id !== currentId && n.token !== currentId; });

        if (nfts.length === 0) {
            container.innerHTML = '<div style="padding: 16px; text-align: center; color: var(--text-secondary);">No other items in this collection</div>';
            return;
        }

        container.innerHTML = nfts.slice(0, 6).map(function (nft) {
            var rawUrl = nft.image || nft.metadata_uri;
            var safeUrl = safeImageUrl(rawUrl);
            var imageStyle;
            if (safeUrl && safeUrl.startsWith('linear-gradient')) {
                imageStyle = 'background: ' + safeUrl;
            } else if (safeUrl) {
                imageStyle = 'background-image: url(' + encodeURI(safeUrl) + '); background-size: cover; background-position: center;';
            } else {
                imageStyle = 'background: ' + gradientFromHash(nft.id || 'x');
            }

            var price = nft.price_molt !== undefined ? Number(nft.price_molt).toFixed(2)
                : nft.price || priceToMolt(nft.price_shells || 0);

            return '<div class="nft-card" onclick="window._itemViewNFT(\'' + escapeHtml(nft.id || nft.token) + '\')" style="cursor:pointer;">' +
                '<div class="nft-image" style="height:180px;border-radius:8px;' + imageStyle + '"></div>' +
                '<div class="nft-info" style="padding:8px 0;">' +
                '<div class="nft-name">' + escapeHtml(nft.name || '#' + (nft.token_id || '0')) + '</div>' +
                '<div class="nft-price-value">' + escapeHtml(price) + ' MOLT</div>' +
                '</div>' +
                '</div>';
        }).join('');
    }

    // ===== Actions =====
    function handleBuy() {
        if (!currentWallet) {
            alert('Please connect your wallet first');
            return;
        }
        if (!currentNFT) return;

        var price = currentNFT.price_molt || currentNFT.price || priceToMolt(currentNFT.price_shells || 0);
        var confirmed = confirm('Buy ' + (currentNFT.name || 'this NFT') + ' for ' + price + ' MOLT?');
        if (!confirmed) return;

        // Execute purchase via RPC
        var buyBtn = document.getElementById('buyBtn');
        if (buyBtn) {
            buyBtn.disabled = true;
            buyBtn.innerHTML = '<i class="fas fa-spinner fa-spin"></i> Processing...';
        }

        // Build and send purchase transaction via RPC
        rpcCall('sendTransaction', [{
            from: currentWallet.address,
            to: currentNFT.contract_address || currentNFT.contract || '',
            amount: currentNFT.price_shells || Math.round(parseFloat(price) * 1_000_000_000),
            data: JSON.stringify({ action: 'buy', token_id: currentNFT.token_id || currentNFT.id }),
        }]).then(function (result) {
            if (result && result.success !== false) {
                showToast('Purchase successful! NFT transferred to your wallet.');
                loadNFTDetail(); // Refresh
            } else {
                alert('Transaction simulation failed: ' + (result ? result.error || 'Unknown error' : 'No response'));
            }
        }).catch(function (err) {
            alert('Purchase failed: ' + err.message);
        }).finally(function () {
            if (buyBtn) {
                buyBtn.disabled = false;
                buyBtn.innerHTML = '<i class="fas fa-shopping-cart"></i> Buy Now';
            }
        });
    }

    function handleMakeOffer() {
        if (!currentWallet) {
            alert('Please connect your wallet first');
            return;
        }
        var amount = prompt('Enter your offer amount in MOLT:');
        if (!amount || isNaN(parseFloat(amount)) || parseFloat(amount) <= 0) return;

        showToast('Submitting offers requires wallet connection and on-chain transaction signing.');
    }

    // ===== Helpers =====
    function setText(id, value) {
        var el = document.getElementById(id);
        if (el) el.textContent = value;
    }

    function showError(msg) {
        var nftImage = document.getElementById('nftImage');
        if (nftImage) {
            nftImage.innerHTML = '<div style="display:flex;align-items:center;justify-content:center;height:300px;color:var(--text-secondary);">' +
                '<div style="text-align:center;"><i class="fas fa-exclamation-triangle" style="font-size:48px;margin-bottom:12px;opacity:0.3;"></i>' +
                '<h3>' + msg + '</h3></div></div>';
        }
    }

    function showToast(msg) {
        var toast = document.createElement('div');
        toast.style.cssText = 'position:fixed;bottom:24px;left:50%;transform:translateX(-50%);background:#333;color:#fff;padding:12px 24px;border-radius:8px;z-index:9999;font-size:14px;max-width:400px;text-align:center;box-shadow:0 4px 12px rgba(0,0,0,0.3);';
        toast.textContent = msg;
        document.body.appendChild(toast);
        setTimeout(function () { toast.remove(); }, 4000);
    }

    function showPageLoading(show) {
        var nftImage = document.getElementById('nftImage');
        if (nftImage && show) {
            nftImage.innerHTML = '<div style="display:flex;align-items:center;justify-content:center;height:300px;color:var(--text-secondary);">' +
                '<i class="fas fa-spinner fa-spin" style="font-size:32px;"></i></div>';
        }
    }

    // ===== Event Setup =====
    function setupEvents() {
        // Buy / Offer buttons
        var buyBtn = document.getElementById('buyBtn');
        if (buyBtn) buyBtn.addEventListener('click', handleBuy);

        var offerBtn = document.getElementById('makeOfferBtn');
        if (offerBtn) offerBtn.addEventListener('click', handleMakeOffer);

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
    window._itemViewNFT = function (id) {
        window.location.href = 'item.html?id=' + encodeURIComponent(id);
    };

    // ===== Init =====
    document.addEventListener('DOMContentLoaded', function () {
        console.log('🦞 Molt Market Item loading...');
        if (typeof initMarketNetworkSelector === 'function') initMarketNetworkSelector();
        setupEvents();
        loadNFTDetail();
        console.log('✅ Molt Market Item ready');
    });
})();
