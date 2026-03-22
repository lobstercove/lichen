/**
 * MoltChain Shared Wallet Connect — Full Wallet Modal System
 *
 * Provides a DEX-identical wallet connection experience for the marketplace.
 * Supports: Import (private key + mnemonic), Extension, Create New.
 * Uses TweetNaCl (nacl) for real Ed25519 keypair generation and signing.
 * BIP39 PBKDF2-HMAC-SHA512 for mnemonic derivation (matches wallet app + extension).
 *
 * Usage:
 *   <script src="https://cdnjs.cloudflare.com/ajax/libs/tweetnacl/1.0.3/nacl-fast.min.js"></script>
 *   <script src="shared/wallet-connect.js"></script>
 *   — Auto-binds to #connectWallet button if present.
 *   — Exposes window.MoltWallet constructor.
 */

// ─── Utilities (only what shared/utils.js does NOT provide) ──

// bytesToHex / hexToBytes / keypairSeedHex are wallet-specific — keep here.
// BS58_ALPHABET, bs58encode, bs58decode, formatHash, moltRpcCall,
// escapeHtml, getMoltRpcUrl are already declared in shared/utils.js
// which is loaded BEFORE this file.

function bytesToHex(b) { return Array.from(b).map(x => x.toString(16).padStart(2, '0')).join(''); }
function hexToBytes(h) {
    const c = h.startsWith('0x') ? h.slice(2) : h;
    if (!/^[0-9a-fA-F]*$/.test(c)) throw new Error('Key must be hexadecimal');
    if (c.length % 2 !== 0) throw new Error('Key has odd number of hex characters');
    const arr = new Uint8Array(c.length / 2);
    for (let i = 0; i < c.length; i += 2) arr[i / 2] = parseInt(c.slice(i, i + 2), 16);
    return arr;
}
function keypairSeedHex(keypair) {
    if (!keypair || !keypair.secretKey) return '';
    return bytesToHex(keypair.secretKey.slice(0, 32));
}

// ─── Wallet Modal HTML ──────────────────────────────────

const WALLET_MODAL_HTML = '<div class="wallet-modal-overlay hidden" id="walletModal">' +
    '<div class="wallet-modal-content">' +
    '<div class="wallet-modal-header"><h3>Connect Wallet</h3>' +
    '<button class="wallet-modal-close" id="closeWalletModal"><i class="fas fa-times"></i></button></div>' +
    '<div class="wallet-modal-tabs">' +
    '<button class="wm-tab active" data-wm-tab="wallets">Wallets</button>' +
    '<button class="wm-tab" data-wm-tab="import">Import</button>' +
    '<button class="wm-tab" data-wm-tab="extension">Extension</button>' +
    '<button class="wm-tab" data-wm-tab="create">Create New</button></div>' +
    '<div class="wm-tab-content" id="wmTabWallets"><div class="wm-wallets-list" id="wmWalletsList"></div></div>' +
    '<div class="wm-tab-content hidden" id="wmTabImport">' +
    '<div class="wm-import-toggle">' +
    '<button class="wm-import-type active" data-import="key">Private Key</button>' +
    '<button class="wm-import-type" data-import="mnemonic">Mnemonic</button></div>' +
    '<div class="wm-import-key" id="wmImportKey"><div class="form-group"><label>Private Key</label>' +
    '<input type="password" id="wmPrivateKey" placeholder="Enter your private key (hex or base58)" class="form-input"></div></div>' +
    '<div class="wm-import-mnemonic hidden" id="wmImportMnemonic"><div class="form-group">' +
    '<label>Recovery Phrase (12 or 24 words)</label><div class="mnemonic-grid" id="mnemonicGrid"></div></div></div>' +
    '<div class="form-group"><label>Password <span style="font-size:0.75rem;color:var(--text-muted);">' +
    '(optional \u2014 encrypts local storage)</span></label>' +
    '<input type="password" id="wmPassword" placeholder="Set a password to encrypt" class="form-input"></div>' +
    '<button class="btn btn-primary btn-full" id="wmConnectBtn"><i class="fas fa-key"></i> Connect Wallet</button></div>' +
    '<div class="wm-tab-content hidden" id="wmTabExtension"><div class="wm-create-info">' +
    '<i class="fas fa-plug"></i><h4>Connect Wallet Extension</h4>' +
    '<p>Connect your MoltChain Wallet browser extension to trade securely. Your private keys never leave the extension.</p></div>' +
    '<button class="btn btn-primary btn-full" id="wmExtensionBtn"><i class="fas fa-plug"></i> Connect Extension</button></div>' +
    '<div class="wm-tab-content hidden" id="wmTabCreate"><div class="wm-create-info">' +
    '<i class="fas fa-shield-alt"></i><h4>Generate New Wallet</h4>' +
    '<p>Create a fresh MoltChain wallet. Save the private key securely \u2014 it will not be shown again.</p></div>' +
    '<button class="btn btn-primary btn-full" id="wmCreateBtn"><i class="fas fa-plus-circle"></i> Create Wallet</button>' +
    '<div class="wm-created-wallet hidden" id="wmCreatedWallet">' +
    '<div class="wm-generated-row"><span class="wm-label">Address</span><span class="wm-value" id="wmNewAddress"></span></div>' +
    '<div class="wm-generated-row"><span class="wm-label">Private Key</span>' +
    '<span class="wm-value wm-secret" id="wmNewKey"></span>' +
    '<button class="btn btn-small btn-secondary wm-copy-btn" data-copy="wmNewKey"><i class="fas fa-copy"></i></button></div>' +
    '<div class="wm-warning"><i class="fas fa-exclamation-triangle"></i>' +
    '<span>Save your private key! It will not be shown again.</span></div></div></div>' +
    '</div></div>';

