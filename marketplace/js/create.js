// Molt Market — Create / Mint NFT Page
// Handles file upload, form validation, live preview, and minting transaction

(function () {
    'use strict';

    const RPC_URL = (window.moltMarketConfig && window.moltMarketConfig.rpcUrl) || 'http://localhost:8899';

    let currentWallet = null;
    let uploadedFile = null;
    let uploadedDataUrl = null;
    let properties = [];

    // rpcCall, formatHash provided by shared/utils.js

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

    // ===== File Upload =====
    function setupUpload() {
        var uploadArea = document.getElementById('uploadArea');
        var fileInput = document.getElementById('fileInput');
        var filePreview = document.getElementById('filePreview');

        if (!uploadArea || !fileInput) return;

        // Click to open file picker
        uploadArea.addEventListener('click', function () {
            fileInput.click();
        });

        // Drag & drop
        uploadArea.addEventListener('dragover', function (e) {
            e.preventDefault();
            e.stopPropagation();
            uploadArea.classList.add('drag-over');
        });

        uploadArea.addEventListener('dragleave', function (e) {
            e.preventDefault();
            e.stopPropagation();
            uploadArea.classList.remove('drag-over');
        });

        uploadArea.addEventListener('drop', function (e) {
            e.preventDefault();
            e.stopPropagation();
            uploadArea.classList.remove('drag-over');
            var files = e.dataTransfer.files;
            if (files.length > 0) handleFile(files[0]);
        });

        // File input change
        fileInput.addEventListener('change', function () {
            if (fileInput.files.length > 0) handleFile(fileInput.files[0]);
        });
    }

    function handleFile(file) {
        // Validate file type
        var allowedTypes = ['image/png', 'image/jpeg', 'image/gif', 'image/webp', 'image/svg+xml', 'video/mp4', 'video/webm', 'audio/mpeg', 'audio/wav'];
        if (allowedTypes.indexOf(file.type) === -1) {
            alert('Unsupported file type. Please upload an image, video, or audio file.');
            return;
        }

        // Validate file size (max 50MB)
        if (file.size > 50 * 1024 * 1024) {
            alert('File too large. Maximum size is 50MB.');
            return;
        }

        uploadedFile = file;

        // Read file for preview
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
        html += '<button onclick="window._createRemoveFile()" style="position:absolute;top:8px;right:8px;background:rgba(0,0,0,0.7);color:white;border:none;border-radius:50%;width:32px;height:32px;cursor:pointer;font-size:16px;">&times;</button>';
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
    function renderProperties() {
        var container = document.getElementById('propertiesList');
        if (!container) return;

        container.innerHTML = properties.map(function (prop, index) {
            return '<div class="property-row" style="display:flex;gap:8px;margin-bottom:8px;align-items:center;">' +
                '<input type="text" placeholder="Trait type (e.g., Background)" value="' + escapeHtml(prop.trait_type || '') + '" ' +
                'onchange="window._createUpdateProperty(' + index + ', \'trait_type\', this.value)" ' +
                'style="flex:1;padding:8px 12px;border:1px solid var(--border-color);border-radius:8px;background:var(--bg-secondary);color:var(--text-primary);">' +
                '<input type="text" placeholder="Value (e.g., Blue)" value="' + escapeHtml(prop.value || '') + '" ' +
                'onchange="window._createUpdateProperty(' + index + ', \'value\', this.value)" ' +
                'style="flex:1;padding:8px 12px;border:1px solid var(--border-color);border-radius:8px;background:var(--bg-secondary);color:var(--text-primary);">' +
                '<button onclick="window._createRemoveProperty(' + index + ')" ' +
                'style="background:none;border:none;color:var(--text-secondary);cursor:pointer;font-size:18px;padding:4px 8px;">&times;</button>' +
                '</div>';
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
                setText('previewName', nameInput.value || 'Untitled');
            });
        }

        if (collectionSelect) {
            collectionSelect.addEventListener('change', function () {
                var selected = collectionSelect.options[collectionSelect.selectedIndex];
                setText('previewCollection', selected ? selected.text : 'No Collection');
            });
        }

        if (supplyInput) {
            supplyInput.addEventListener('input', function () {
                setText('previewSupply', supplyInput.value || '1');
            });
        }

        if (royaltyInput) {
            royaltyInput.addEventListener('input', function () {
                setText('previewRoyalty', (royaltyInput.value || '0') + '%');
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
            previewImage.innerHTML = '<div style="display:flex;align-items:center;justify-content:center;height:100%;font-size:48px;opacity:0.3;">🖼️</div>';
        }
    }

    // ===== Mint NFT =====
    async function mintNFT() {
        // Validate wallet
        if (!currentWallet) {
            alert('Please connect your wallet first');
            return;
        }

        // Validate form
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

        if (!name) {
            alert('Please enter an NFT name');
            if (nameInput) nameInput.focus();
            return;
        }

        if (name.length > 128) {
            alert('NFT name must be 128 characters or fewer');
            if (nameInput) nameInput.focus();
            return;
        }

        if (description.length > 2048) {
            alert('Description must be 2048 characters or fewer');
            if (descInput) descInput.focus();
            return;
        }

        if (!uploadedFile) {
            alert('Please upload an image or media file');
            return;
        }

        if (supply < 1 || supply > 1000) {
            alert('Supply must be between 1 and 1000');
            return;
        }

        if (royalty < 0 || royalty > 50) {
            alert('Royalty must be between 0% and 50%');
            return;
        }

        var createBtn = document.getElementById('createBtn');
        if (createBtn) {
            createBtn.disabled = true;
            createBtn.innerHTML = '<i class="fas fa-spinner fa-spin"></i> Minting...';
        }

        try {
            // Build metadata
            var metadata = {
                name: name,
                description: description,
                image: uploadedDataUrl, // In production: upload to IPFS first
                properties: properties.filter(function (p) { return p.trait_type && p.value; }),
                creator: currentWallet.address,
                supply: supply,
                royalty: royalty,
            };

            // Build mint transaction
            var txData = {
                action: 'mint_nft',
                collection: collection,
                metadata: metadata,
                supply: supply,
                royalty_bps: Math.round(royalty * 100), // Convert % to basis points
            };

            // Simulate first
            var simResult = await rpcCall('simulateTransaction', [{
                from: currentWallet.address,
                to: collection || 'system',
                amount: 0,
                data: JSON.stringify(txData),
            }]);

            if (simResult && simResult.success === false) {
                throw new Error(simResult.error || 'Simulation failed');
            }

            // Send the actual transaction
            var sendResult = await rpcCall('sendTransaction', [{
                from: currentWallet.address,
                to: collection || 'system',
                amount: 0,
                data: JSON.stringify(txData),
            }]);

            var tokenId = (sendResult && sendResult.token_id) || (simResult && simResult.token_id) || 'pending';
            alert('NFT "' + name + '" minted successfully! Token ID: #' + tokenId);

            // Reset form
            if (nameInput) nameInput.value = '';
            if (descInput) descInput.value = '';
            if (supplyInput) supplyInput.value = '1';
            if (royaltyInput) royaltyInput.value = '0';
            properties = [];
            renderProperties();
            removeFile();

            // Reset live preview
            setText('previewName', 'Untitled');
            setText('previewCollection', 'No Collection');
            setText('previewSupply', '1');
            setText('previewRoyalty', '0%');

        } catch (err) {
            alert('Minting failed: ' + err.message);
        } finally {
            if (createBtn) {
                createBtn.disabled = false;
                createBtn.innerHTML = '<i class="fas fa-magic"></i> Create & Mint NFT';
            }
        }
    }

    // ===== Load Collections for Dropdown =====
    async function loadCollections() {
        var select = document.getElementById('nftCollection');
        if (!select) return;

        try {
            var collections = await rpcCall('getAllContracts', []);
            if (Array.isArray(collections) && collections.length > 0) {
                collections.forEach(function (col) {
                    var option = document.createElement('option');
                    option.value = col.id || col.program_id || '';
                    option.textContent = col.name || col.symbol || formatHash(col.id || '', 12);
                    select.appendChild(option);
                });
            }
        } catch (err) {
            console.warn('Failed to load collections:', err);
        }
    }

    // ===== Helpers =====
    function setText(id, value) {
        var el = document.getElementById(id);
        if (el) el.textContent = value;
    }

    // ===== Event Setup =====
    function setupEvents() {
        // Create button
        var createBtn = document.getElementById('createBtn');
        if (createBtn) {
            createBtn.addEventListener('click', function (e) {
                e.preventDefault();
                mintNFT();
            });
        }

        // Use shared wallet manager
        if (window.MoltWallet) {
            window.moltWallet = window.moltWallet || new MoltWallet({ rpcUrl: RPC_URL });
            window.moltWallet.bindConnectButton('#connectWallet');
            window.moltWallet.onConnect(function(info) {
                currentWallet = info;
                updateCreateBtnState();
            });
            window.moltWallet.onDisconnect(function() {
                currentWallet = null;
                updateCreateBtnState();
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

    function updateCreateBtnState() {
        var createBtn = document.getElementById('createBtn');
        if (!createBtn) return;
        if (!currentWallet) {
            createBtn.title = 'Connect wallet to mint';
        } else {
            createBtn.title = '';
        }
    }

    // ===== Public API (for onclick references in HTML) =====
    window.addProperty = function () {
        properties.push({ trait_type: '', value: '' });
        renderProperties();
    };

    window._createRemoveProperty = function (index) {
        properties.splice(index, 1);
        renderProperties();
    };

    window._createUpdateProperty = function (index, field, value) {
        if (properties[index]) properties[index][field] = value;
    };

    window._createRemoveFile = function () {
        removeFile();
    };

    window.removeProperty = function (index) {
        properties.splice(index, 1);
        renderProperties();
    };

    // ===== Init =====
    document.addEventListener('DOMContentLoaded', function () {
        console.log('🦞 Molt Market Create loading...');
        if (typeof initMarketNetworkSelector === 'function') initMarketNetworkSelector();
        setupEvents();
        setupUpload();
        setupLivePreview();
        loadCollections();
        renderProperties();
        updateLivePreviewImage(null, null);
        console.log('✅ Molt Market Create ready');
    });
})();
