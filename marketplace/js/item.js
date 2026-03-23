// Lichen Market — NFT Detail / Item Page
// Owner detection → sell/list or buy/offer, balance check, proper contract calls
(function () {
    'use strict';

    var RPC_URL = (window.lichenMarketConfig && window.lichenMarketConfig.rpcUrl) || 'http://localhost:8899';
    var CONTRACT_PROGRAM_ID = null;
    var SYSTEM_PROGRAM_ID = null;

    var currentWallet = null;
    var currentNFT = null;
    var currentListing = null;
    var currentAuction = null;
    var marketplaceProgram = null;
    var userBalance = 0;
    var FAVORITES_STORAGE_KEY = 'lichenmarket_favorites_v1';

    var fmp = (window.marketplaceUtils && window.marketplaceUtils.formatLicnPrice) || function(v, isLicn) { var n = Number(isLicn ? v : v/1e9); if (n >= 0.01) return n.toFixed(2); if (n >= 0.0001) return n.toFixed(4); if (n >= 0.000001) return n.toFixed(6); if (n > 0) return n.toFixed(9); return '0'; };

    function lazyAddresses() {
        if (!SYSTEM_PROGRAM_ID) SYSTEM_PROGRAM_ID = bs58encode(new Uint8Array(32));
    }

    function buildContractCallData(functionName, args, value) {
        var argBytes = Array.from(new TextEncoder().encode(JSON.stringify(args || [])));
        return JSON.stringify({ Call: { function: functionName, args: argBytes, value: value || 0 } });
    }

    function sporesToLicn(value) {
        var n = Number(value || 0);
        return n / 1e9;
    }

    function listingPriceToLicn(listing, fallbackPrice) {
        if (listing && listing.price_licn !== undefined && listing.price_licn !== null) {
            return Number(listing.price_licn) || 0;
        }
        return sporesToLicn(fallbackPrice || 0);
    }

    async function resolveMarketplaceProgram() {
        if (marketplaceProgram) return marketplaceProgram;
        try {
            var entry = await rpcCall('getSymbolRegistry', ['LICHENMARKET']);
            marketplaceProgram = entry && (entry.program || entry.program_id) ? (entry.program || entry.program_id) : null;
            if (marketplaceProgram) CONTRACT_PROGRAM_ID = marketplaceProgram;
        } catch (_) {}
        return marketplaceProgram;
    }

    function showToast(msg, type) {
        var bg = type === 'error' ? '#ef4444' : type === 'success' ? '#22c55e' : '#3b82f6';
        var toast = document.createElement('div');
        toast.style.cssText = 'position:fixed;bottom:24px;left:50%;transform:translateX(-50%);background:' + bg + ';color:#fff;padding:12px 24px;border-radius:8px;z-index:9999;font-size:14px;max-width:500px;text-align:center;box-shadow:0 4px 12px rgba(0,0,0,0.3);';
        toast.textContent = msg;
        document.body.appendChild(toast);
        setTimeout(function () { toast.remove(); }, 5000);
    }

    function readFavoriteStore() {
        try {
            var raw = localStorage.getItem(FAVORITES_STORAGE_KEY);
            return raw ? (JSON.parse(raw) || {}) : {};
        } catch (_) {
            return {};
        }
    }

    function writeFavoriteStore(store) {
        try {
            localStorage.setItem(FAVORITES_STORAGE_KEY, JSON.stringify(store || {}));
        } catch (_) {}
    }

    function currentFavoriteKey() {
        if (!currentNFT) return null;
        var collection = currentNFT.collection || currentNFT.contract_id || '';
        var tokenId = String(currentNFT.token_id || currentNFT.id || '');
        if (!collection || !tokenId) return null;
        return collection + ':' + tokenId;
    }

    function isCurrentFavorited() {
        if (!currentWallet || !currentNFT) return false;
        var key = currentFavoriteKey();
        if (!key) return false;
        var store = readFavoriteStore();
        var list = Array.isArray(store[currentWallet.address]) ? store[currentWallet.address] : [];
        return list.some(function (entry) {
            return entry && entry.key === key;
        });
    }

    function updateFavoriteUI() {
        var favBtn = document.getElementById('favoriteToggleBtn');
        var likeCount = document.getElementById('likeCount');
        var liked = isCurrentFavorited();

        if (favBtn) {
            favBtn.style.color = liked ? '#ef4444' : '';
            favBtn.title = liked ? 'Unfavorite' : 'Favorite';
        }
        if (likeCount) likeCount.textContent = liked ? '1' : '0';
    }

    function toggleFavorite() {
        if (!currentWallet) {
            if (window.lichenWallet) window.lichenWallet._openWalletModal();
            return;
        }
        if (!currentNFT) return;

        var key = currentFavoriteKey();
        if (!key) return;

        var store = readFavoriteStore();
        var list = Array.isArray(store[currentWallet.address]) ? store[currentWallet.address] : [];
        var idx = list.findIndex(function (entry) { return entry && entry.key === key; });
        if (idx >= 0) {
            list.splice(idx, 1);
            showToast('Removed from favorites', 'success');
        } else {
            list.push({
                key: key,
                collection: currentNFT.collection || currentNFT.contract_id || '',
                token_id: String(currentNFT.token_id || currentNFT.id || ''),
                id: String(currentNFT.id || ''),
                name: currentNFT.name || '',
                added_at: Date.now()
            });
            showToast('Added to favorites', 'success');
        }
        store[currentWallet.address] = list;
        writeFavoriteStore(store);
        updateFavoriteUI();
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
        function c(n) { return '#' + ((n & 0xff0000) >> 16).toString(16).padStart(2, '0') + ((n & 0x00ff00) >> 8).toString(16).padStart(2, '0') + (n & 0x0000ff).toString(16).padStart(2, '0'); }
        return 'linear-gradient(135deg, ' + c(hashString(base + '-a')) + ', ' + c(hashString(base + '-b')) + ')';
    }

    function safeImageUrl(url) {
        if (!url || typeof url !== 'string') return null;
        var value = url.trim();
        if (!value) return null;
        if (value.indexOf('ipfs://') === 0) {
            return 'https://ipfs.io/ipfs/' + value.slice('ipfs://'.length);
        }
        if (value.indexOf('http://') === 0 || value.indexOf('https://') === 0) {
            return value;
        }
        if (value.indexOf('linear-gradient') === 0) {
            return value;
        }
        return null;
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

    // ===== Balance =====
    async function refreshBalance() {
        if (!currentWallet) { userBalance = 0; return; }
        try {
            var ds = window.marketplaceDataSource;
            if (ds) {
                userBalance = await ds.getWalletBalance(currentWallet.address);
            }
        } catch (_) { userBalance = 0; }
    }

    // ===== Parse URL =====
    function parseItemFromURL() {
        var params = new URLSearchParams(window.location.search);
        return {
            id: params.get('id') || '',
            contract: params.get('contract') || params.get('collection') || '',
            tokenId: params.get('token') || params.get('tokenId') || ''
        };
    }

    // ===== Load NFT =====
    async function loadNFT() {
        var parsed = parseItemFromURL();
        if (!parsed.id && !parsed.contract) {
            showToast('No NFT ID specified', 'error');
            return;
        }

        try {
            var ds = window.marketplaceDataSource;
            if (ds) {
                currentNFT = await ds.getNFTDetail(parsed.id);
            }
            if (!currentNFT && parsed.contract && parsed.tokenId) {
                currentNFT = await rpcCall('getNFT', [parsed.contract, parsed.tokenId]);
            }
            if (!currentNFT && parsed.id) {
                currentNFT = await rpcCall('getNFT', [parsed.id]);
            }
        } catch (err) {
            console.warn('RPC getNFT failed:', err);
        }

        if (!currentNFT) {
            currentNFT = {
                id: parsed.id,
                name: 'NFT #' + (parsed.tokenId || parsed.id),
                description: '',
                collection: parsed.contract || '',
                token_id: parsed.tokenId || parsed.id,
                image: null,
                owner: '',
                creator: '',
                price: null,
                royalty: 0,
                properties: [],
                listed: false
            };
        }

        renderNFT();
        loadActivity();
        loadOffers();
        loadAuctionState();
        loadMoreFromCollection();
        checkListingStatus();
        updateFavoriteUI();
    }

    // ===== Check Listing Status =====
    async function checkListingStatus() {
        if (!currentNFT) return;
        try {
            var mp = await resolveMarketplaceProgram();
            if (mp) {
                var collectionId = currentNFT.collection || currentNFT.contract_id || '';
                var listingResp = await rpcCall('getMarketListings', [{ collection: collectionId, limit: 100 }]);
                var listings = Array.isArray(listingResp)
                    ? listingResp
                    : ((listingResp && Array.isArray(listingResp.listings)) ? listingResp.listings : []);

                var matchedListing = listings.find(function (l) {
                    return (l.collection === collectionId && String(l.token_id || '') === String(currentNFT.token_id || '')) ||
                        (l.token && currentNFT.id && String(l.token) === String(currentNFT.id));
                }) || null;

                if (matchedListing) {
                    currentListing = matchedListing.active === false ? null : matchedListing;
                } else {
                    currentListing = null;
                }
            }
        } catch (_) {}
        updateActionButtons();
        updateAuctionPanel();
    }

    function parseAuctionType(event) {
        var raw = String((event && (event.type || event.kind || event.function)) || '').toLowerCase();
        if (raw.indexOf('auction_created') !== -1 || raw.indexOf('create_auction') !== -1) return 'created';
        if (raw.indexOf('auction_bid') !== -1 || raw.indexOf('place_bid') !== -1) return 'bid';
        if (raw.indexOf('auction_settled') !== -1 || raw.indexOf('settle_auction') !== -1) return 'settled';
        if (raw.indexOf('auction_cancelled') !== -1 || raw.indexOf('cancel_auction') !== -1) return 'cancelled';
        return '';
    }

    async function loadAuctionState() {
        if (!currentNFT) return;
        currentAuction = null;

        try {
            var collectionId = currentNFT.collection || currentNFT.contract_id || '';
            var tokenId = String(currentNFT.token_id || currentNFT.id || '');

            var auctionsResp = await rpcCall('getMarketAuctions', [{ collection: collectionId, limit: 200 }]);
            var auctions = Array.isArray(auctionsResp)
                ? auctionsResp
                : ((auctionsResp && Array.isArray(auctionsResp.auctions)) ? auctionsResp.auctions : []);

            var activityResp = await rpcCall('getMarketActivity', [{ collection: collectionId, limit: 200 }]);
            var activity = Array.isArray(activityResp)
                ? activityResp
                : ((activityResp && Array.isArray(activityResp.activity)) ? activityResp.activity : []);

            var relatedAuctionActivity = activity.filter(function (e) {
                var eventTokenId = String((e && e.token_id !== undefined) ? e.token_id : '');
                if (eventTokenId !== tokenId) return false;
                return !!parseAuctionType(e);
            }).sort(function (a, b) {
                return Number(b.timestamp || 0) - Number(a.timestamp || 0);
            });

            var latest = relatedAuctionActivity.length > 0 ? relatedAuctionActivity[0] : null;
            var latestType = parseAuctionType(latest);
            var created = auctions.find(function (a) {
                return String((a && a.token_id !== undefined) ? a.token_id : '') === tokenId;
            }) || null;

            var active = latestType === 'created' || latestType === 'bid';
            currentAuction = {
                active: !!active,
                latestType: latestType || '',
                createdEvent: created,
                latestEvent: latest,
                highestBid: latest && latest.price ? Number(latest.price || 0) : (created && created.price ? Number(created.price || 0) : 0),
                highestBidder: (latest && (latest.buyer || latest.seller)) || '',
            };
        } catch (_) {
            currentAuction = null;
        }

        updateAuctionPanel();
        updateActionButtons();
    }

    function updateAuctionPanel() {
        var panel = document.getElementById('auctionPanel');
        if (!panel || !currentNFT) return;

        var isOwner = currentWallet && currentNFT.owner && currentNFT.owner === currentWallet.address;
        var active = !!(currentAuction && currentAuction.active);
        var highestBidLicn = currentAuction && currentAuction.highestBid
            ? fmp(sporesToLicn(currentAuction.highestBid), true) + ' LICN'
            : '—';

        var statusLabel = 'No active auction';
        if (active) statusLabel = 'Active auction';
        else if (currentAuction && currentAuction.latestType === 'settled') statusLabel = 'Auction settled';
        else if (currentAuction && currentAuction.latestType === 'cancelled') statusLabel = 'Auction cancelled';

        var html =
            '<div style="display:flex;justify-content:space-between;align-items:center;margin-bottom:10px;">' +
            '<strong>' + escapeHtml(statusLabel) + '</strong>' +
            '<span style="opacity:0.7;font-size:12px;">Highest Bid: ' + escapeHtml(highestBidLicn) + '</span>' +
            '</div>';

        if (currentWallet) {
            if (isOwner) {
                html += '<button class="btn btn-small btn-secondary" id="auctionCreateBtn" style="margin-right:6px;">Create Auction</button>';
                html += '<button class="btn btn-small btn-secondary" id="auctionCancelBtn" style="margin-right:6px;">Cancel Auction</button>';
                html += '<button class="btn btn-small btn-primary" id="auctionSettleBtn">Settle Auction</button>';
            } else {
                html += '<button class="btn btn-small btn-primary" id="auctionBidBtn">Place Bid</button>';
            }
        } else {
            html += '<div style="opacity:0.6;font-size:13px;">Connect wallet to interact with auctions.</div>';
        }

        panel.innerHTML = html;

        var cBtn = document.getElementById('auctionCreateBtn');
        if (cBtn) cBtn.addEventListener('click', handleCreateAuction);
        var pBtn = document.getElementById('auctionBidBtn');
        if (pBtn) pBtn.addEventListener('click', handlePlaceBid);
        var sBtn = document.getElementById('auctionSettleBtn');
        if (sBtn) sBtn.addEventListener('click', handleSettleAuction);
        var xBtn = document.getElementById('auctionCancelBtn');
        if (xBtn) xBtn.addEventListener('click', handleCancelAuction);
    }

    // ===== Render NFT =====
    function renderNFT() {
        if (!currentNFT) return;
        var nft = currentNFT;

        document.title = (nft.name || 'NFT') + ' - Lichen Market';

        // Image
        var imageEl = document.getElementById('nftImage');
        if (imageEl) {
            var imageUrl = safeImageUrl(nft.image);
            if (imageUrl) {
                imageEl.innerHTML = '<img src="' + escapeHtml(imageUrl) + '" style="width:100%;height:100%;object-fit:cover;border-radius:12px;" alt="' + escapeHtml(nft.name || 'NFT') + '">';
            } else {
                imageEl.style.background = gradientFromHash(nft.id || nft.name || 'nft');
                imageEl.innerHTML = '<div style="display:flex;align-items:center;justify-content:center;height:100%;font-size:80px;opacity:0.5;min-height:300px;">\uD83D\uDDBC\uFE0F</div>';
            }
        }

        // Basic info
        setText('nftName', nft.name || 'Unnamed NFT');
        setText('nftDescription', nft.description || 'No description provided.');
        setText('contractAddress', formatHash(nft.collection || nft.contract_id || '', 12));
        setText('tokenId', nft.token_id || nft.id || '-');
        setText('royalty', (nft.royalty || 0) + '%');

        // Owner & Creator
        var ownerAddr = nft.owner || '';
        var creatorAddr = nft.creator || nft.minter || '';
        setText('ownerName', ownerAddr ? formatHash(ownerAddr, 10) : 'Unknown');
        setText('creatorName', creatorAddr ? formatHash(creatorAddr, 10) : 'Unknown');
        var ownerLink = document.getElementById('ownerLink');
        if (ownerLink && ownerAddr) ownerLink.href = 'profile.html?id=' + encodeURIComponent(ownerAddr);
        var creatorLink = document.getElementById('creatorLink');
        if (creatorLink && creatorAddr) creatorLink.href = 'profile.html?id=' + encodeURIComponent(creatorAddr);

        // Collection
        var colName = nft.collection_name || nft.collection || '';
        setText('collectionName', colName ? formatHash(colName, 16) : 'Unknown Collection');
        var colLink = document.getElementById('collectionLink');
        if (colLink && nft.collection) colLink.href = 'browse.html?collection=' + encodeURIComponent(nft.collection);

        // Price
        var price = nft.price || (currentListing ? currentListing.price : null);
        if (price) {
            var priceInLicn = currentListing
                ? listingPriceToLicn(currentListing, price)
                : Number(price || 0);
            setText('priceValue', fmp(priceInLicn, true));
            var usdVal = Number(priceInLicn) * 0.15;
            setText('priceUSD', usdVal >= 0.01 ? ('$' + usdVal.toFixed(2)) : (usdVal > 0 ? '<$0.01' : '$0.00'));
        } else {
            setText('priceValue', 'Not Listed');
            var usdEl = document.getElementById('priceUSD');
            if (usdEl) usdEl.textContent = '';
        }

        // Properties
        var propsGrid = document.getElementById('propertiesGrid');
        if (propsGrid) {
            var props = nft.properties || nft.attributes || [];
            if (props.length > 0) {
                propsGrid.innerHTML = props.map(function (p) {
                    var prop = p || {};
                    return '<div class="property-item">' +
                        '<span class="property-type">' + escapeHtml(prop.trait_type || prop.key || 'Unknown') + '</span>' +
                        '<span class="property-value">' + escapeHtml(prop.value || '-') + '</span>' +
                        '</div>';
                }).join('');
            } else {
                propsGrid.innerHTML = '<p style="opacity:0.5;">No properties</p>';
            }
        }

        updateActionButtons();
    }

    function setText(id, value) {
        var el = document.getElementById(id);
        if (el) el.textContent = value;
    }

    // ===== Action Buttons — Owner vs Buyer =====
    function updateActionButtons() {
        var actionContainer = document.querySelector('.action-buttons');
        if (!actionContainer) return;

        var isOwner = currentWallet && currentNFT && currentNFT.owner &&
            currentNFT.owner === currentWallet.address;
        var isListed = currentListing && currentListing.active !== false;
        var isMyListing = isListed && currentListing.seller === (currentWallet && currentWallet.address);

        actionContainer.innerHTML = '';

        if (isOwner || isMyListing) {
            // OWNER VIEW
            if (isMyListing) {
                // Already listed — show cancel
                actionContainer.innerHTML =
                    '<div style="padding:16px;background:var(--bg-secondary);border-radius:12px;margin-bottom:12px;">' +
                    '<div style="display:flex;align-items:center;gap:8px;margin-bottom:8px;">' +
                    '<i class="fas fa-check-circle" style="color:#22c55e;"></i>' +
                    '<strong>Listed for Sale</strong></div>' +
                    '<div style="font-size:24px;font-weight:700;margin-bottom:12px;">' +
                    (fmp(listingPriceToLicn(currentListing, currentListing.price), true)) + ' LICN</div>' +
                    '<div style="margin-bottom:10px;">' +
                    '<label style="font-size:12px;opacity:0.7;display:block;margin-bottom:4px;">Update Price (LICN)</label>' +
                    '<input type="number" id="updatePriceInput" class="form-input" min="0.001" step="0.001" placeholder="e.g. 12.5" ' +
                    'style="width:100%;padding:10px 12px;border:1px solid var(--border-color);border-radius:8px;background:var(--bg-primary);color:var(--text-primary);font-size:14px;">' +
                    '</div>' +
                    '<button class="btn btn-large btn-primary btn-block" id="updatePriceBtn" style="margin-bottom:8px;">' +
                    '<i class="fas fa-pen"></i> Update Price</button>' +
                    '<button class="btn btn-large btn-secondary btn-block" id="cancelListingBtn">' +
                    '<i class="fas fa-times"></i> Cancel Listing</button></div>';
                document.getElementById('updatePriceBtn').addEventListener('click', handleUpdatePrice);
                document.getElementById('cancelListingBtn').addEventListener('click', handleCancelListing);
            } else {
                // Not listed — show sell form
                actionContainer.innerHTML =
                    '<div style="padding:16px;background:var(--bg-secondary);border-radius:12px;">' +
                    '<h4 style="margin-bottom:12px;"><i class="fas fa-tag"></i> List for Sale</h4>' +
                    '<div style="margin-bottom:12px;">' +
                    '<label style="font-size:13px;opacity:0.7;display:block;margin-bottom:4px;">Price (LICN)</label>' +
                    '<input type="number" id="listPriceInput" class="form-input" placeholder="e.g. 10.0" min="0.001" step="0.001" ' +
                    'style="width:100%;padding:10px 14px;border:1px solid var(--border-color);border-radius:8px;background:var(--bg-primary);color:var(--text-primary);font-size:16px;">' +
                    '</div>' +
                    '<button class="btn btn-large btn-secondary btn-block" id="acceptCollectionOfferBtn" style="margin-top:8px;">' +
                    '<i class="fas fa-handshake"></i> Accept Collection Offer</button>' +
                    '<button class="btn btn-large btn-primary btn-block" id="listForSaleBtn">' +
                    '<i class="fas fa-tag"></i> List for Sale</button>' +
                    '<p style="margin-top:8px;font-size:12px;opacity:0.5;">Marketplace fee: 2.5%</p>' +
                    '</div>';
                document.getElementById('acceptCollectionOfferBtn').addEventListener('click', handleAcceptCollectionOffer);
                document.getElementById('listForSaleBtn').addEventListener('click', handleListForSale);
            }
        } else if (isListed) {
            // BUYER VIEW — NFT is listed
            if (currentWallet) {
                actionContainer.innerHTML =
                    '<button class="btn btn-large btn-primary btn-block" id="buyBtn">' +
                    '<i class="fas fa-shopping-cart"></i> Buy Now</button>' +
                    '<button class="btn btn-large btn-secondary btn-block" id="makeOfferBtn" style="margin-top:8px;">' +
                    '<i class="fas fa-hand-holding-usd"></i> Make Offer</button>' +
                    '<div style="margin-top:8px;">' +
                    '<label style="font-size:12px;opacity:0.7;display:block;margin-bottom:4px;">Offer Expiry (hours, optional)</label>' +
                    '<input type="number" id="offerExpiryHours" class="form-input" min="0" step="1" placeholder="e.g. 24" ' +
                    'style="width:100%;padding:10px 12px;border:1px solid var(--border-color);border-radius:8px;background:var(--bg-primary);color:var(--text-primary);font-size:14px;">' +
                    '</div>';
                document.getElementById('buyBtn').addEventListener('click', handleBuy);
                document.getElementById('makeOfferBtn').addEventListener('click', handleMakeOffer);
            } else {
                actionContainer.innerHTML =
                    '<button class="btn btn-large btn-primary btn-block" id="connectToBuyBtn">' +
                    '<i class="fas fa-wallet"></i> Connect Wallet to Buy</button>' +
                    '<p style="text-align:center;font-size:13px;opacity:0.5;margin-top:8px;">Connect your wallet to purchase or make offers</p>';
                document.getElementById('connectToBuyBtn').addEventListener('click', function () {
                    if (window.lichenWallet) window.lichenWallet._openWalletModal();
                });
            }
        } else {
            // NOT LISTED
            if (currentWallet) {
                actionContainer.innerHTML =
                    '<div style="text-align:center;padding:16px;opacity:0.6;">' +
                    '<i class="fas fa-tag" style="font-size:32px;margin-bottom:8px;display:block;"></i>' +
                    '<p>This NFT is not currently listed for sale.</p></div>' +
                    '<button class="btn btn-large btn-secondary btn-block" id="makeOfferBtn" style="margin-top:8px;">' +
                    '<i class="fas fa-hand-holding-usd"></i> Make Offer</button>' +
                    '<button class="btn btn-large btn-secondary btn-block" id="makeCollectionOfferBtn" style="margin-top:8px;">' +
                    '<i class="fas fa-layer-group"></i> Make Collection Offer</button>' +
                    '<button class="btn btn-large btn-secondary btn-block" id="cancelCollectionOfferBtn" style="margin-top:8px;">' +
                    '<i class="fas fa-times-circle"></i> Cancel Collection Offer</button>' +
                    '<div style="margin-top:8px;">' +
                    '<label style="font-size:12px;opacity:0.7;display:block;margin-bottom:4px;">Offer Expiry (hours, optional)</label>' +
                    '<input type="number" id="offerExpiryHours" class="form-input" min="0" step="1" placeholder="e.g. 24" ' +
                    'style="width:100%;padding:10px 12px;border:1px solid var(--border-color);border-radius:8px;background:var(--bg-primary);color:var(--text-primary);font-size:14px;">' +
                    '</div>';
                document.getElementById('makeOfferBtn').addEventListener('click', handleMakeOffer);
                document.getElementById('makeCollectionOfferBtn').addEventListener('click', handleMakeCollectionOffer);
                document.getElementById('cancelCollectionOfferBtn').addEventListener('click', handleCancelCollectionOffer);
            } else {
                actionContainer.innerHTML =
                    '<div style="text-align:center;padding:16px;opacity:0.6;">' +
                    '<i class="fas fa-tag" style="font-size:32px;margin-bottom:8px;display:block;"></i>' +
                    '<p>This NFT is not currently listed for sale.</p></div>' +
                    '<button class="btn btn-large btn-secondary btn-block" id="connectToBuyBtn">' +
                    '<i class="fas fa-wallet"></i> Connect Wallet</button>';
                document.getElementById('connectToBuyBtn').addEventListener('click', function () {
                    if (window.lichenWallet) window.lichenWallet._openWalletModal();
                });
            }
        }
    }

    // ===== List For Sale =====
    async function handleListForSale() {
        lazyAddresses();
        if (!currentWallet || !currentNFT) return;

        var priceInput = document.getElementById('listPriceInput');
        var price = priceInput ? parseFloat(priceInput.value) : 0;
        if (!price || price <= 0) {
            showToast('Please enter a valid price', 'error');
            if (priceInput) priceInput.focus();
            return;
        }

        var listBtn = document.getElementById('listForSaleBtn');
        if (listBtn) { listBtn.disabled = true; listBtn.innerHTML = '<i class="fas fa-spinner fa-spin"></i> Listing...'; }

        try {
            var mp = await resolveMarketplaceProgram();
            if (!mp) throw new Error('Cannot resolve marketplace program');

            var nftContract = currentNFT.collection || currentNFT.contract_id || '';
            var tokenId = String(currentNFT.token_id || currentNFT.id);
            var priceSpores = Math.round(price * 1e9);
            var royaltyPercent = Number(currentNFT.royalty || 0);
            var royaltyBps = Math.max(0, Math.min(5000, Math.round(royaltyPercent * 100)));
            var royaltyRecipient = currentNFT.creator || currentWallet.address;

            var callData;
            if (royaltyBps > 0) {
                callData = buildContractCallData('list_nft_with_royalty', [
                    currentWallet.address,
                    nftContract,
                    tokenId,
                    priceSpores,
                    '', // native LICN as payment token
                    royaltyRecipient,
                    royaltyBps
                ], 0);
            } else {
                // For list_nft: seller, nft_contract, token_id, price, payment_token
                callData = buildContractCallData('list_nft', [
                    currentWallet.address,
                    nftContract,
                    tokenId,
                    priceSpores,
                    '' // native LICN as payment token
                ], 0);
            }

            await window.lichenWallet.sendTransaction([{
                program_id: CONTRACT_PROGRAM_ID,
                accounts: [currentWallet.address, mp, nftContract],
                data: callData,
            }]);

            showToast('NFT listed for ' + price + ' LICN!', 'success');
            // Reload listing status
            await checkListingStatus();

        } catch (err) {
            showToast('Listing failed: ' + err.message, 'error');
        } finally {
            if (listBtn) { listBtn.disabled = false; listBtn.innerHTML = '<i class="fas fa-tag"></i> List for Sale'; }
        }
    }

    // ===== Cancel Listing =====
    async function handleCancelListing() {
        lazyAddresses();
        if (!currentWallet || !currentNFT || !currentListing) return;

        var cancelBtn = document.getElementById('cancelListingBtn');
        if (cancelBtn) { cancelBtn.disabled = true; cancelBtn.innerHTML = '<i class="fas fa-spinner fa-spin"></i> Cancelling...'; }

        try {
            var mp = await resolveMarketplaceProgram();
            if (!mp) throw new Error('Cannot resolve marketplace program');

            var nftContract = currentListing.nft_contract || currentNFT.collection || '';
            var tokenId = String(currentListing.token_id || currentNFT.token_id);

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
            currentListing = null;
            updateActionButtons();

        } catch (err) {
            showToast('Cancel failed: ' + err.message, 'error');
        } finally {
            if (cancelBtn) { cancelBtn.disabled = false; cancelBtn.innerHTML = '<i class="fas fa-times"></i> Cancel Listing'; }
        }
    }

    // ===== Update Listing Price =====
    async function handleUpdatePrice() {
        lazyAddresses();
        if (!currentWallet || !currentNFT || !currentListing) return;

        var updateInput = document.getElementById('updatePriceInput');
        var newPrice = updateInput ? parseFloat(updateInput.value) : 0;
        if (!newPrice || newPrice <= 0) {
            showToast('Please enter a valid new price', 'error');
            if (updateInput) updateInput.focus();
            return;
        }

        var updateBtn = document.getElementById('updatePriceBtn');
        if (updateBtn) { updateBtn.disabled = true; updateBtn.innerHTML = '<i class="fas fa-spinner fa-spin"></i> Updating...'; }

        try {
            var mp = await resolveMarketplaceProgram();
            if (!mp) throw new Error('Cannot resolve marketplace program');

            var nftContract = currentListing.nft_contract || currentNFT.collection || currentNFT.contract_id || '';
            var tokenId = Number(currentListing.token_id || currentNFT.token_id || currentNFT.id || 0);
            var newPriceSpores = Math.round(newPrice * 1e9);

            var callData = buildContractCallData('update_listing_price', [
                currentWallet.address,
                nftContract,
                tokenId,
                newPriceSpores
            ], 0);

            await window.lichenWallet.sendTransaction([{
                program_id: CONTRACT_PROGRAM_ID,
                accounts: [currentWallet.address, mp, nftContract],
                data: callData,
            }]);

            showToast('Listing price updated to ' + fmp(newPrice, true) + ' LICN', 'success');
            await checkListingStatus();
        } catch (err) {
            showToast('Update price failed: ' + err.message, 'error');
        } finally {
            if (updateBtn) { updateBtn.disabled = false; updateBtn.innerHTML = '<i class="fas fa-pen"></i> Update Price'; }
        }
    }

    // ===== Buy =====
    async function handleBuy() {
        lazyAddresses();
        if (!currentWallet || !currentNFT) {
            showToast('Connect wallet to buy', 'error');
            return;
        }

        // Balance check
        await refreshBalance();
        var price = currentListing ? currentListing.price : (currentNFT.price || 0);
        var priceInLicn = currentListing
            ? listingPriceToLicn(currentListing, price)
            : Number(price || 0);

        if (userBalance < priceInLicn) {
            showToast('Insufficient balance. Need ' + Number(priceInLicn).toFixed(3) + ' LICN, have ' + userBalance.toFixed(4) + ' LICN.', 'error');
            return;
        }

        var buyBtn = document.getElementById('buyBtn');
        if (buyBtn) { buyBtn.disabled = true; buyBtn.innerHTML = '<i class="fas fa-spinner fa-spin"></i> Buying...'; }

        try {
            var mp = await resolveMarketplaceProgram();
            if (!mp) throw new Error('Cannot resolve marketplace program');

            var nftContract = currentNFT.collection || currentNFT.contract_id || '';
            var tokenId = String(currentNFT.token_id || currentNFT.id);

            // buy_nft: buyer, nft_contract, token_id
            var callData = buildContractCallData('buy_nft', [
                currentWallet.address,
                nftContract,
                tokenId
            ], Math.round(priceInLicn * 1e9));

            await window.lichenWallet.sendTransaction([{
                program_id: CONTRACT_PROGRAM_ID,
                accounts: [currentWallet.address, mp, nftContract],
                data: callData,
            }]);

            showToast('Purchase complete! You now own this NFT.', 'success');

            // Refresh the page to show new owner
            currentNFT.owner = currentWallet.address;
            currentListing = null;
            renderNFT();
            refreshBalance();

        } catch (err) {
            showToast('Purchase failed: ' + err.message, 'error');
        } finally {
            if (buyBtn) { buyBtn.disabled = false; buyBtn.innerHTML = '<i class="fas fa-shopping-cart"></i> Buy Now'; }
        }
    }

    // ===== Make Offer =====
    async function handleMakeOffer() {
        lazyAddresses();
        if (!currentWallet) {
            if (window.lichenWallet) window.lichenWallet._openWalletModal();
            return;
        }

        // Prompt for offer amount
        var offerAmount = prompt('Enter your offer amount in LICN:');
        if (!offerAmount || isNaN(parseFloat(offerAmount)) || parseFloat(offerAmount) <= 0) {
            return;
        }
        var offerLicn = parseFloat(offerAmount);

        var expiryInput = document.getElementById('offerExpiryHours');
        var expiryHoursText = expiryInput ? expiryInput.value : '';
        var expiryHours = 0;
        if (expiryHoursText && expiryHoursText.trim() !== '') {
            expiryHours = Number(expiryHoursText);
            if (!Number.isFinite(expiryHours) || expiryHours < 0) {
                showToast('Expiry must be a non-negative number of hours', 'error');
                return;
            }
        }
        var expiryTs = expiryHours > 0
            ? Math.floor(Date.now() / 1000) + Math.floor(expiryHours * 3600)
            : 0;

        // Balance check
        await refreshBalance();
        if (userBalance < offerLicn) {
            showToast('Insufficient balance for this offer. Have ' + userBalance.toFixed(4) + ' LICN.', 'error');
            return;
        }

        var offerBtn = document.getElementById('makeOfferBtn');
        if (offerBtn) { offerBtn.disabled = true; offerBtn.innerHTML = '<i class="fas fa-spinner fa-spin"></i> Submitting...'; }

        try {
            var mp = await resolveMarketplaceProgram();
            if (!mp) throw new Error('Cannot resolve marketplace program');

            var nftContract = currentNFT.collection || currentNFT.contract_id || '';
            var tokenId = String(currentNFT.token_id || currentNFT.id);
            var offerSpores = Math.round(offerLicn * 1e9);

            // make_offer_with_expiry: offerer, nft_contract, token_id, price, payment_token, expiry
            var callData = buildContractCallData('make_offer_with_expiry', [
                currentWallet.address,
                nftContract,
                tokenId,
                offerSpores,
                '', // native LICN
                expiryTs
            ], offerSpores);

            await window.lichenWallet.sendTransaction([{
                program_id: CONTRACT_PROGRAM_ID,
                accounts: [currentWallet.address, mp, nftContract],
                data: callData,
            }]);

            var expiryLabel = expiryTs > 0
                ? (' (expires in ' + Math.floor(expiryHours) + 'h)')
                : '';
            showToast('Offer of ' + fmp(offerLicn, true) + ' LICN submitted!' + expiryLabel, 'success');

        } catch (err) {
            showToast('Offer failed: ' + err.message, 'error');
        } finally {
            if (offerBtn) { offerBtn.disabled = false; offerBtn.innerHTML = '<i class="fas fa-hand-holding-usd"></i> Make Offer'; }
        }
    }

    async function handleMakeCollectionOffer() {
        if (!currentWallet || !currentNFT) return;

        var collectionId = currentNFT.collection || currentNFT.contract_id || '';
        if (!collectionId) {
            showToast('Collection is required for collection offers', 'error');
            return;
        }

        var offerAmount = prompt('Enter your collection offer amount in LICN:');
        if (!offerAmount || isNaN(parseFloat(offerAmount)) || parseFloat(offerAmount) <= 0) return;
        var offerLicn = parseFloat(offerAmount);
        var offerSpores = Math.round(offerLicn * 1e9);

        var expiryInput = document.getElementById('offerExpiryHours');
        var expiryHours = expiryInput && expiryInput.value ? Number(expiryInput.value) : 0;
        if (!Number.isFinite(expiryHours) || expiryHours < 0) {
            showToast('Expiry must be a non-negative number of hours', 'error');
            return;
        }
        var expiryTs = expiryHours > 0 ? (Math.floor(Date.now() / 1000) + Math.floor(expiryHours * 3600)) : 0;

        await refreshBalance();
        if (userBalance < offerLicn) {
            showToast('Insufficient balance for this collection offer.', 'error');
            return;
        }

        try {
            var mp = await resolveMarketplaceProgram();
            if (!mp) throw new Error('Cannot resolve marketplace program');

            var callData = buildContractCallData('make_collection_offer', [
                currentWallet.address,
                collectionId,
                offerSpores,
                '',
                expiryTs
            ], offerSpores);

            await window.lichenWallet.sendTransaction([{
                program_id: CONTRACT_PROGRAM_ID,
                accounts: [currentWallet.address, mp, collectionId],
                data: callData,
            }]);

            showToast('Collection offer submitted for ' + fmp(offerLicn, true) + ' LICN', 'success');
        } catch (err) {
            showToast('Collection offer failed: ' + err.message, 'error');
        }
    }

    async function handleCancelCollectionOffer() {
        if (!currentWallet || !currentNFT) return;
        var collectionId = currentNFT.collection || currentNFT.contract_id || '';
        if (!collectionId) return;

        try {
            var mp = await resolveMarketplaceProgram();
            if (!mp) throw new Error('Cannot resolve marketplace program');

            var callData = buildContractCallData('cancel_collection_offer', [
                currentWallet.address,
                collectionId
            ], 0);

            await window.lichenWallet.sendTransaction([{
                program_id: CONTRACT_PROGRAM_ID,
                accounts: [currentWallet.address, mp, collectionId],
                data: callData,
            }]);

            showToast('Collection offer cancelled', 'success');
        } catch (err) {
            showToast('Cancel collection offer failed: ' + err.message, 'error');
        }
    }

    async function handleAcceptCollectionOffer() {
        if (!currentWallet || !currentNFT) return;
        var collectionId = currentNFT.collection || currentNFT.contract_id || '';
        var tokenId = String(currentNFT.token_id || currentNFT.id || '');
        if (!collectionId || !tokenId) {
            showToast('NFT collection/token is required', 'error');
            return;
        }

        var offerer = prompt('Enter offerer address to accept collection offer:');
        if (!offerer || !offerer.trim()) return;

        try {
            var mp = await resolveMarketplaceProgram();
            if (!mp) throw new Error('Cannot resolve marketplace program');

            var callData = buildContractCallData('accept_collection_offer', [
                currentWallet.address,
                collectionId,
                tokenId,
                offerer.trim()
            ], 0);

            await window.lichenWallet.sendTransaction([{
                program_id: CONTRACT_PROGRAM_ID,
                accounts: [currentWallet.address, mp, collectionId],
                data: callData,
            }]);

            showToast('Collection offer accepted', 'success');
            await checkListingStatus();
        } catch (err) {
            showToast('Accept collection offer failed: ' + err.message, 'error');
        }
    }

    async function handleCreateAuction() {
        if (!currentWallet || !currentNFT) return;

        var startPriceInput = prompt('Start price in LICN:');
        if (!startPriceInput || isNaN(parseFloat(startPriceInput)) || parseFloat(startPriceInput) <= 0) return;
        var reserveInput = prompt('Reserve price in LICN (optional, default 0):');
        var durationInput = prompt('Duration in hours (default 24):');

        var startSpores = Math.round(parseFloat(startPriceInput) * 1e9);
        var reserveSpores = reserveInput && !isNaN(parseFloat(reserveInput)) ? Math.round(parseFloat(reserveInput) * 1e9) : 0;
        var durationHours = durationInput && !isNaN(parseFloat(durationInput)) ? Math.max(1, Math.floor(parseFloat(durationInput))) : 24;
        var now = Math.floor(Date.now() / 1000);
        var endTs = now + (durationHours * 3600);

        try {
            var mp = await resolveMarketplaceProgram();
            if (!mp) throw new Error('Cannot resolve marketplace program');
            var nftContract = currentNFT.collection || currentNFT.contract_id || '';
            var tokenId = Number(currentNFT.token_id || currentNFT.id || 0);

            var callData = buildContractCallData('create_auction', [
                currentWallet.address,
                nftContract,
                tokenId,
                startSpores,
                reserveSpores,
                '',
                now,
                endTs
            ], 0);

            await window.lichenWallet.sendTransaction([{
                program_id: CONTRACT_PROGRAM_ID,
                accounts: [currentWallet.address, mp, nftContract],
                data: callData,
            }]);

            showToast('Auction created', 'success');
            await loadAuctionState();
            await loadActivity();
        } catch (err) {
            showToast('Create auction failed: ' + err.message, 'error');
        }
    }

    async function handlePlaceBid() {
        if (!currentWallet || !currentNFT) return;

        var amountInput = prompt('Bid amount in LICN:');
        if (!amountInput || isNaN(parseFloat(amountInput)) || parseFloat(amountInput) <= 0) return;
        var bidSpores = Math.round(parseFloat(amountInput) * 1e9);

        try {
            var mp = await resolveMarketplaceProgram();
            if (!mp) throw new Error('Cannot resolve marketplace program');
            var nftContract = currentNFT.collection || currentNFT.contract_id || '';
            var tokenId = Number(currentNFT.token_id || currentNFT.id || 0);

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
            await loadAuctionState();
            await loadActivity();
        } catch (err) {
            showToast('Place bid failed: ' + err.message, 'error');
        }
    }

    async function handleSettleAuction() {
        if (!currentWallet || !currentNFT) return;
        try {
            var mp = await resolveMarketplaceProgram();
            if (!mp) throw new Error('Cannot resolve marketplace program');
            var nftContract = currentNFT.collection || currentNFT.contract_id || '';
            var tokenId = Number(currentNFT.token_id || currentNFT.id || 0);

            var callData = buildContractCallData('settle_auction', [
                currentWallet.address,
                nftContract,
                tokenId
            ], 0);

            await window.lichenWallet.sendTransaction([{
                program_id: CONTRACT_PROGRAM_ID,
                accounts: [currentWallet.address, mp, nftContract],
                data: callData,
            }]);

            showToast('Auction settled', 'success');
            await loadAuctionState();
            await checkListingStatus();
            await loadActivity();
        } catch (err) {
            showToast('Settle auction failed: ' + err.message, 'error');
        }
    }

    async function handleCancelAuction() {
        if (!currentWallet || !currentNFT) return;
        try {
            var mp = await resolveMarketplaceProgram();
            if (!mp) throw new Error('Cannot resolve marketplace program');
            var nftContract = currentNFT.collection || currentNFT.contract_id || '';
            var tokenId = Number(currentNFT.token_id || currentNFT.id || 0);

            var callData = buildContractCallData('cancel_auction', [
                currentWallet.address,
                nftContract,
                tokenId
            ], 0);

            await window.lichenWallet.sendTransaction([{
                program_id: CONTRACT_PROGRAM_ID,
                accounts: [currentWallet.address, mp, nftContract],
                data: callData,
            }]);

            showToast('Auction cancelled', 'success');
            await loadAuctionState();
            await loadActivity();
        } catch (err) {
            showToast('Cancel auction failed: ' + err.message, 'error');
        }
    }

    // ===== Load Offers =====
    async function loadOffers() {
        var offersList = document.getElementById('offersList');
        if (!offersList || !currentNFT) return;

        offersList.innerHTML = '<div style="text-align:center;padding:16px;opacity:0.5;"><i class="fas fa-spinner fa-spin"></i> Loading offers...</div>';

        try {
            var collectionId = currentNFT.collection || currentNFT.contract_id || '';
            var offers = await rpcCall('getMarketOffers', [{ collection: collectionId, token_id: currentNFT.token_id, include_collection_offers: true, limit: 50 }]);
            var offerItems = Array.isArray(offers) ? offers : ((offers && Array.isArray(offers.offers)) ? offers.offers : []);

            if (offerItems.length === 0) {
                offersList.innerHTML = '<div style="text-align:center;padding:16px;opacity:0.5;">No offers yet</div>';
                return;
            }

            var isOwner = currentWallet && currentNFT && currentNFT.owner && currentNFT.owner === currentWallet.address;

            offersList.innerHTML = offerItems.map(function (offer) {
                var price = offer.price ? fmp(sporesToLicn(offer.price), true) + ' LICN' : '?';
                var from = offer.seller || offer.buyer || offer.from || '';
                var acceptBtnHtml = isOwner ? ' <button class="btn btn-small btn-primary" onclick="window._itemAcceptOffer(\'' + escapeJsAttr(from) + '\')">Accept</button>' : '';
                return '<div style="display:flex;align-items:center;gap:12px;padding:10px 0;border-bottom:1px solid var(--border-color);">' +
                    '<i class="fas fa-hand-holding-usd" style="opacity:0.6;"></i>' +
                    '<div style="flex:1;">' +
                    '<strong>' + escapeHtml(price) + '</strong>' +
                    (from ? ' <span style="opacity:0.5;">from</span> <a href="profile.html?id=' + encodeURIComponent(from) + '" style="color:var(--accent-color);">' + formatHash(from, 8) + '</a>' : '') +
                    '</div>' +
                    acceptBtnHtml +
                    '</div>';
            }).join('');
        } catch (err) {
            offersList.innerHTML = '<div style="text-align:center;padding:16px;opacity:0.5;">No offers yet</div>';
        }
    }

    // Expose accept offer handler
    window._itemAcceptOffer = async function (offererAddress) {
        if (!currentWallet || !currentNFT) return;
        try {
            var mp = await resolveMarketplaceProgram();
            if (!mp) throw new Error('Cannot resolve marketplace program');
            var nftContract = currentNFT.collection || currentNFT.contract_id || '';
            var tokenId = String(currentNFT.token_id || currentNFT.id);
            var callData = buildContractCallData('accept_offer', [
                currentWallet.address, nftContract, tokenId, offererAddress
            ], 0);
            await window.lichenWallet.sendTransaction([{
                program_id: CONTRACT_PROGRAM_ID,
                accounts: [currentWallet.address, mp, nftContract],
                data: callData,
            }]);
            showToast('Offer accepted!', 'success');
            loadOffers();
            checkListingStatus();
        } catch (err) {
            showToast('Accept offer failed: ' + err.message, 'error');
        }
    };

    // ===== Activity =====
    async function loadActivity() {
        var activityList = document.getElementById('activityList');
        if (!activityList || !currentNFT) return;

        activityList.innerHTML = '<div style="text-align:center;padding:24px;opacity:0.5;"><i class="fas fa-spinner fa-spin"></i> Loading activity...</div>';

        try {
            var ds = window.marketplaceDataSource;
            var collectionId = currentNFT.collection || currentNFT.contract_id || '';
            var activity = ds ? await ds.getNFTActivity(collectionId) : [];
            if (!Array.isArray(activity)) activity = [];

            var tokenRef = String(currentNFT.id || '');
            var tokenIdRef = String(currentNFT.token_id || '');
            activity = activity.filter(function (item) {
                if (tokenRef && item.token && String(item.token) === tokenRef) return true;
                if (tokenIdRef && item.token_id !== undefined && String(item.token_id) === tokenIdRef) return true;
                return false;
            });

            if (activity.length === 0) {
                activityList.innerHTML = '<div style="text-align:center;padding:24px;opacity:0.5;">No activity yet</div>';
                return;
            }

            activityList.innerHTML = activity.slice(0, 20).map(function (event) {
                var icon = event.type === 'sale' ? 'fa-shopping-cart' :
                    event.type === 'listing' ? 'fa-tag' :
                    event.type === 'transfer' ? 'fa-exchange-alt' :
                    event.type === 'offer' ? 'fa-hand-holding-usd' :
                    event.type === 'mint' ? 'fa-plus-circle' : 'fa-clock';
                var price = event.price ? fmp(sporesToLicn(event.price), true) + ' LICN' : '';
                return '<div class="activity-item" style="display:flex;align-items:center;gap:12px;padding:10px 0;border-bottom:1px solid var(--border-color);">' +
                    '<i class="fas ' + icon + '" style="width:20px;text-align:center;opacity:0.6;"></i>' +
                    '<div style="flex:1;">' +
                    '<strong style="text-transform:capitalize;">' + escapeHtml(event.type || event.kind || 'Event') + '</strong>' +
                    (event.from ? ' <span style="opacity:0.5;">by</span> <a href="profile.html?id=' + encodeURIComponent(event.from) + '" style="color:var(--accent-color);">' + formatHash(event.from, 8) + '</a>' : '') +
                    '</div>' +
                    (price ? '<strong>' + escapeHtml(price) + '</strong>' : '') +
                    '<span style="font-size:12px;opacity:0.5;">' + (event.timestamp ? timeAgo(event.timestamp) : '') + '</span>' +
                    '</div>';
            }).join('');

        } catch (err) {
            activityList.innerHTML = '<div style="text-align:center;padding:24px;opacity:0.5;">Failed to load activity</div>';
        }
    }

    // ===== More from Collection =====
    async function loadMoreFromCollection() {
        var grid = document.getElementById('moreFromCollection');
        if (!grid || !currentNFT) return;

        grid.innerHTML = '<div style="text-align:center;padding:24px;opacity:0.5;grid-column:1/-1;"><i class="fas fa-spinner fa-spin"></i> Loading...</div>';

        try {
            var ds = window.marketplaceDataSource;
            var collectionId = currentNFT.collection || currentNFT.contract_id || '';
            var items = [];
            if (ds && collectionId) {
                items = await ds.getNFTsByCollection(collectionId);
            }
            if (!Array.isArray(items)) items = [];

            // Filter out current NFT
            items = items.filter(function (n) { return n.id !== currentNFT.id; }).slice(0, 8);

            if (items.length === 0) {
                grid.innerHTML = '<div style="text-align:center;padding:24px;opacity:0.5;grid-column:1/-1;">No other NFTs in this collection</div>';
                return;
            }

            grid.innerHTML = items.map(function (nft) {
                var rawUrl = nft.image || '';
                var safeUrl = safeImageUrl(rawUrl);
                var img = safeUrl
                    ? '<img src="' + encodeURI(safeUrl) + '" style="width:100%;height:100%;object-fit:cover;" alt="">'
                    : '<div style="width:100%;height:100%;background:' + gradientFromHash(nft.id || nft.name) + ';display:flex;align-items:center;justify-content:center;font-size:36px;opacity:0.5;">\uD83D\uDDBC\uFE0F</div>';
                var price = nft.price ? fmp(sporesToLicn(nft.price), true) + ' LICN' : 'Not Listed';
                return '<a href="item.html?id=' + encodeURIComponent(nft.id || '') + '&contract=' + encodeURIComponent(nft.collection || '') + '&token=' + encodeURIComponent(nft.token_id || '') + '" class="nft-card">' +
                    '<div class="nft-image">' + img + '</div>' +
                    '<div class="nft-info">' +
                    '<div class="nft-token" style="display:none;">' + escapeHtml(nft.id || nft.token) + '</div>' +
                    '<div class="nft-name">' + escapeHtml(nft.name || '#' + (nft.token_id || nft.id || '')) + '</div>' +
                    '<div class="nft-footer"><div class="nft-price"><span class="nft-price-value">' + escapeHtml(price) + '</span></div></div>' +
                    '</div></a>';
            }).join('');

        } catch (err) {
            grid.innerHTML = '<div style="text-align:center;padding:24px;opacity:0.5;grid-column:1/-1;">Failed to load collection</div>';
        }
    }

    // ===== Events =====
    function setupEvents() {
        if (window.LichenWallet) {
            window.lichenWallet = window.lichenWallet || new LichenWallet({ rpcUrl: RPC_URL });
            window.lichenWallet.bindConnectButton('#connectWallet');
            window.lichenWallet.onConnect(function (info) {
                currentWallet = info;
                updateNav();
                refreshBalance().then(function () {
                    updateActionButtons();
                    loadOffers();
                    checkListingStatus();
                    updateFavoriteUI();
                });
            });
            window.lichenWallet.onDisconnect(function () {
                currentWallet = null;
                userBalance = 0;
                updateNav();
                updateActionButtons();
                loadOffers();
                updateFavoriteUI();
            });
        }

        var favoriteBtn = document.getElementById('favoriteToggleBtn');
        if (favoriteBtn) {
            favoriteBtn.addEventListener('click', function () {
                toggleFavorite();
            });
        }

        var searchInput = document.getElementById('searchInput');
        if (searchInput) {
            searchInput.addEventListener('keypress', function (e) {
                if (e.key === 'Enter') {
                    var q = searchInput.value.trim();
                    if (q) window.location.href = 'browse.html?q=' + encodeURIComponent(q);
                }
            });
        }

        var navToggle = document.getElementById('navToggle');
        if (navToggle) {
            navToggle.addEventListener('click', function () {
                var navMenu = document.querySelector('.nav-menu');
                if (navMenu) navMenu.classList.toggle('active');
            });
        }

        if (typeof initMarketNetworkSelector === 'function') initMarketNetworkSelector();

        // Refresh button
        var refreshBtns = document.querySelectorAll('.icon-btn');
        refreshBtns.forEach(function (btn) {
            if (btn.title === 'Refresh') {
                btn.addEventListener('click', function () {
                    loadNFT();
                });
            }
        });
    }

    // ===== Init =====
    document.addEventListener('DOMContentLoaded', function () {
        console.log('Lichen Market Item loading...');
        setupEvents();
        updateNav();
        loadNFT();
        console.log('Lichen Market Item ready');
    });
})();