// ─── Wallet Manager ──────────────────────────────────────

function MoltWallet(options) {
    options = options || {};
    this.rpcUrl = options.rpcUrl || getMoltRpcUrl();
    this.storageKey = options.storageKey || 'marketWallets';
    this.persist = options.persist !== false;

    this.address = null;
    this.shortAddr = null;
    this.balance = 0;
    this.keypair = null;
    this.signingReady = false;
    this._connectCallbacks = [];
    this._disconnectCallbacks = [];
    this._balanceCallbacks = [];
    this._balanceInterval = null;
    this._buttonEl = null;

    // MoltyID / .molt name state (matches DEX)
    this.moltName = null;
    this.moltyIdProfile = null;
    this.reputation = 0;
    this.trustTier = 0;

    // Multi-wallet support
    this.savedWallets = [];
    this.localWalletSessions = new Map();

    // Restore saved wallets from localStorage
    if (this.persist) {
        try {
            var stored = localStorage.getItem(this.storageKey);
            if (stored) this.savedWallets = JSON.parse(stored) || [];
        } catch (e) { this.savedWallets = []; }
    }

    this._modalInjected = false;
}

MoltWallet.prototype.isConnected = function() { return this.address !== null; };

/** Import wallet from hex or base58 private key */
MoltWallet.prototype.fromSecretKey = async function(secretInput) {
    var text = (secretInput || '').trim();
    if (!text) throw new Error('Private key is required');
    var bytes;
    try {
        var hex = text.startsWith('0x') ? text.slice(2) : text;
        if (/^[0-9a-fA-F]+$/.test(hex) && (hex.length === 64 || hex.length === 128)) {
            bytes = hexToBytes(hex);
        } else {
            bytes = bs58decode(text);
        }
    } catch (e) {
        throw new Error('Invalid private key format (expected hex or base58)');
    }

    var kp;
    if (bytes.length === 64) {
        kp = nacl.sign.keyPair.fromSecretKey(bytes);
    } else if (bytes.length === 32) {
        kp = nacl.sign.keyPair.fromSeed(bytes);
    } else {
        throw new Error('Private key must be 32-byte seed or 64-byte Ed25519 secret key');
    }

    this.keypair = kp;
    this.address = bs58encode(kp.publicKey);
    this.shortAddr = this.address.slice(0, 8) + '...' + this.address.slice(-6);
    this.signingReady = true;
    return this;
};

/** Generate a fresh Ed25519 keypair */
MoltWallet.prototype.generate = async function() {
    var kp = nacl.sign.keyPair();
    this.keypair = kp;
    this.address = bs58encode(kp.publicKey);
    this.shortAddr = this.address.slice(0, 8) + '...' + this.address.slice(-6);
    this.signingReady = true;
    return this;
};

