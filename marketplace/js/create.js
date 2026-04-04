// Lichen Market — Create / Mint NFT Page
// Full wallet gating, user's own collections, balance check, price breakdown,
// new collection creation, proper on-chain transactions

(function () {
    'use strict';

    var RPC_URL = (window.lichenMarketConfig && window.lichenMarketConfig.rpcUrl) || (typeof LICHEN_CONFIG !== 'undefined' && typeof LICHEN_CONFIG.rpc === 'function' ? LICHEN_CONFIG.rpc() : 'https://rpc.lichen.network');
    var CONTRACT_PROGRAM_ID = null; // resolved lazily
    var SYSTEM_PROGRAM_ID = null;   // resolved lazily
    var MOSS_STORAGE_PROGRAM_ID = null; // resolved lazily
    var MINTING_FEE = 0.5; // default — overridden by on-chain value at init\n    var _mintingFeeLoaded = false;
    var CREATE_COLLECTION_OPCODE = 6;
    var MINT_NFT_OPCODE = 7;

    var currentWallet = null;
    var uploadedFile = null;
    var uploadedDataUrl = null;
    var properties = [];
    var userCollections = [];
    var userBalance = 0;
    var marketplaceProgram = null;
    var marketTrustedRpcCall = window.marketTrustedRpcCall || rpcCall;

    function lazyAddresses() {
        if (!SYSTEM_PROGRAM_ID) SYSTEM_PROGRAM_ID = bs58encode(new Uint8Array(32));
    }

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

    function encodeU64LE(value) {
        var bytes = new Uint8Array(8);
        var view = new DataView(bytes.buffer);
        view.setBigUint64(0, BigInt(value), true);
        return bytes;
    }

    function encodeU16LE(value) {
        var bytes = new Uint8Array(2);
        var view = new DataView(bytes.buffer);
        view.setUint16(0, Number(value), true);
        return bytes;
    }

    function encodeBincodeString(value) {
        var strBytes = new TextEncoder().encode(value || '');
        var lenBytes = encodeU64LE(strBytes.length);
        var out = new Uint8Array(lenBytes.length + strBytes.length);
        out.set(lenBytes, 0);
        out.set(strBytes, lenBytes.length);
        return out;
    }

    function concatBytes(parts) {
        var total = 0;
        for (var i = 0; i < parts.length; i++) total += parts[i].length;
        var out = new Uint8Array(total);
        var offset = 0;
        for (var j = 0; j < parts.length; j++) {
            out.set(parts[j], offset);
            offset += parts[j].length;
        }
        return out;
    }

    async function sha256Bytes(bytes) {
        var digest = await crypto.subtle.digest('SHA-256', bytes);
        return new Uint8Array(digest);
    }

    async function hashFileToBase58(file) {
        var buffer = await file.arrayBuffer();
        var digest = await sha256Bytes(buffer);
        return bs58encode(digest);
    }

    function utf8ToBase64(input) {
        var bytes = new TextEncoder().encode(input || '');
        var binary = '';
        for (var i = 0; i < bytes.length; i++) binary += String.fromCharCode(bytes[i]);
        return btoa(binary);
    }

    function makeTokenBaseId() {
        return Date.now() * 1000 + Math.floor(Math.random() * 1000);
    }

    function buildContractCallData(functionName, args, value) {
        var argBytes = Array.from(new TextEncoder().encode(JSON.stringify(args || [])));
        return JSON.stringify({ Call: { function: functionName, args: argBytes, value: value || 0 } });
    }

    async function resolveMarketplaceProgram() {
        if (marketplaceProgram) return marketplaceProgram;
        var entry = await marketTrustedRpcCall('getSymbolRegistry', ['LICHENMARKET']);
        marketplaceProgram = entry && (entry.program || entry.program_id) ? (entry.program || entry.program_id) : null;
        if (!marketplaceProgram) throw new Error('Marketplace program not found in symbol registry');
        CONTRACT_PROGRAM_ID = marketplaceProgram;
        return marketplaceProgram;
    }

    async function resolveMossStorageProgram() {
        lazyAddresses();
        if (MOSS_STORAGE_PROGRAM_ID) return MOSS_STORAGE_PROGRAM_ID;
        var entry = await marketTrustedRpcCall('getSymbolRegistry', ['MOSS']);
        var candidate = null;
        if (typeof entry === 'string') {
            candidate = entry;
        } else if (entry && typeof entry === 'object') {
            candidate = entry.id || entry.program_id || entry.contract_id || null;
        }
        if (!candidate) throw new Error('Moss storage contract not found in symbol registry');
        MOSS_STORAGE_PROGRAM_ID = candidate;
        return MOSS_STORAGE_PROGRAM_ID;
    }

    function estimateMossStorageCost(sizeBytes, replicationFactor, durationSlots) {
        var size = BigInt(Math.max(1, Number(sizeBytes) || 1));
        var replication = BigInt(Math.max(1, Number(replicationFactor) || 1));
        var duration = BigInt(Math.max(1000, Number(durationSlots) || 1000));
        var rewardPerSlotPerByte = 10n;
        var baseCost = size * replication * duration * rewardPerSlotPerByte;
        var buffered = baseCost + (baseCost / 5n);
        if (buffered > BigInt(Number.MAX_SAFE_INTEGER)) {
            throw new Error('Metadata storage cost exceeds wallet transaction limits');
        }
        return Number(buffered);
    }

    async function storeMetadataOnMoss(metadataObj) {
        var mossProgram = await resolveMossStorageProgram();
        var metadataJson = JSON.stringify(metadataObj);
        var metadataBytes = new TextEncoder().encode(metadataJson);
        var metadataHash = bs58encode(await sha256Bytes(metadataBytes));
        var replicationFactor = 1;
        var durationSlots = 1000;
        var storageValue = estimateMossStorageCost(metadataBytes.length, replicationFactor, durationSlots);

        var callData = buildContractCallData('store_data', [
            currentWallet.address,
            metadataHash,
            metadataBytes.length,
            replicationFactor,
            durationSlots
        ], storageValue);

        await window.lichenWallet.sendTransaction([{
            program_id: mossProgram,
            accounts: [currentWallet.address],
            data: callData,
        }]);

        return 'moss://' + metadataHash;
    }

    async function deriveCollectionAccount(creatorAddress, name, symbol) {
        var creatorBytes = bs58decode(creatorAddress);
        var seedBytes = new TextEncoder().encode(name + '|' + symbol + '|' + Date.now() + '|' + Math.random());
        var preimage = new Uint8Array(creatorBytes.length + seedBytes.length);
        preimage.set(creatorBytes, 0);
        preimage.set(seedBytes, creatorBytes.length);
        var digest = await sha256Bytes(preimage);
        return bs58encode(digest);
    }

    function buildCreateCollectionInstructionData(name, symbol, royaltyBps, maxSupply, publicMint) {
        var payload = concatBytes([
            encodeBincodeString(name),
            encodeBincodeString(symbol),
            encodeU16LE(royaltyBps),
            encodeU64LE(maxSupply),
            new Uint8Array([publicMint ? 1 : 0]),
            new Uint8Array([0]) // Option<Pubkey>::None for mint_authority
        ]);
        var out = new Uint8Array(1 + payload.length);
        out[0] = CREATE_COLLECTION_OPCODE;
        out.set(payload, 1);
        return out;
    }

    function buildMintInstructionData(tokenId, metadataUri) {
        var payload = concatBytes([
            encodeU64LE(tokenId),
            encodeBincodeString(metadataUri)
        ]);
        var out = new Uint8Array(1 + payload.length);
        out[0] = MINT_NFT_OPCODE;
        out.set(payload, 1);
        return out;
    }

    async function deriveTokenAccount(collectionAddress, tokenId) {
        var collectionBytes = bs58decode(collectionAddress);
        if (!collectionBytes || collectionBytes.length !== 32) {
            throw new Error('Invalid collection address for token derivation');
        }
        var numericTokenId = Number(tokenId);
        if (!Number.isFinite(numericTokenId) || numericTokenId < 0) {
            throw new Error('Invalid token ID for token derivation');
        }
        var tokenIdBytes = encodeU64LE(tokenId);
        var preimage = new Uint8Array(collectionBytes.length + tokenIdBytes.length);
        preimage.set(collectionBytes, 0);
        preimage.set(tokenIdBytes, collectionBytes.length);
        var digest = await sha256Bytes(preimage);
        return bs58encode(digest);
    }

    function setText(id, value) {
        var el = document.getElementById(id);
        if (el) el.textContent = value;
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

    // ===== Wallet Gating =====
    function updateWalletGate() {
        var overlay = document.getElementById('walletRequiredOverlay');
        var createForm = document.querySelector('.create-form');
        var createPreview = document.querySelector('.create-preview');

        if (!currentWallet) {
            // Create overlay if needed
            if (!overlay) {
                overlay = document.createElement('div');
                overlay.id = 'walletRequiredOverlay';
                overlay.style.cssText = 'position:fixed;top:0;left:0;right:0;bottom:0;background:rgba(10,10,20,0.92);z-index:100;display:flex;align-items:center;justify-content:center;';
                overlay.innerHTML = '<div style="text-align:center;color:#fff;max-width:420px;padding:48px;">' +
                    '<i class="fas fa-wallet" style="font-size:64px;margin-bottom:24px;color:var(--accent-color,#f97316);"></i>' +
                    '<h2 style="margin-bottom:12px;font-size:1.5rem;">Connect Your Wallet</h2>' +
                    '<p style="opacity:0.7;margin-bottom:24px;line-height:1.6;">You need to connect your wallet to create collections and mint NFTs on Lichen.</p>' +
                    '<button class="btn btn-primary btn-large" id="overlayConnectBtn"><i class="fas fa-wallet"></i> Connect Wallet</button>' +
                    '</div>';
                document.body.appendChild(overlay);
                document.getElementById('overlayConnectBtn').addEventListener('click', function () {
                    if (window.lichenWallet) window.lichenWallet._openWalletModal();
                });
            }
            overlay.style.display = 'flex';
            if (createForm) { createForm.style.opacity = '0.15'; createForm.style.pointerEvents = 'none'; }
            if (createPreview) { createPreview.style.opacity = '0.15'; }
        } else {
            if (overlay) overlay.style.display = 'none';
            if (createForm) { createForm.style.opacity = '1'; createForm.style.pointerEvents = 'auto'; }
            if (createPreview) { createPreview.style.opacity = '1'; }
            loadUserCollections();
            refreshBalance();
        }
        updateNav();
        updateCreateBtnState();
    }

    // ===== Balance =====
    async function refreshBalance() {
        if (!currentWallet) return;
        try {
            var ds = window.marketplaceDataSource;
            if (ds) {
                userBalance = await ds.getWalletBalance(currentWallet.address);
            }
        } catch (_) {
            userBalance = 0;
        }
        updatePriceBreakdown();
        updateCreateBtnState();
    }

    // ===== Price Breakdown =====
    function updatePriceBreakdown() {
        var supply = parseInt((document.getElementById('nftSupply') || {}).value) || 1;
        var isNewCollection = (document.getElementById('nftCollection') || {}).value === 'new';
        var collectionFee = isNewCollection ? 1.0 : 0;
        var totalMintFee = MINTING_FEE * supply;
        var total = totalMintFee + collectionFee;
        var hasBalance = currentWallet && userBalance >= total;

        var breakdown = document.getElementById('priceBreakdown');
        if (!breakdown) return;

        var html = '';
        if (isNewCollection) {
            html += '<div class="detail-row"><span>Collection Deployment</span><strong>' + collectionFee.toFixed(3) + ' LICN</strong></div>';
        }
        html += '<div class="detail-row"><span>Minting Fee (' + supply + '&times;)</span><strong>' + totalMintFee.toFixed(3) + ' LICN</strong></div>';
        html += '<div class="detail-row" style="border-top:1px solid var(--border-color);padding-top:8px;margin-top:8px;">' +
            '<span><strong>Total</strong></span><strong>' + total.toFixed(3) + ' LICN</strong></div>';

        if (currentWallet) {
            html += '<div class="detail-row"><span>Your Balance</span><strong style="color:' +
                (hasBalance ? '#22c55e' : '#ef4444') + ';">' + userBalance.toFixed(4) + ' LICN</strong></div>';
            if (!hasBalance) {
                html += '<div style="color:#ef4444;font-size:13px;margin-top:8px;"><i class="fas fa-exclamation-triangle"></i> Insufficient balance</div>';
            }
        }
        breakdown.innerHTML = html;
    }

    // ===== Load User Collections =====
    async function loadUserCollections() {
        var select = document.getElementById('nftCollection');
        if (!select || !currentWallet) return;

        // Remove all options except placeholders
        while (select.options.length > 2) {
            select.remove(2);
        }

        userCollections = [];
        try {
            var ds = window.marketplaceDataSource;
            if (ds) {
                userCollections = await ds.getUserCollections(currentWallet.address);
            }
        } catch (err) {
            console.warn('Failed to load user collections:', err);
        }

        userCollections.forEach(function (col) {
            var option = document.createElement('option');
            option.value = col.id;
            option.textContent = col.name + (col.symbol ? ' (' + col.symbol + ')' : '');
            select.appendChild(option);
        });

        // If no collections, auto-select create new
        if (userCollections.length === 0 && select.value !== 'new') {
            // Don't auto-select, let user choose
        }
        handleCollectionChange();
    }

    // ===== Collection Dropdown Change =====
    function handleCollectionChange() {
        var select = document.getElementById('nftCollection');
        var newFields = document.getElementById('newCollectionFields');

        if (select && newFields) {
            newFields.style.display = select.value === 'new' ? 'block' : 'none';
        }

        // Update preview
        var previewCol = document.getElementById('previewCollection');
        if (previewCol && select) {
            if (select.value === 'new') {
                var nameInput = document.getElementById('newCollectionName');
                previewCol.textContent = (nameInput && nameInput.value.trim()) || 'New Collection';
            } else if (select.value) {
                var selected = select.options[select.selectedIndex];
                previewCol.textContent = selected ? selected.text : 'No Collection';
            } else {
                previewCol.textContent = 'Select a Collection';
            }
        }
        updatePriceBreakdown();
    }

    // ===== Create Button State =====
    function updateCreateBtnState() {
        var createBtn = document.getElementById('createBtn');
        if (!createBtn) return;

        if (!currentWallet) {
            createBtn.disabled = true;
            createBtn.title = 'Connect wallet to mint';
            createBtn.innerHTML = '<i class="fas fa-wallet"></i> Connect Wallet to Mint';
            return;
        }

        var supply = parseInt((document.getElementById('nftSupply') || {}).value) || 1;
        var isNewCollection = (document.getElementById('nftCollection') || {}).value === 'new';
        var collectionFee = isNewCollection ? 1.0 : 0;
        var totalCost = (MINTING_FEE * supply) + collectionFee;

        if (userBalance < totalCost) {
            createBtn.disabled = true;
            createBtn.title = 'Insufficient balance (need ' + totalCost.toFixed(3) + ' LICN)';
            createBtn.innerHTML = '<i class="fas fa-exclamation-triangle"></i> Insufficient Balance';
            return;
        }

        createBtn.disabled = false;
        createBtn.title = '';
        createBtn.innerHTML = '<i class="fas fa-rocket"></i> Create & Mint NFT';
    }

    // ===== File Upload =====
    function setupUpload() {
        var uploadArea = document.getElementById('uploadArea');
        var fileInput = document.getElementById('fileInput');
        if (!uploadArea || !fileInput) return;

        uploadArea.addEventListener('click', function () { fileInput.click(); });

        uploadArea.addEventListener('dragover', function (e) {
            e.preventDefault(); e.stopPropagation();
            uploadArea.classList.add('drag-over');
        });
        uploadArea.addEventListener('dragleave', function (e) {
            e.preventDefault(); e.stopPropagation();
            uploadArea.classList.remove('drag-over');
        });
        uploadArea.addEventListener('drop', function (e) {
            e.preventDefault(); e.stopPropagation();
            uploadArea.classList.remove('drag-over');
            if (e.dataTransfer.files.length > 0) handleFile(e.dataTransfer.files[0]);
        });
        fileInput.addEventListener('change', function () {
            if (fileInput.files.length > 0) handleFile(fileInput.files[0]);
        });
    }

    function handleFile(file) {
        var allowedTypes = ['image/png', 'image/jpeg', 'image/gif', 'image/webp', 'image/svg+xml', 'video/mp4', 'video/webm', 'audio/mpeg', 'audio/wav'];
        if (allowedTypes.indexOf(file.type) === -1) {
            alert('Unsupported file type. Upload image, video, or audio.');
            return;
        }
        if (file.size > 50 * 1024 * 1024) {
            alert('File too large. Maximum 50MB.');
            return;
        }
        uploadedFile = file;
        var reader = new FileReader();
        reader.onload = function (e) {
            uploadedDataUrl = e.target.result;
            showFilePreview(file, e.target.result);
            updateLivePreviewImage(e.target.result, file.type);
        };
        reader.readAsDataURL(file);
    }

    function showFilePreview(file, dataUrl) {
        var filePreview = document.getElementById('filePreview');
        var uploadArea = document.getElementById('uploadArea');
        if (!filePreview) return;
        var isVideo = file.type.startsWith('video/');
        var isAudio = file.type.startsWith('audio/');
        var html = '<div style="position: relative; text-align: center;">';
        if (isVideo) {
            html += '<video src="' + dataUrl + '" controls style="max-width:100%;max-height:300px;border-radius:8px;"></video>';
        } else if (isAudio) {
            html += '<div style="padding:40px;"><i class="fas fa-music" style="font-size:48px;color:var(--accent-color);"></i></div>';
            html += '<audio src="' + dataUrl + '" controls style="width:100%;margin-top:12px;"></audio>';
        } else {
            html += '<img src="' + dataUrl + '" style="max-width:100%;max-height:300px;border-radius:8px;" alt="Preview">';
        }
        html += '<button type="button" data-create-action="remove-file" style="position:absolute;top:8px;right:8px;background:rgba(0,0,0,0.7);color:white;border:none;border-radius:50%;width:32px;height:32px;cursor:pointer;font-size:16px;">&times;</button>';
        html += '<div style="margin-top:8px;font-size:13px;color:var(--text-secondary);">' + escapeHtml(file.name) + ' (' + (file.size / 1024).toFixed(1) + ' KB)</div>';
        html += '</div>';
        filePreview.innerHTML = html;
        filePreview.style.display = 'block';
        if (uploadArea) uploadArea.style.display = 'none';
    }

    function removeFile() {
        uploadedFile = null;
        uploadedDataUrl = null;
        var filePreview = document.getElementById('filePreview');
        var uploadArea = document.getElementById('uploadArea');
        var fileInput = document.getElementById('fileInput');
        if (filePreview) { filePreview.innerHTML = ''; filePreview.style.display = 'none'; }
        if (uploadArea) uploadArea.style.display = '';
        if (fileInput) fileInput.value = '';
        updateLivePreviewImage(null, null);
    }

    // ===== Properties (Traits) =====
    function addProperty() {
        properties.push({ trait_type: '', value: '' });
        renderProperties();
    }

    function renderProperties() {
        var container = document.getElementById('propertiesList');
        if (!container) return;
        container.innerHTML = properties.map(function (prop, index) {
            return '<div class="property-row" style="display:flex;gap:8px;margin-bottom:8px;align-items:center;">' +
                '<input type="text" placeholder="Trait type" value="' + escapeHtml(prop.trait_type || '') + '" ' +
                'data-property-index="' + index + '" data-property-field="trait_type" ' +
                'style="flex:1;padding:8px 12px;border:1px solid var(--border-color);border-radius:8px;background:var(--bg-secondary);color:var(--text-primary);">' +
                '<input type="text" placeholder="Value" value="' + escapeHtml(prop.value || '') + '" ' +
                'data-property-index="' + index + '" data-property-field="value" ' +
                'style="flex:1;padding:8px 12px;border:1px solid var(--border-color);border-radius:8px;background:var(--bg-secondary);color:var(--text-primary);">' +
                '<button type="button" data-create-action="remove-property" data-property-index="' + index + '" ' +
                'style="background:none;border:none;color:var(--text-secondary);cursor:pointer;font-size:18px;padding:4px 8px;">&times;</button></div>';
        }).join('');
    }

    // ===== Live Preview =====
    function setupLivePreview() {
        var nameInput = document.getElementById('nftName');
        var collectionSelect = document.getElementById('nftCollection');
        var supplyInput = document.getElementById('nftSupply');
        var royaltyInput = document.getElementById('nftRoyalty');

        if (nameInput) {
            nameInput.addEventListener('input', function () {
                setText('previewName', nameInput.value || 'NFT Name');
            });
        }
        if (collectionSelect) {
            collectionSelect.addEventListener('change', handleCollectionChange);
        }
        if (supplyInput) {
            supplyInput.addEventListener('input', function () {
                setText('previewSupply', supplyInput.value || '1');
                updatePriceBreakdown();
                updateCreateBtnState();
            });
        }
        if (royaltyInput) {
            royaltyInput.addEventListener('input', function () {
                setText('previewRoyalty', (royaltyInput.value || '0') + '%');
            });
        }

        var listingPriceInput = document.getElementById('nftListingPrice');
        if (listingPriceInput) {
            listingPriceInput.addEventListener('input', function () {
                var price = parseFloat(listingPriceInput.value);
                var previewPrice = document.getElementById('previewPrice');
                if (previewPrice) {
                    previewPrice.textContent = (price > 0) ? (price + ' LICN') : 'Not for sale';
                }
            });
        }
    }

    function updateLivePreviewImage(dataUrl, fileType) {
        var previewImage = document.getElementById('previewImage');
        if (!previewImage) return;
        if (dataUrl && fileType && fileType.startsWith('image/')) {
            previewImage.style.background = 'none';
            previewImage.innerHTML = '<img src="' + dataUrl + '" style="width:100%;height:100%;object-fit:cover;border-radius:12px;" alt="Preview">';
        } else if (dataUrl && fileType && fileType.startsWith('video/')) {
            previewImage.style.background = 'none';
            previewImage.innerHTML = '<video src="' + dataUrl + '" style="width:100%;height:100%;object-fit:cover;border-radius:12px;" muted></video>';
        } else {
            previewImage.style.background = gradientFromHash('preview-default');
            previewImage.innerHTML = '<div style="display:flex;align-items:center;justify-content:center;height:100%;font-size:48px;opacity:0.3;">\uD83D\uDDBC\uFE0F</div>';
        }
    }

    // ===== Create Collection =====
    async function createNewCollection(name, symbol) {
        lazyAddresses();
        if (!currentWallet || !window.lichenWallet) {
            throw new Error('Wallet not connected');
        }
        var collectionAddress = await deriveCollectionAccount(currentWallet.address, name, symbol);
        var ixData = buildCreateCollectionInstructionData(name, symbol, 250, 1000000, true);
        await window.lichenWallet.sendTransaction([{
            program_id: SYSTEM_PROGRAM_ID,
            accounts: [currentWallet.address, collectionAddress],
            data: ixData,
        }]);
        return collectionAddress;
    }

    // ===== Mint NFT =====
    async function mintNFT() {
        lazyAddresses();
        if (!currentWallet) {
            alert('Please connect your wallet first');
            return;
        }

        var nameInput = document.getElementById('nftName');
        var descInput = document.getElementById('nftDescription');
        var collectionSelect = document.getElementById('nftCollection');
        var supplyInput = document.getElementById('nftSupply');
        var royaltyInput = document.getElementById('nftRoyalty');

        var name = nameInput ? nameInput.value.trim() : '';
        var description = descInput ? descInput.value.trim() : '';
        var collection = collectionSelect ? collectionSelect.value : '';
        var supply = supplyInput ? parseInt(supplyInput.value) || 1 : 1;
        var royalty = royaltyInput ? parseFloat(royaltyInput.value) || 0 : 0;

        if (!name) { alert('Please enter an NFT name'); if (nameInput) nameInput.focus(); return; }
        if (name.length > 128) { alert('NFT name must be 128 characters or fewer'); return; }
        if (description.length > 2048) { alert('Description must be 2048 characters or fewer'); return; }
        if (!uploadedFile) { alert('Please upload an image or media file'); return; }
        if (supply < 1 || supply > 1000) { alert('Supply must be 1–1000'); return; }
        if (royalty < 0 || royalty > 10) { alert('Royalty must be 0–10%'); return; }
        if (!collection) { alert('Please select a collection or create a new one'); return; }

        // Validate new collection fields
        if (collection === 'new') {
            var colNameInput = document.getElementById('newCollectionName');
            if (!colNameInput || !colNameInput.value.trim()) {
                alert('Please enter a name for your new collection');
                if (colNameInput) colNameInput.focus();
                return;
            }
        }

        // Balance check
        var isNewCol = collection === 'new';
        var collectionFee = isNewCol ? 1.0 : 0;
        var totalCost = (MINTING_FEE * supply) + collectionFee;
        await refreshBalance();
        if (userBalance < totalCost) {
            alert('Insufficient balance. Need ' + totalCost.toFixed(3) + ' LICN, you have ' + userBalance.toFixed(4) + ' LICN.');
            return;
        }

        if (!window.lichenWallet || typeof window.lichenWallet.sendTransaction !== 'function') {
            alert('Wallet signing unavailable. Reconnect wallet and try again.');
            return;
        }

        var createBtn = document.getElementById('createBtn');
        if (createBtn) { createBtn.disabled = true; createBtn.innerHTML = '<i class="fas fa-spinner fa-spin"></i> Creating...'; }

        try {
            var collectionAddress = collection;

            // Create new collection if needed
            if (collection === 'new') {
                var colName = document.getElementById('newCollectionName').value.trim();
                var colSymbolInput = document.getElementById('newCollectionSymbol');
                var colSymbol = colSymbolInput ? colSymbolInput.value.trim() : '';
                if (!colSymbol) colSymbol = colName.substring(0, 6).toUpperCase().replace(/[^A-Z0-9]/g, '');

                if (createBtn) createBtn.innerHTML = '<i class="fas fa-spinner fa-spin"></i> Creating Collection...';
                collectionAddress = await createNewCollection(colName, colSymbol);
                showToast('Collection "' + colName + '" created!', 'success');
            }

            var mediaHash = await hashFileToBase58(uploadedFile);

            // Build metadata (stored via Moss hash URI)
            var metadata = {
                name: name,
                description: description,
                image: 'moss://' + mediaHash,
                media_hash: mediaHash,
                media_type: uploadedFile.type,
                media_size: uploadedFile.size,
                properties: properties.filter(function (p) { return p.trait_type && p.value; }),
                creator: currentWallet.address,
                supply: supply,
                royalty: royalty,
            };

            if (createBtn) createBtn.innerHTML = '<i class="fas fa-spinner fa-spin"></i> Storing metadata...';
            var metadataUri = await storeMetadataOnMoss(metadata);
            var tokenBaseId = makeTokenBaseId();
            var mintedTokenIds = [];

            for (var i = 0; i < supply; i++) {
                if (createBtn) createBtn.innerHTML = '<i class="fas fa-spinner fa-spin"></i> Minting ' + (i + 1) + '/' + supply + '...';

                var tokenId = tokenBaseId + i;
                var tokenAccount = await deriveTokenAccount(collectionAddress, tokenId);
                var instructionData = buildMintInstructionData(tokenId, metadataUri);

                await window.lichenWallet.sendTransaction([{
                    program_id: SYSTEM_PROGRAM_ID,
                    accounts: [currentWallet.address, collectionAddress, tokenAccount, currentWallet.address],
                    data: instructionData,
                }]);

                try {
                    var mintedToken = await rpcCall('getNFT', [collectionAddress, tokenId]);
                    if (mintedToken && mintedToken.token && mintedToken.token !== tokenAccount) {
                        throw new Error('Token account derivation mismatch with runtime for token #' + tokenId);
                    }
                } catch (verifyErr) {
                    if (verifyErr && verifyErr.message && verifyErr.message.indexOf('derivation mismatch') !== -1) {
                        throw verifyErr;
                    }
                    console.warn('Skipping post-mint derivation verification:', verifyErr);
                }

                mintedTokenIds.push(tokenId);
            }

            var mintedSummary = mintedTokenIds.length === 1
                ? '#' + mintedTokenIds[0]
                : ('#' + mintedTokenIds[0] + ' – #' + mintedTokenIds[mintedTokenIds.length - 1]);

            showToast('NFT "' + name + '" minted! Token IDs: ' + mintedSummary, 'success');

            // Auto-list for sale if listing price is set
            var listingPriceInput = document.getElementById('nftListingPrice');
            var listingPrice = listingPriceInput ? parseFloat(listingPriceInput.value) : 0;
            if (listingPrice > 0) {
                try {
                    if (createBtn) createBtn.innerHTML = '<i class="fas fa-spinner fa-spin"></i> Listing for sale...';
                    var mp = await resolveMarketplaceProgram();
                    var priceSpores = Math.round(listingPrice * 1e9);
                    var paymentToken = bs58encode(new Uint8Array(32)); // native LICN
                    var royaltyBps = Math.max(0, Math.min(5000, Math.round(Number(royalty || 0) * 100)));
                    var royaltyRecipient = currentWallet.address;

                    for (var li = 0; li < mintedTokenIds.length; li++) {
                        var listCallData;
                        if (royaltyBps > 0) {
                            listCallData = buildContractCallData('list_nft_with_royalty', [
                                currentWallet.address,
                                collectionAddress,
                                String(mintedTokenIds[li]),
                                priceSpores,
                                paymentToken,
                                royaltyRecipient,
                                royaltyBps
                            ], 0);
                        } else {
                            listCallData = buildContractCallData('list_nft', [
                                currentWallet.address,
                                collectionAddress,
                                String(mintedTokenIds[li]),
                                priceSpores,
                                paymentToken
                            ], 0);
                        }

                        await window.lichenWallet.sendTransaction([{
                            program_id: mp,
                            accounts: [currentWallet.address, collectionAddress],
                            data: listCallData,
                        }]);
                    }
                    showToast('Listed for ' + listingPrice + ' LICN!', 'success');
                } catch (listErr) {
                    showToast('Minted successfully but listing failed: ' + listErr.message, 'error');
                }
            }

            resetForm();
            refreshBalance();
            loadUserCollections();

        } catch (err) {
            showToast('Minting failed: ' + err.message, 'error');
        } finally {
            updateCreateBtnState();
        }
    }

    function resetForm() {
        ['nftName', 'nftDescription'].forEach(function (id) {
            var el = document.getElementById(id);
            if (el) el.value = '';
        });
        var supplyEl = document.getElementById('nftSupply');
        if (supplyEl) supplyEl.value = '1';
        var royaltyEl = document.getElementById('nftRoyalty');
        if (royaltyEl) royaltyEl.value = '10';
        var colEl = document.getElementById('nftCollection');
        if (colEl) colEl.value = '';
        var newColName = document.getElementById('newCollectionName');
        if (newColName) newColName.value = '';
        var newColSymbol = document.getElementById('newCollectionSymbol');
        if (newColSymbol) newColSymbol.value = '';
        var listPriceEl = document.getElementById('nftListingPrice');
        if (listPriceEl) listPriceEl.value = '';

        properties = [];
        renderProperties();
        removeFile();
        handleCollectionChange();

        setText('previewName', 'NFT Name');
        setText('previewCollection', 'Collection');
        setText('previewSupply', '1');
        setText('previewRoyalty', '10%');
        var previewPrice = document.getElementById('previewPrice');
        if (previewPrice) previewPrice.textContent = 'Not for sale';
    }

    // ===== Event Setup =====
    function setupEvents() {
        var createBtn = document.getElementById('createBtn');
        if (createBtn) {
            createBtn.addEventListener('click', function (e) {
                e.preventDefault();
                mintNFT();
            });
        }

        var collectionSelect = document.getElementById('nftCollection');
        if (collectionSelect) {
            collectionSelect.addEventListener('change', handleCollectionChange);
        }

        var addPropertyBtn = document.getElementById('addPropertyBtn');
        if (addPropertyBtn) {
            addPropertyBtn.addEventListener('click', addProperty);
        }

        var colNameInput = document.getElementById('newCollectionName');
        if (colNameInput) {
            colNameInput.addEventListener('input', function () {
                var previewCol = document.getElementById('previewCollection');
                if (previewCol) previewCol.textContent = colNameInput.value.trim() || 'New Collection';
                updatePriceBreakdown();
            });
        }

        // Wallet
        if (window.LichenWallet) {
            window.lichenWallet = window.lichenWallet || new LichenWallet({ rpcUrl: RPC_URL });
            window.lichenWallet.bindConnectButton('#connectWallet');
            window.lichenWallet.onConnect(function (info) {
                currentWallet = info;
                updateWalletGate();
            });
            window.lichenWallet.onDisconnect(function () {
                currentWallet = null;
                userBalance = 0;
                updateWalletGate();
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

        var filePreview = document.getElementById('filePreview');
        if (filePreview) {
            filePreview.addEventListener('click', function (event) {
                var control = event.target.closest('[data-create-action="remove-file"]');
                if (!control) return;
                removeFile();
            });
        }

        var propertiesList = document.getElementById('propertiesList');
        if (propertiesList) {
            propertiesList.addEventListener('input', function (event) {
                var input = event.target.closest('[data-property-index][data-property-field]');
                if (!input) return;
                var index = parseInt(input.getAttribute('data-property-index'), 10);
                var field = input.getAttribute('data-property-field');
                if (!Number.isFinite(index) || !field || !properties[index]) return;
                properties[index][field] = input.value;
            });

            propertiesList.addEventListener('click', function (event) {
                var control = event.target.closest('[data-create-action="remove-property"]');
                if (!control) return;
                var index = parseInt(control.getAttribute('data-property-index'), 10);
                if (!Number.isFinite(index)) return;
                properties.splice(index, 1);
                renderProperties();
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

        // Network selector
        if (typeof initMarketNetworkSelector === 'function') initMarketNetworkSelector();
    }

    // Fetch on-chain minting fee from marketplace contract config
    async function loadMintingFee() {
        if (_mintingFeeLoaded) return;
        try {
            var resp = await fetch(RPC_URL, {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ jsonrpc: '2.0', id: 1, method: 'getMarketplaceConfig', params: [] })
            });
            var json = await resp.json();
            var result = json && json.result;
            if (result && result.minting_fee !== undefined) {
                MINTING_FEE = parseFloat(result.minting_fee) || 0.5;
                _mintingFeeLoaded = true;
                updatePriceBreakdown();
            }
        } catch (_) { /* use default */ }
    }

    // ===== Init =====
    document.addEventListener('DOMContentLoaded', function () {
        setupEvents();
        setupUpload();
        setupLivePreview();
        renderProperties();
        updateLivePreviewImage(null, null);
        updateWalletGate();
        updatePriceBreakdown();
        loadMintingFee(); // async — updates breakdown when complete
    });
})();
