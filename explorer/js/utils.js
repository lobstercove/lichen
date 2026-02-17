// MoltChain Explorer - Shared Utilities
// Single source of truth for formatting, clipboard, and common helpers.
// All other JS files should use these instead of redefining them.

function formatNumber(num) {
    if (num === null || num === undefined || Number.isNaN(num)) return '0';
    return Number(num).toLocaleString();
}

function formatSlot(slot) {
    if (slot === null || slot === undefined) return 'N/A';
    return slot.toLocaleString();
}

function formatHash(hash, length = 6) {
    if (!hash) return 'N/A';
    if (hash.length <= length * 2 + 3) return hash;
    return hash.substring(0, length) + '...' + hash.substring(hash.length - length);
}

// Truncated display for addresses — always use formatHash for consistent ABCDEF...GHIJKL format.
// Copy and links always use the full address; this is display only.
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
    // Auto-detect seconds vs milliseconds
    const ts = timestamp < 1e12 ? timestamp : timestamp / 1000;
    const now = Date.now() / 1000;
    const diff = now - ts;
    if (diff < 0) return 'just now';
    if (diff < 60) return Math.floor(diff) + 's ago';
    if (diff < 3600) return Math.floor(diff / 60) + 'm ago';
    if (diff < 86400) return Math.floor(diff / 3600) + 'h ago';
    return Math.floor(diff / 86400) + 'd ago';
}

// Alias used by address.js
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

function formatValidator(validator) {
    if (validator === '11111111111111111111111111111111' ||
        validator === '1111111111111111111111111111111111111111') {
        return '<span class="pill pill-info" style="background: var(--bg-secondary);">Genesis</span>';
    }
    return formatAddress(validator);
}

function readLeU64(bytes) {
    if (!bytes || bytes.length < 8) return null;
    let value = 0;
    for (let i = 0; i < 8; i++) {
        value += bytes[i] * Math.pow(256, i);
    }
    return value;
}

function resolveTxAmountShells(tx, instruction) {
    if (tx.amount_shells !== undefined) return tx.amount_shells;
    if (tx.amount !== undefined) return Math.round(tx.amount * 1_000_000_000);
    const SYSTEM_ID = typeof SYSTEM_PROGRAM_ID !== 'undefined'
        ? SYSTEM_PROGRAM_ID : '11111111111111111111111111111111';
    if (instruction && instruction.program_id === SYSTEM_ID) {
        const data = instruction.data || [];
        if (data.length >= 9) return readLeU64(data.slice(1, 9));
    }
    return null;
}

function resolveTxType(tx, instruction) {
    if (tx.type) return tx.type === 'DebtRepay' ? 'GrantRepay' : tx.type;
    const SYSTEM_ID = typeof SYSTEM_PROGRAM_ID !== 'undefined'
        ? SYSTEM_PROGRAM_ID : '11111111111111111111111111111111';
    if (instruction && instruction.program_id === SYSTEM_ID) {
        const opcode = instruction.data && instruction.data.length > 0 ? instruction.data[0] : null;
        if (opcode === 2) return 'Reward';
        if (opcode === 3) return 'GrantRepay';
        if (opcode === 4) return 'GenesisTransfer';
        if (opcode === 5) return 'GenesisMint';
        return 'Transfer';
    }
    if (instruction) return 'Contract';
    return 'Unknown';
}

function copyToClipboard(text) {
    navigator.clipboard.writeText(text).then(() => {
        showToast('Copied to clipboard!');
    }).catch(err => {
        console.error('Failed to copy:', err);
    });
}

function showToast(message) {
    const toast = document.createElement('div');
    toast.className = 'toast';
    toast.textContent = message;
    toast.style.cssText = `
        position: fixed; bottom: 2rem; right: 2rem;
        background: var(--success); color: white;
        padding: 1rem 1.5rem; border-radius: 8px; font-weight: 600;
        box-shadow: 0 4px 16px rgba(0,0,0,0.3); z-index: 10000;
        animation: slideIn 0.3s ease;
    `;
    document.body.appendChild(toast);
    setTimeout(() => toast.remove(), 3000);
}

// Toast animation (injected once)
if (!document.getElementById('_utils_toast_css')) {
    const s = document.createElement('style');
    s.id = '_utils_toast_css';
    s.textContent = `@keyframes slideIn { from { transform: translateX(400px); opacity: 0; } to { transform: translateX(0); opacity: 1; } }`;
    document.head.appendChild(s);
}

// ── Bincode Message Serializer ──
// Produces the same bytes as Rust's `bincode::serialize(&Message)` so signatures match.
// Shared between wallet and explorer for any page that signs transactions.
function serializeMessageBincode(message) {
    const parts = [];
    function writeU64LE(n) {
        const buf = new ArrayBuffer(8);
        const view = new DataView(buf);
        view.setBigUint64(0, BigInt(n), true);
        parts.push(new Uint8Array(buf));
    }
    function writeBytes(bytes) { parts.push(new Uint8Array(bytes)); }

    const ixs = message.instructions || [];
    writeU64LE(ixs.length);
    for (const ix of ixs) {
        writeBytes(ix.program_id);
        const accounts = ix.accounts || [];
        writeU64LE(accounts.length);
        for (const acct of accounts) writeBytes(acct);
        const data = ix.data || [];
        writeU64LE(data.length);
        writeBytes(data);
    }

    const hashHex = message.blockhash || message.recent_blockhash;
    const hashBytes = new Uint8Array(32);
    for (let i = 0; i < 32; i++) hashBytes[i] = parseInt(hashHex.substr(i * 2, 2), 16);
    writeBytes(hashBytes);

    const totalLen = parts.reduce((s, p) => s + p.length, 0);
    const result = new Uint8Array(totalLen);
    let offset = 0;
    for (const p of parts) { result.set(p, offset); offset += p.length; }
    return result;
}

// console.log('✅ Shared utils.js loaded');