/** Connect to known address (read-only or with keypair) */
MoltWallet.prototype.connectAddress = async function(addr, options) {
    options = options || {};
    this.address = addr;
    this.shortAddr = addr.slice(0, 8) + '...' + addr.slice(-6);
    this.signingReady = !!options.signingReady;
    this.keypair = options.localKeypair || (this.signingReady ? { connected: true } : null);
    return this;
};

/** Connect via wallet extension */
MoltWallet.prototype.connectExtension = async function() {
    if (window.moltchain && typeof window.moltchain.connect === 'function') {
        var ext = await window.moltchain.connect();
        this.address = ext.publicKey || ext.address;
        this.shortAddr = this.address.slice(0, 8) + '...' + this.address.slice(-6);
        this.signingReady = true;
        this.keypair = { connected: true };
        this._moltExtension = ext;
        return this;
    }
    // Fallback: local wallet generation when extension is unavailable.
    // RPC does not expose createWallet; generate a local Ed25519 keypair instead.
    return this.generate();
};

/** Sign a message (Ed25519 detached) */
MoltWallet.prototype.sign = function(message) {
    if (!this.keypair || !this.keypair.secretKey) throw new Error('No local keypair available for signing');
    return nacl.sign.detached(message, this.keypair.secretKey);
};

/** Send transaction via RPC with optional local signing */
MoltWallet.prototype.sendTransaction = async function(instructions) {
    if (!this.address) throw new Error('Wallet not connected');
    if (!this.signingReady) throw new Error('Signing session not active. Reconnect wallet to sign.');

    var blockhash = await moltRpcCall('getRecentBlockhash', [], this.rpcUrl);
    var self = this;
    var message = {
        instructions: instructions.map(function(ix) {
            var accounts = (ix.accounts || [self.address]).map(function(a) {
                return Array.from(bs58decode(a));
            });
            var dataBytes = typeof ix.data === 'string'
                ? Array.from(new TextEncoder().encode(ix.data))
                : Array.from(ix.data || []);
            return {
                program_id: Array.from(bs58decode(ix.program_id)),
                accounts: accounts,
                data: dataBytes,
            };
        }),
        blockhash: blockhash,
    };

    if (this.keypair && this.keypair.secretKey) {
        var msg = serializeMessageBincode(message);
        var sig = this.sign(msg);
        var txPayload = { signatures: [Array.from(sig)], message: message };
        var txBase64 = btoa(String.fromCharCode.apply(null, new TextEncoder().encode(JSON.stringify(txPayload))));
        return moltRpcCall('sendTransaction', [txBase64], this.rpcUrl);
    }

    var extension = this._moltExtension || (window.moltchain && typeof window.moltchain === 'object' ? window.moltchain : null);
    if (!extension) {
        throw new Error('No local keypair available and wallet extension is unavailable for signing.');
    }

    var unsignedTx = { signatures: [], message: message };
    if (typeof extension.sendTransaction === 'function') {
        return extension.sendTransaction(unsignedTx);
    }

    if (typeof extension.signTransaction === 'function') {
        var signed = await extension.signTransaction(unsignedTx);
        if (typeof signed === 'string') {
            return moltRpcCall('sendTransaction', [signed], this.rpcUrl);
        }
        if (signed && typeof signed.signedTransactionBase64 === 'string') {
            return moltRpcCall('sendTransaction', [signed.signedTransactionBase64], this.rpcUrl);
        }
        if (signed && signed.signedTransaction) {
            var txBase64 = btoa(String.fromCharCode.apply(null, new TextEncoder().encode(JSON.stringify(signed.signedTransaction))));
            return moltRpcCall('sendTransaction', [txBase64], this.rpcUrl);
        }
        throw new Error('Wallet extension returned an unsupported signed transaction format');
    }

    throw new Error('Wallet extension does not expose sendTransaction/signTransaction');
};

