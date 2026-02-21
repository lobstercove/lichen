// ============================================================================
// MoltChain Shared Utilities
// Single source of truth for common JS helpers used across all frontends.
// Import via <script src="../shared/utils.js"></script> BEFORE app scripts.
// ============================================================================

// ── HTML Escaping ──

function escapeHtml(str) {
    return String(str ?? '')
        .replace(/&/g, '&amp;')
        .replace(/</g, '&lt;')
        .replace(/>/g, '&gt;')
        .replace(/"/g, '&quot;')
        .replace(/'/g, '&#39;');
}

// ── Formatting ──

function formatNumber(num) {
    if (num === null || num === undefined || Number.isNaN(num)) return '0';
    return Number(num).toLocaleString();
}

function formatHash(hash, length) {
    length = length || 6;
    if (!hash) return 'N/A';
    if (hash.length <= length * 2 + 3) return hash;
    return hash.substring(0, length) + '...' + hash.substring(hash.length - length);
}

function formatAddress(addr) {
    if (!addr) return 'N/A';
    return formatHash(addr, 6);
}

function formatMolt(shells) {
    const molt = shells / 1_000_000_000;
    return molt.toLocaleString(undefined, {
        minimumFractionDigits: 2,
        maximumFractionDigits: 4,
    }) + ' MOLT';
}

function formatMoltShells(shells) {
    return formatMolt(shells);
}

function formatTime(timestamp) {
    if (!timestamp || timestamp <= 0) return 'Genesis';
    const ts = timestamp < 1e12 ? timestamp : timestamp / 1000;
    const now = Date.now() / 1000;
    const diff = now - ts;
    if (diff < 0) return 'just now';
    if (diff < 60) return Math.floor(diff) + 's ago';
    if (diff < 3600) return Math.floor(diff / 60) + 'm ago';
    if (diff < 86400) return Math.floor(diff / 3600) + 'h ago';
    return Math.floor(diff / 86400) + 'd ago';
}

function timeAgo(timestamp) {
    return formatTime(timestamp);
}

function formatBytes(bytes) {
    if (bytes === 0) return '0 Bytes';
    const k = 1024;
    const sizes = ['Bytes', 'KB', 'MB', 'GB'];
    const i = Math.floor(Math.log(bytes) / Math.log(k));
    return Math.round(bytes / Math.pow(k, i) * 100) / 100 + ' ' + sizes[i];
}

function formatSlot(slot) {
    if (slot === null || slot === undefined) return 'N/A';
    return slot.toLocaleString();
}

// ── Clipboard ──

function copyToClipboard(text) {
    navigator.clipboard.writeText(text).then(function() {
        showToast('Copied to clipboard!');
    }).catch(function(err) {
        console.error('Failed to copy:', err);
    });
}

function safeCopy(el) {
    var text = el && el.dataset && el.dataset.copy;
    if (text) copyToClipboard(text);
}

function showToast(message) {
    var toast = document.createElement('div');
    toast.className = 'toast';
    toast.textContent = message;
    toast.style.cssText =
        'position:fixed;bottom:2rem;right:2rem;' +
        'background:var(--success,#22c55e);color:white;' +
        'padding:1rem 1.5rem;border-radius:8px;font-weight:600;' +
        'box-shadow:0 4px 16px rgba(0,0,0,0.3);z-index:10000;' +
        'animation:slideIn 0.3s ease;';
    document.body.appendChild(toast);
    setTimeout(function() { toast.remove(); }, 3000);
}

// Toast animation CSS (injected once)
if (typeof document !== 'undefined' && !document.getElementById('_shared_toast_css')) {
    var s = document.createElement('style');
    s.id = '_shared_toast_css';
    s.textContent = '@keyframes slideIn { from { transform: translateX(400px); opacity: 0; } to { transform: translateX(0); opacity: 1; } }';
    document.head.appendChild(s);
}

// ── Base58 ──

var BS58_ALPHABET = '123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz';

function bs58encode(bytes) {
    var leadingZeros = 0;
    for (var i = 0; i < bytes.length && bytes[i] === 0; i++) leadingZeros++;
    var num = 0n;
    for (var j = 0; j < bytes.length; j++) num = num * 256n + BigInt(bytes[j]);
    var encoded = '';
    while (num > 0n) { encoded = BS58_ALPHABET[Number(num % 58n)] + encoded; num = num / 58n; }
    return '1'.repeat(leadingZeros) + encoded;
}

function bs58decode(str) {
    var num = 0n;
    for (var i = 0; i < str.length; i++) {
        var idx = BS58_ALPHABET.indexOf(str[i]);
        if (idx < 0) throw new Error('Invalid base58 character');
        num = num * 58n + BigInt(idx);
    }
    var hex = num === 0n ? '' : num.toString(16);
    if (hex.length % 2) hex = '0' + hex;
    var bytes = [];
    for (var j = 0; j < hex.length; j += 2) bytes.push(parseInt(hex.slice(j, j + 2), 16));
    var leadingOnes = 0;
    for (var k = 0; k < str.length && str[k] === '1'; k++) leadingOnes++;
    var result = new Uint8Array(leadingOnes + bytes.length);
    result.set(bytes, leadingOnes);
    return result;
}

// Aliases
var base58Encode = bs58encode;
var base58Decode = bs58decode;

// ── RPC Client ──

function getMoltRpcUrl() {
    if (typeof window !== 'undefined') {
        if (window.moltConfig && window.moltConfig.rpcUrl) return window.moltConfig.rpcUrl;
        if (window.moltMarketConfig && window.moltMarketConfig.rpcUrl) return window.moltMarketConfig.rpcUrl;
        if (window.moltExplorerConfig && window.moltExplorerConfig.rpcUrl) return window.moltExplorerConfig.rpcUrl;
    }
    return 'http://localhost:9000';
}

async function moltRpcCall(method, params, rpcUrl) {
    var url = rpcUrl || getMoltRpcUrl();
    var response = await fetch(url, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
            jsonrpc: '2.0',
            id: Date.now(),
            method: method,
            params: params || []
        })
    });
    var data = await response.json();
    if (data.error) {
        throw new Error(data.error.message || 'RPC error');
    }
    return data.result;
}