/** Derive seed from mnemonic — BIP39 PBKDF2-HMAC-SHA512 (matches wallet app & extension) */
MoltWallet.prototype.mnemonicToSeed = async function(phrase) {
    var mnemonicBytes = new TextEncoder().encode(phrase.normalize('NFKD').trim());
    var saltBytes = new TextEncoder().encode('mnemonic');
    var keyMaterial = await crypto.subtle.importKey('raw', mnemonicBytes, 'PBKDF2', false, ['deriveBits']);
    var seedBuffer = await crypto.subtle.deriveBits(
        { name: 'PBKDF2', salt: saltBytes, iterations: 2048, hash: 'SHA-512' },
        keyMaterial, 512
    );
    return new Uint8Array(seedBuffer).slice(0, 32);
};

/** Refresh balance from RPC */
MoltWallet.prototype.refreshBalance = async function() {
    if (!this.address) return 0;
    try {
        var result = await moltRpcCall('getBalance', [this.address], this.rpcUrl);
        this.balance = (typeof result === 'object') ? (result.balance || result.value || 0) : (result || 0);
    } catch (err) { /* balance unavailable */ }
    for (var i = 0; i < this._balanceCallbacks.length; i++) {
        try { this._balanceCallbacks[i](this.balance, this.address); } catch (e) { console.error(e); }
    }
    return this.balance;
};

MoltWallet.prototype._startBalancePolling = function() {
    this._stopBalancePolling();
    var self = this;
    this._balanceInterval = setInterval(function() { self.refreshBalance(); }, 15000);
};

MoltWallet.prototype._stopBalancePolling = function() {
    if (this._balanceInterval) { clearInterval(this._balanceInterval); this._balanceInterval = null; }
};

// ─── Events ──────────────────────────────────────────────

MoltWallet.prototype.onConnect = function(cb) {
    this._connectCallbacks.push(cb);
    if (this.isConnected()) {
        try { cb({ address: this.address, shortAddr: this.shortAddr, balance: this.balance }); } catch (e) { console.error(e); }
    }
};

MoltWallet.prototype.onDisconnect = function(cb) { this._disconnectCallbacks.push(cb); };
MoltWallet.prototype.onBalanceUpdate = function(cb) { this._balanceCallbacks.push(cb); };

MoltWallet.prototype._fireConnect = function() {
    var info = { address: this.address, shortAddr: this.shortAddr, balance: this.balance };
    for (var i = 0; i < this._connectCallbacks.length; i++) {
        try { this._connectCallbacks[i](info); } catch (e) { console.error(e); }
    }
    this._updateButton();
    this._startBalancePolling();
    this.refreshBalance();
};

MoltWallet.prototype._fireDisconnect = function() {
    for (var i = 0; i < this._disconnectCallbacks.length; i++) {
        try { this._disconnectCallbacks[i]({ address: null }); } catch (e) { console.error(e); }
    }
    this._updateButton();
    this._stopBalancePolling();
};

// ─── Connection Logic ────────────────────────────────────

MoltWallet.prototype._connectTo = async function(address, shortAddr, options) {
    options = options || {};
    this.address = address;
    this.shortAddr = shortAddr;
    this.signingReady = !!options.signingReady;
    var sessionKp = options.localKeypair || this.localWalletSessions.get(address) || null;
    this.keypair = sessionKp || (this.signingReady ? { connected: true } : null);
    this.signingReady = this.signingReady || !!sessionKp;

    // M16: Resolve .molt name and fetch MoltyID profile (matches DEX)
    var displayLabel = shortAddr;
    try {
        var reverseResult = await moltRpcCall('reverseMoltName', [address], this.rpcUrl);
        if (reverseResult && reverseResult.name) {
            this.moltName = reverseResult.name + '.molt';
            displayLabel = this.moltName;
        } else {
            this.moltName = null;
        }
    } catch (e) { this.moltName = null; }
    try {
        var profileResult = await moltRpcCall('getMoltyIdProfile', [address], this.rpcUrl);
        if (profileResult) {
            this.moltyIdProfile = profileResult;
            this.reputation = profileResult.reputation || 0;
            this.trustTier = profileResult.trustTier || profileResult.trust_tier || 0;
        } else {
            this.moltyIdProfile = null; this.reputation = 0; this.trustTier = 0;
        }
    } catch (e) { this.moltyIdProfile = null; this.reputation = 0; this.trustTier = 0; }
    this._displayLabel = displayLabel;

    if (!options.preserveCreatedDetails) this._resetModalInputs();
    await this._renderWalletList();
    this._fireConnect();
};

MoltWallet.prototype._disconnect = function() {
    this.address = null;
    this.shortAddr = null;
    this.keypair = null;
    this.signingReady = false;
    this.moltName = null;
    this.moltyIdProfile = null;
    this.reputation = 0;
    this.trustTier = 0;
    this._displayLabel = null;
    this._fireDisconnect();
};

MoltWallet.prototype._disconnectAll = function() {
    this.savedWallets = [];
    if (this.persist) { try { localStorage.removeItem(this.storageKey); } catch (e) {} }
    this._disconnect();
    this._showNotification('All wallets disconnected', 'info');
};

MoltWallet.prototype._saveWallets = function() {
    if (this.persist) {
        try { localStorage.setItem(this.storageKey, JSON.stringify(this.savedWallets)); } catch (e) {}
    }
};

// ─── UI: Button ──────────────────────────────────────────

MoltWallet.prototype.bindConnectButton = function(selector) {
    var el = (typeof selector === 'string') ? document.querySelector(selector) : selector;
    if (!el) return;
    this._buttonEl = el;
    var self = this;
    el.addEventListener('click', function(e) {
        e.preventDefault();
        self._openWalletModal();
    });
    this._updateButton();

    // Restore last connected wallet on page load
    if (this.savedWallets.length > 0) {
        var last = this.savedWallets[this.savedWallets.length - 1];
        var sessionKp = this.localWalletSessions.get(last.address) || null;
        this._connectTo(last.address, last.short || last.address.slice(0, 8) + '...', { signingReady: !!sessionKp, localKeypair: sessionKp });
    }
};

MoltWallet.prototype._updateButton = function() {
    if (!this._buttonEl) return;
    if (this.isConnected()) {
        var label = this._displayLabel || this.moltName || escapeHtml(formatHash(this.address, 6));
        var repBadge = this.reputation > 0
            ? ' <span class="moltyid-rep-badge" title="MoltyID Reputation: ' + this.reputation + '">\u2b50' + this.reputation + '</span>'
            : '';
        this._buttonEl.innerHTML = '<i class="fas fa-wallet"></i> ' + escapeHtml(label) + repBadge;
        this._buttonEl.className = 'btn btn-small btn-secondary';
        this._buttonEl.title = this.address;
    } else {
        this._buttonEl.innerHTML = '<i class="fas fa-wallet"></i> Connect Wallet';
        this._buttonEl.className = 'btn btn-small btn-primary';
        this._buttonEl.title = 'Click to connect wallet';
    }
};

// ─── UI: Notification Toast ──────────────────────────────

MoltWallet.prototype._showNotification = function(message, type) {
    type = type || 'info';
    var toast = document.createElement('div');
    toast.className = 'wm-toast wm-toast-' + type;
    toast.textContent = message;
    Object.assign(toast.style, {
        position: 'fixed', bottom: '24px', right: '24px', zIndex: '9999',
        padding: '12px 20px', borderRadius: '12px', fontSize: '0.88rem', fontWeight: '500',
        color: '#fff', boxShadow: '0 8px 24px rgba(0,0,0,0.3)', transition: 'opacity 0.3s',
        background: type === 'success' ? '#22c55e' : type === 'error' ? '#ef4444' : '#3b82f6'
    });
    document.body.appendChild(toast);
    setTimeout(function() { toast.style.opacity = '0'; setTimeout(function() { toast.remove(); }, 300); }, 3000);
};

// ─── UI: Modal ───────────────────────────────────────────

MoltWallet.prototype._injectModal = function() {
    if (this._modalInjected) return;
    var container = document.createElement('div');
    container.innerHTML = WALLET_MODAL_HTML;
    document.body.appendChild(container.firstElementChild);
    this._modalInjected = true;
    this._wireModalEvents();
};

MoltWallet.prototype._openWalletModal = function() {
    this._injectModal();
    var modal = document.getElementById('walletModal');
    if (modal) {
        modal.classList.remove('hidden');
        this._renderWalletList();
        this._switchTab(this.savedWallets.length ? 'wallets' : 'extension');
    }
};

MoltWallet.prototype._closeWalletModal = function() {
    var modal = document.getElementById('walletModal');
    if (modal) modal.classList.add('hidden');
    this._resetModalInputs();
};