// Legacy alias used by some files
var rpcCall = moltRpcCall;

// ── Binary Helpers ──

// J-4: Use BigInt for precision above Number.MAX_SAFE_INTEGER
function readLeU64(bytes) {
    if (!bytes || bytes.length < 8) return null;
    var lo = BigInt(bytes[0]) | (BigInt(bytes[1]) << 8n) | (BigInt(bytes[2]) << 16n) | (BigInt(bytes[3]) << 24n);
    var hi = BigInt(bytes[4]) | (BigInt(bytes[5]) << 8n) | (BigInt(bytes[6]) << 16n) | (BigInt(bytes[7]) << 24n);
    var value = lo | (hi << 32n);
    // Return Number when safe, BigInt for large values
    if (value <= BigInt(Number.MAX_SAFE_INTEGER)) return Number(value);
    return value;
}

// ── Bincode Message Serializer ──
// Produces the same bytes as Rust's bincode::serialize(&Message) so signatures match.

function serializeMessageBincode(message) {
    var parts = [];
    function writeU64LE(n) {
        var buf = new ArrayBuffer(8);
        var view = new DataView(buf);
        view.setBigUint64(0, BigInt(n), true);
        parts.push(new Uint8Array(buf));
    }
    function writeBytes(bytes) { parts.push(new Uint8Array(bytes)); }

    var ixs = message.instructions || [];
    writeU64LE(ixs.length);
    for (var i = 0; i < ixs.length; i++) {
        var ix = ixs[i];
        writeBytes(ix.program_id);
        var accounts = ix.accounts || [];
        writeU64LE(accounts.length);
        for (var j = 0; j < accounts.length; j++) writeBytes(accounts[j]);
        var data = ix.data || [];
        writeU64LE(data.length);
        writeBytes(data);
    }

    var hashHex = message.blockhash || message.recent_blockhash;
    var hashBytes = new Uint8Array(32);
    for (var h = 0; h < 32; h++) hashBytes[h] = parseInt(hashHex.substr(h * 2, 2), 16);
    writeBytes(hashBytes);

    var totalLen = parts.reduce(function(s, p) { return s + p.length; }, 0);
    var result = new Uint8Array(totalLen);
    var offset = 0;
    for (var k = 0; k < parts.length; k++) { result.set(parts[k], offset); offset += parts[k].length; }
    return result;
}

// ── Pagination ──

function updatePagination(config) {
    var container = config.container;
    if (typeof container === 'string') container = document.getElementById(container);
    if (!container) return;

    var currentPage = config.currentPage || 1;
    var totalPages = config.totalPages || 1;
    var onPageChange = config.onPageChange;

    container.innerHTML = '';
    if (totalPages <= 1) return;

    function addBtn(label, page, disabled, active) {
        var btn = document.createElement('button');
        btn.className = 'pagination-btn' + (active ? ' active' : '');
        btn.textContent = label;
        btn.disabled = disabled;
        if (!disabled && onPageChange) {
            btn.onclick = function() { onPageChange(page); };
        }
        container.appendChild(btn);
    }

    addBtn('«', 1, currentPage === 1, false);
    addBtn('‹', currentPage - 1, currentPage === 1, false);

    var startPage = Math.max(1, currentPage - 2);
    var endPage = Math.min(totalPages, currentPage + 2);
    for (var p = startPage; p <= endPage; p++) {
        addBtn(String(p), p, false, p === currentPage);
    }

    addBtn('›', currentPage + 1, currentPage === totalPages, false);
    addBtn('»', totalPages, currentPage === totalPages, false);
}

// ── Extended Formatters ──

function formatTimeFull(timestamp) {
    if (!timestamp || timestamp <= 0) return 'Genesis';
    var date = new Date(timestamp * 1000);
    var now = new Date();
    var diff = Math.floor((now - date) / 1000);

    var ago = '';
    if (diff < 60) ago = diff + ' seconds ago';
    else if (diff < 3600) ago = Math.floor(diff / 60) + ' minutes ago';
    else if (diff < 86400) ago = Math.floor(diff / 3600) + ' hours ago';
    else ago = Math.floor(diff / 86400) + ' days ago';

    return date.toLocaleString() + ' (' + ago + ')';
}

function formatTimeShort(timestamp) {
    if (timestamp === null || timestamp === undefined) return 'N/A';
    if (timestamp <= 0) return 'Genesis';
    return new Date(timestamp * 1000).toLocaleString();
}

function formatShells(shells) {
    return formatNumber(shells) + ' shells';
}

// console.log('✅ shared/utils.js loaded');