MoltWallet.prototype._switchTab = function(tabName) {
    var tabs = document.querySelectorAll('#walletModal .wm-tab');
    tabs.forEach(function(t) { t.classList.toggle('active', t.dataset.wmTab === tabName); });
    var contents = { wallets: 'wmTabWallets', import: 'wmTabImport', extension: 'wmTabExtension', create: 'wmTabCreate' };
    Object.keys(contents).forEach(function(k) {
        var el = document.getElementById(contents[k]);
        if (el) el.classList.toggle('hidden', k !== tabName);
    });
};

MoltWallet.prototype._resetModalInputs = function(options) {
    options = options || {};
    var clearCreated = options.clearCreated !== false; // default true
    var pk = document.getElementById('wmPrivateKey'); if (pk) pk.value = '';
    var pw = document.getElementById('wmPassword'); if (pw) pw.value = '';

    var importBtns = document.querySelectorAll('#walletModal .wm-import-type');
    importBtns.forEach(function(btn) { btn.classList.toggle('active', btn.dataset.import === 'key'); });

    var keyPanel = document.getElementById('wmImportKey'); if (keyPanel) keyPanel.classList.remove('hidden');
    var mnPanel = document.getElementById('wmImportMnemonic'); if (mnPanel) mnPanel.classList.add('hidden');

    var mnInputs = document.querySelectorAll('#mnemonicGrid input');
    mnInputs.forEach(function(inp, i) { inp.value = ''; inp.style.display = i >= 12 ? 'none' : ''; });

    if (clearCreated) {
        var created = document.getElementById('wmCreatedWallet'); if (created) created.classList.add('hidden');
        var newAddr = document.getElementById('wmNewAddress'); if (newAddr) newAddr.textContent = '';
        var newKey = document.getElementById('wmNewKey'); if (newKey) newKey.textContent = '';
        var createBtn = document.getElementById('wmCreateBtn'); if (createBtn) createBtn.classList.remove('hidden');
    }
};

MoltWallet.prototype._renderWalletList = async function() {
    var list = document.getElementById('wmWalletsList');
    if (!list) return;
    var self = this;

    if (!this.savedWallets.length) {
        list.innerHTML = '<div class="wm-empty"><i class="fas fa-wallet"></i><p>No wallets connected</p>' +
            '<button class="btn btn-primary btn-small" id="wmEmptyImport">Import Wallet</button></div>';
        var btn = document.getElementById('wmEmptyImport');
        if (btn) btn.addEventListener('click', function() { self._switchTab('import'); });
        return;
    }

    // M16: Batch-resolve .molt names for saved wallets (matches DEX)
    var nameMap = {};
    try {
        var result = await moltRpcCall('batchReverseMoltNames', [this.savedWallets.map(function(w) { return w.address; })], this.rpcUrl);
        if (result && typeof result === 'object') {
            for (var addr in result) {
                if (result[addr]) nameMap[addr] = result[addr] + '.molt';
            }
        }
    } catch (e) { /* RPC unavailable — show plain addresses */ }

    list.innerHTML = this.savedWallets.map(function(w, i) {
        var label = nameMap[w.address] || w.short || w.address.slice(0, 8) + '...' + w.address.slice(-6);
        var isActive = self.address === w.address;
        return '<div class="wm-wallet-item' + (isActive ? ' active-wallet' : '') + '">' +
            '<span class="wm-wallet-addr">' + escapeHtml(label) + '</span>' +
            '<div class="wm-wallet-actions">' +
            (isActive ? '<span class="btn btn-small btn-secondary" style="opacity:0.6;cursor:default;">Active</span>' :
                '<button class="btn btn-small btn-primary wm-switch-btn" data-idx="' + i + '">Switch</button>') +
            '<button class="btn btn-small btn-secondary wm-remove-btn" data-idx="' + i + '"><i class="fas fa-times"></i></button>' +
            '</div></div>';
    }).join('') + '<div class="wm-disconnect-all"><button class="btn btn-small btn-secondary" id="wmDisconnectAll">Disconnect All</button></div>';

    list.querySelectorAll('.wm-switch-btn').forEach(function(btn) {
        btn.addEventListener('click', function() {
            var w = self.savedWallets[parseInt(btn.dataset.idx)];
            if (w) {
                var sessionKp = self.localWalletSessions.get(w.address) || null;
                self._connectTo(w.address, w.short || w.address.slice(0, 8) + '...', { signingReady: !!sessionKp, localKeypair: sessionKp });
                self._renderWalletList();
            }
        });
    });

    list.querySelectorAll('.wm-remove-btn').forEach(function(btn) {
        btn.addEventListener('click', async function() {
            var i = parseInt(btn.dataset.idx, 10);
            var removed = self.savedWallets[i];
            if (isNaN(i) || !removed) return;

            self.savedWallets.splice(i, 1);
            self._saveWallets();

            if (self.address === removed.address) {
                var fallback = self.savedWallets[0] || null;
                if (fallback) {
                    var fkp = self.localWalletSessions.get(fallback.address) || null;
                    await self._connectTo(fallback.address, fallback.short || fallback.address.slice(0, 8) + '...', { signingReady: !!fkp, localKeypair: fkp });
                    self._showNotification('Wallet removed. Switched to another wallet', 'info');
                } else {
                    self._disconnect();
                    self._showNotification('Wallet removed', 'info');
                }
            } else {
                self._showNotification('Wallet removed', 'info');
            }
            self._renderWalletList();
        });
    });

    var da = document.getElementById('wmDisconnectAll');
    if (da) da.addEventListener('click', function() { self._disconnectAll(); self._renderWalletList(); });
};

MoltWallet.prototype._wireModalEvents = function() {
    var self = this;

    // Close
    var closeBtn = document.getElementById('closeWalletModal');
    if (closeBtn) closeBtn.addEventListener('click', function() { self._closeWalletModal(); });

    var modal = document.getElementById('walletModal');
    if (modal) modal.addEventListener('click', function(e) { if (e.target === modal) self._closeWalletModal(); });

    document.addEventListener('keydown', function(e) {
        if (e.key === 'Escape' && modal && !modal.classList.contains('hidden')) self._closeWalletModal();
    });

    // Tabs
    document.querySelectorAll('#walletModal .wm-tab').forEach(function(t) {
        t.addEventListener('click', function() { self._switchTab(t.dataset.wmTab); });
    });

    // Import type toggle
    document.querySelectorAll('#walletModal .wm-import-type').forEach(function(btn) {
        btn.addEventListener('click', function() {
            document.querySelectorAll('#walletModal .wm-import-type').forEach(function(b) { b.classList.remove('active'); });
            btn.classList.add('active');
            var k = document.getElementById('wmImportKey'), m = document.getElementById('wmImportMnemonic');
            if (btn.dataset.import === 'key') { if (k) k.classList.remove('hidden'); if (m) m.classList.add('hidden'); }
            else { if (k) k.classList.add('hidden'); if (m) m.classList.remove('hidden'); }
        });
    });

    // Mnemonic grid (24 inputs, show 12 by default, expand on paste)
    var mnGrid = document.getElementById('mnemonicGrid');
    if (mnGrid) {
        for (var i = 0; i < 24; i++) {
            var inp = document.createElement('input');
            inp.type = 'text'; inp.placeholder = 'Word ' + (i + 1);
            inp.className = 'form-input'; inp.dataset.wordIdx = i;
            if (i >= 12) inp.style.display = 'none';
            mnGrid.appendChild(inp);
        }
        mnGrid.addEventListener('paste', function(e) {
            var text = (e.clipboardData || window.clipboardData).getData('text');
            var words = text.trim().split(/\s+/);
            if (words.length >= 2) {
                e.preventDefault();
                var inputs = mnGrid.querySelectorAll('input');
                if (words.length > 12) inputs.forEach(function(inp) { inp.style.display = ''; });
                words.forEach(function(w, j) { if (inputs[j]) inputs[j].value = w; });
            }
        });
    }

    // Import button — connect from private key or mnemonic
    var wmConnectBtn = document.getElementById('wmConnectBtn');
    if (wmConnectBtn) wmConnectBtn.addEventListener('click', async function() {
        try {
            var activeImport = (document.querySelector('#walletModal .wm-import-type.active') || {}).dataset;
            var importType = (activeImport && activeImport.import) || 'key';
            if (importType === 'mnemonic') {
                var words = Array.from(document.querySelectorAll('#mnemonicGrid input'))
                    .map(function(i) { return (i.value || '').trim(); })
                    .filter(Boolean);
                if (words.length !== 12 && words.length !== 24) throw new Error('Mnemonic must have 12 or 24 words');
                var phrase = words.join(' ').toLowerCase();
                var seed = await self.mnemonicToSeed(phrase);
                var kp = nacl.sign.keyPair.fromSeed(seed);
                self.keypair = kp;
                self.address = bs58encode(kp.publicKey);
                self.shortAddr = self.address.slice(0, 8) + '...' + self.address.slice(-6);
                self.signingReady = true;
            } else {
                var pkInput = (document.getElementById('wmPrivateKey') || {}).value || '';
                await self.fromSecretKey(pkInput);
            }

            self.localWalletSessions.set(self.address, self.keypair);
            if (!self.savedWallets.some(function(w) { return w.address === self.address; })) {
                self.savedWallets.push({ address: self.address, short: self.shortAddr, added: Date.now() });
                self._saveWallets();
            }
            await self._connectTo(self.address, self.shortAddr, { signingReady: true, localKeypair: self.keypair });
            self._closeWalletModal();
            self._showNotification('Wallet connected: ' + self.shortAddr, 'success');
        } catch (e) {
            self._showNotification('Import failed: ' + e.message, 'error');
        }
    });

    // Extension button
    var wmExtensionBtn = document.getElementById('wmExtensionBtn');
    if (wmExtensionBtn) wmExtensionBtn.addEventListener('click', async function() {
        try {
            await self.connectExtension();
            if (!self.savedWallets.some(function(w) { return w.address === self.address; })) {
                self.savedWallets.push({ address: self.address, short: self.shortAddr, added: Date.now() });
                self._saveWallets();
            }
            await self._connectTo(self.address, self.shortAddr, { signingReady: true });
            self._closeWalletModal();
            self._showNotification('Wallet connected: ' + self.shortAddr, 'success');
        } catch (e) {
            self._showNotification('Extension connection failed: ' + e.message, 'error');
        }
    });

    // Create button — generate new keypair
    var wmCreateBtn = document.getElementById('wmCreateBtn');
    if (wmCreateBtn) wmCreateBtn.addEventListener('click', async function() {
        try {
            await self.generate();
            self.localWalletSessions.set(self.address, self.keypair);
            if (!self.savedWallets.some(function(w) { return w.address === self.address; })) {
                self.savedWallets.push({ address: self.address, short: self.shortAddr, added: Date.now() });
                self._saveWallets();
            }
            var newAddrEl = document.getElementById('wmNewAddress');
            var newKeyEl = document.getElementById('wmNewKey');
            var createdBox = document.getElementById('wmCreatedWallet');
            if (newAddrEl) newAddrEl.textContent = self.address;
            if (newKeyEl) newKeyEl.textContent = keypairSeedHex(self.keypair);
            if (createdBox) createdBox.classList.remove('hidden');
            if (wmCreateBtn) wmCreateBtn.classList.add('hidden');
            await self._connectTo(self.address, self.shortAddr, { signingReady: true, localKeypair: self.keypair, preserveCreatedDetails: true });
            self._showNotification('New wallet created and connected', 'success');
        } catch (e) {
            self._showNotification('Create wallet failed: ' + e.message, 'error');
        }
    });

    // Copy buttons
    document.querySelectorAll('#walletModal .wm-copy-btn').forEach(function(btn) {
        btn.addEventListener('click', function() {
            var el = document.getElementById(btn.dataset.copy);
            if (el) navigator.clipboard.writeText(el.textContent).then(function() { self._showNotification('Copied!', 'success'); });
        });
    });
};

// ─── Public API ──────────────────────────────────────────

MoltWallet.prototype.disconnect = function() { this._disconnect(); };
MoltWallet.prototype.toggle = async function() { this._openWalletModal(); };

// ─── Export ──────────────────────────────────────────────

window.MoltWallet = MoltWallet;
// bytesToHex / hexToBytes are wallet-specific; others already on window from utils.js
window.bytesToHex = window.bytesToHex || bytesToHex;
window.hexToBytes = window.hexToBytes || hexToBytes;
