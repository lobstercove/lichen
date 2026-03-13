// ============================================================================
// MoltChain Shared Utilities
// Single source of truth for common JS helpers used across all frontends.
// Import via <script src="../shared/utils.js"></script> BEFORE app scripts.
// ============================================================================

// ── Protocol Constants ──
// These mirror on-chain parameters. If protocol upgrades change them,
// update here — all frontends read from this single source.

const SHELLS_PER_MOLT = 1_000_000_000;
const MS_PER_SLOT = 400;
const SLOTS_PER_EPOCH = 432_000;
const SLOTS_PER_YEAR = 78_840_000;
const SLOTS_PER_DAY = 86_400_000 / MS_PER_SLOT; // 216000 at 400ms
const BASE_FEE_SHELLS = 1_000_000; // 0.001 MOLT
const BASE_FEE_MOLT = BASE_FEE_SHELLS / SHELLS_PER_MOLT; // 0.001

// Fee distribution ratios (must sum to 1.0)
const FEE_SPLIT = {
    burned: 0.40,
    producer: 0.30,
    voters: 0.10,
    treasury: 0.10,
    community: 0.10,
};

// ZK shielded transaction compute surcharges (shells)
const ZK_COMPUTE_FEE = {
    shield: 100_000,
    unshield: 150_000,
    transfer: 200_000,
};

// MoltyID reputation constants (matches RPC moltyid_trust_tier in rpc/src/lib.rs)
const MAX_REPUTATION = 100_000;       // on-chain cap from contracts/moltyid MAX_REPUTATION
const MAX_REP_PROGRESS_BAR = 10_000;  // Legendary tier threshold — progress bar caps here
const TRUST_TIER_THRESHOLDS = [
    { min: 10000, label: 'Legendary', className: 'legendary', tier: 5 },
    { min: 5000, label: 'Elite', className: 'elite', tier: 4 },
    { min: 1000, label: 'Established', className: 'established', tier: 3 },
    { min: 500, label: 'Trusted', className: 'trusted', tier: 2 },
    { min: 100, label: 'Verified', className: 'verified', tier: 1 },
    { min: 0, label: 'Newcomer', className: 'newcomer', tier: 0 },
];

// MoltyID achievement definitions (matches RPC moltyid_achievement_name in rpc/src/lib.rs)
const ACHIEVEMENT_DEFS = [
    // Identity (1-12)
    { id: 1, name: 'First Transaction', icon: 'fa-exchange-alt', desc: 'Sent your first transaction' },
    { id: 2, name: 'Governance Voter', icon: 'fa-vote-yea', desc: 'Voted on a governance proposal' },
    { id: 3, name: 'Program Builder', icon: 'fa-code', desc: 'Deployed a program to MoltChain' },
    { id: 4, name: 'Trusted Agent', icon: 'fa-shield-alt', desc: 'Reached 500+ reputation' },
    { id: 5, name: 'Veteran Agent', icon: 'fa-medal', desc: 'Reached 1,000+ reputation' },
    { id: 6, name: 'Legendary Agent', icon: 'fa-crown', desc: 'Reached 5,000+ reputation' },
    { id: 7, name: 'Well Endorsed', icon: 'fa-handshake', desc: 'Received 10+ vouches' },
    { id: 8, name: 'Bootstrap Graduation', icon: 'fa-graduation-cap', desc: 'Completed bootstrap graduation' },
    { id: 9, name: 'Name Registrar', icon: 'fa-at', desc: 'Registered a .molt name' },
    { id: 10, name: 'Skill Master', icon: 'fa-tools', desc: 'Added 5+ skills to your profile' },
    { id: 11, name: 'Social Butterfly', icon: 'fa-users', desc: 'Received 3+ vouches' },
    { id: 12, name: 'First Name', icon: 'fa-id-card', desc: 'Registered your first .molt name' },
    // DEX (13-21)
    { id: 13, name: 'First Trade', icon: 'fa-chart-line', desc: 'Executed your first DEX swap' },
    { id: 14, name: 'LP Provider', icon: 'fa-water', desc: 'Added liquidity to a pool' },
    { id: 15, name: 'LP Withdrawal', icon: 'fa-faucet', desc: 'Removed liquidity from a pool' },
    { id: 16, name: 'DEX User', icon: 'fa-random', desc: 'Used the DEX multiple times' },
    { id: 17, name: 'Multi-hop Trader', icon: 'fa-route', desc: 'Executed a multi-hop swap via DEX Router' },
    { id: 18, name: 'Margin Trader', icon: 'fa-chart-bar', desc: 'Opened a margin position' },
    { id: 19, name: 'Position Closer', icon: 'fa-compress-alt', desc: 'Closed a margin position' },
    { id: 20, name: 'Yield Farmer', icon: 'fa-seedling', desc: 'Claimed DEX rewards' },
    { id: 21, name: 'Analytics Explorer', icon: 'fa-chart-pie', desc: 'Used DEX analytics tracking' },
    // Lending (31-38)
    { id: 31, name: 'First Lend', icon: 'fa-hand-holding-usd', desc: 'Deposited into LobsterLend' },
    { id: 32, name: 'First Borrow', icon: 'fa-file-invoice-dollar', desc: 'Borrowed from LobsterLend' },
    { id: 33, name: 'Loan Repaid', icon: 'fa-check-circle', desc: 'Repaid a LobsterLend loan' },
    { id: 34, name: 'Liquidator', icon: 'fa-gavel', desc: 'Liquidated an undercollateralized position' },
    { id: 35, name: 'Withdrawal Expert', icon: 'fa-sign-out-alt', desc: 'Withdrew from LobsterLend' },
    { id: 36, name: 'Stablecoin Minter', icon: 'fa-coins', desc: 'Minted MUSD stablecoins' },
    { id: 37, name: 'Stablecoin Redeemer', icon: 'fa-undo', desc: 'Redeemed MUSD stablecoins' },
    { id: 38, name: 'Stable Sender', icon: 'fa-paper-plane', desc: 'Transferred MUSD to another user' },
    // Staking (41-48)
    { id: 41, name: 'First Stake', icon: 'fa-layer-group', desc: 'Staked MOLT for the first time' },
    { id: 42, name: 'Unstaked', icon: 'fa-unlock', desc: 'Unstaked MOLT' },
    { id: 43, name: 'ReefStake Pioneer', icon: 'fa-fish', desc: 'Used ReefStake liquid staking' },
    { id: 44, name: 'Locked Staker', icon: 'fa-lock', desc: 'Locked stake for a fixed period' },
    { id: 45, name: 'Diamond Hands', icon: 'fa-gem', desc: 'Locked stake for 365 days' },
    { id: 46, name: 'Whale Staker', icon: 'fa-whale', desc: 'Staked a large amount' },
    { id: 47, name: 'Reward Harvester', icon: 'fa-gift', desc: 'Claimed staking rewards' },
    { id: 48, name: 'stMOLT Transferrer', icon: 'fa-share', desc: 'Transferred stMOLT tokens' },
    // Bridge (51-56)
    { id: 51, name: 'Bridge Pioneer', icon: 'fa-bridge', desc: 'Bridged assets to MoltChain' },
    { id: 52, name: 'Bridge Out', icon: 'fa-sign-out-alt', desc: 'Bridged assets out of MoltChain' },
    { id: 53, name: 'Bridge User', icon: 'fa-exchange-alt', desc: 'Used the bridge multiple times' },
    { id: 54, name: 'Wrapper', icon: 'fa-box', desc: 'Wrapped native tokens (WETH/WBNB/WSOL)' },
    { id: 55, name: 'Unwrapper', icon: 'fa-box-open', desc: 'Unwrapped tokens back to native' },
    { id: 56, name: 'Cross-chain Trader', icon: 'fa-globe', desc: 'Traded cross-chain assets' },
    // Shield/Privacy (57-60)
    { id: 57, name: 'Privacy Pioneer', icon: 'fa-user-secret', desc: 'Shielded assets for privacy' },
    { id: 58, name: 'Unshielded', icon: 'fa-eye', desc: 'Unshielded private assets' },
    { id: 59, name: 'Shadow Sender', icon: 'fa-mask', desc: 'Sent a shielded transfer' },
    { id: 60, name: 'ZK Privacy User', icon: 'fa-user-shield', desc: 'Used privacy features multiple times' },
    // NFT (63-70)
    { id: 63, name: 'Collection Creator', icon: 'fa-palette', desc: 'Created an NFT collection' },
    { id: 64, name: 'First Mint', icon: 'fa-stamp', desc: 'Minted your first NFT' },
    { id: 65, name: 'NFT Trader', icon: 'fa-store', desc: 'Traded NFTs on the marketplace' },
    { id: 66, name: 'First Listing', icon: 'fa-tag', desc: 'Listed an NFT for sale' },
    { id: 67, name: 'First Purchase', icon: 'fa-shopping-cart', desc: 'Purchased an NFT' },
    { id: 68, name: 'Bidder', icon: 'fa-gavel', desc: 'Placed a bid on an NFT' },
    { id: 69, name: 'Deal Maker', icon: 'fa-handshake', desc: 'Accepted an offer on an NFT' },
    { id: 70, name: 'Punk Collector', icon: 'fa-robot', desc: 'Interacted with MoltPunks' },
    // Governance (71-73)
    { id: 71, name: 'Proposal Creator', icon: 'fa-scroll', desc: 'Created a governance proposal' },
    { id: 72, name: 'First Vote', icon: 'fa-ballot-check', desc: 'Cast your first governance vote' },
    { id: 73, name: 'Delegator', icon: 'fa-people-arrows', desc: 'Delegated governance voting power' },
    // Oracle (81-82)
    { id: 81, name: 'Oracle Reporter', icon: 'fa-satellite-dish', desc: 'Submitted a price feed report' },
    { id: 82, name: 'Oracle User', icon: 'fa-broadcast-tower', desc: 'Consumed oracle price data' },
    // Storage (86-88)
    { id: 86, name: 'File Uploader', icon: 'fa-cloud-upload-alt', desc: 'Uploaded a file to Reef Storage' },
    { id: 87, name: 'Data Retriever', icon: 'fa-cloud-download-alt', desc: 'Retrieved data from Reef Storage' },
    { id: 88, name: 'Storage User', icon: 'fa-database', desc: 'Used decentralized storage' },
    // Marketplace/Auction (91-93)
    { id: 91, name: 'Auctioneer', icon: 'fa-bullhorn', desc: 'Created an auction' },
    { id: 92, name: 'Auction Bidder', icon: 'fa-hand-paper', desc: 'Bid on an auction' },
    { id: 93, name: 'Auction Winner', icon: 'fa-trophy', desc: 'Won an auction' },
    // Bounty (96-98)
    { id: 96, name: 'Bounty Poster', icon: 'fa-clipboard-list', desc: 'Posted a bounty' },
    { id: 97, name: 'Bounty Hunter', icon: 'fa-crosshairs', desc: 'Claimed a bounty reward' },
    { id: 98, name: 'Bounty Judge', icon: 'fa-balance-scale', desc: 'Judged a bounty submission' },
    // Prediction (101-104)
    { id: 101, name: 'Market Maker', icon: 'fa-chart-area', desc: 'Created a prediction market' },
    { id: 102, name: 'First Prediction', icon: 'fa-dice', desc: 'Placed your first prediction' },
    { id: 103, name: 'Oracle Resolver', icon: 'fa-check-double', desc: 'Resolved a prediction market' },
    { id: 104, name: 'Prediction Winner', icon: 'fa-star', desc: 'Won a prediction market payout' },
    // General milestones (106-124)
    { id: 106, name: 'Big Spender', icon: 'fa-money-bill-wave', desc: 'Sent a transaction worth 10,000+ MOLT' },
    { id: 107, name: 'Whale Transfer', icon: 'fa-whale', desc: 'Sent a transaction worth 100,000+ MOLT' },
    { id: 108, name: 'EVM Connected', icon: 'fa-link', desc: 'Registered an EVM address' },
    { id: 109, name: 'Identity Created', icon: 'fa-id-badge', desc: 'Created your MoltyID identity' },
    { id: 110, name: 'Profile Customizer', icon: 'fa-paint-brush', desc: 'Customized your MoltyID profile' },
    { id: 111, name: 'Voucher', icon: 'fa-thumbs-up', desc: 'Vouched for another identity' },
    { id: 112, name: 'Agent Creator', icon: 'fa-robot', desc: 'Created a compute agent' },
    { id: 113, name: 'Compute Provider', icon: 'fa-server', desc: 'Provided compute resources' },
    { id: 114, name: 'Compute Consumer', icon: 'fa-microchip', desc: 'Consumed compute resources' },
    { id: 115, name: 'Payment Creator', icon: 'fa-file-invoice', desc: 'Created a ClawPay payment' },
    { id: 116, name: 'First Payment', icon: 'fa-credit-card', desc: 'Claimed a ClawPay payment' },
    { id: 117, name: 'Subscription Creator', icon: 'fa-calendar-check', desc: 'Created a subscription plan' },
    { id: 118, name: 'Token Launcher', icon: 'fa-rocket', desc: 'Launched a token on ClawPump' },
    { id: 119, name: 'Early Buyer', icon: 'fa-bolt', desc: 'Bought tokens on ClawPump early' },
    { id: 120, name: 'Token Seller', icon: 'fa-cash-register', desc: 'Sold tokens on ClawPump' },
    { id: 121, name: 'Vault Depositor', icon: 'fa-piggy-bank', desc: 'Deposited into ClawVault' },
    { id: 122, name: 'Vault Withdrawer', icon: 'fa-wallet', desc: 'Withdrew from ClawVault' },
    { id: 123, name: 'Token Contract User', icon: 'fa-coins', desc: 'Interacted with a MoltCoin token' },
    { id: 124, name: 'Contract Interactor', icon: 'fa-cog', desc: 'Interacted with a smart contract' },
];

function getTrustTier(score) {
    const s = Number(score) || 0;
    for (const tier of TRUST_TIER_THRESHOLDS) {
        if (s >= tier.min) return tier;
    }
    return TRUST_TIER_THRESHOLDS[TRUST_TIER_THRESHOLDS.length - 1];
}

function getTrustTierNumber(score) {
    const s = Number(score) || 0;
    for (let i = 0; i < TRUST_TIER_THRESHOLDS.length; i++) {
        if (s >= TRUST_TIER_THRESHOLDS[i].min) return TRUST_TIER_THRESHOLDS.length - i;
    }
    return 0;
}

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

// Normalize legacy transaction type names to current terminology
function normalizeTxType(type) {
    if (!type) return type;
    return type === 'DebtRepay' ? 'GrantRepay' : type;
}

function formatMolt(shells) {
    const molt = shells / SHELLS_PER_MOLT;
    return molt.toLocaleString(undefined, {
        minimumFractionDigits: 2,
        maximumFractionDigits: 9,
    }) + ' MOLT';
}

function formatMoltShells(shells) {
    return formatMolt(shells);
}

/**
 * Format a MOLT amount preserving all significant decimals (up to 9).
 * Accepts a number or string. Strips trailing zeros but keeps at least 2 decimals.
 */
function formatMoltExact(molt) {
    if (molt === null || molt === undefined || Number.isNaN(Number(molt))) return '0';
    return Number(molt).toLocaleString(undefined, {
        minimumFractionDigits: 2,
        maximumFractionDigits: 9,
    });
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
    navigator.clipboard.writeText(text).then(function () {
        showToast('Copied to clipboard!');
    }).catch(function (err) {
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
    setTimeout(function () { toast.remove(); }, 3000);
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
        if (typeof MOLT_CONFIG !== 'undefined' && typeof MOLT_CONFIG.rpc === 'function') return MOLT_CONFIG.rpc();
        if (typeof MOLT_CONFIG !== 'undefined' && MOLT_CONFIG?.rpc) return MOLT_CONFIG.rpc;
        if (window.MOLT_CONFIG && typeof window.MOLT_CONFIG.rpc === 'function') return window.MOLT_CONFIG.rpc();
        if (window.MOLT_CONFIG && window.MOLT_CONFIG.rpc) return window.MOLT_CONFIG.rpc;
        if (window.moltConfig && window.moltConfig.rpcUrl) return window.moltConfig.rpcUrl;
        if (window.moltMarketConfig && window.moltMarketConfig.rpcUrl) return window.moltMarketConfig.rpcUrl;
        if (window.moltExplorerConfig && window.moltExplorerConfig.rpcUrl) return window.moltExplorerConfig.rpcUrl;
    }
    return 'http://localhost:8899';
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
    if (!hashHex || typeof hashHex !== 'string' || !/^[0-9a-fA-F]{64}$/.test(hashHex)) {
        throw new Error('Invalid or missing blockhash: must be a 64-character hex string');
    }
    var hashBytes = new Uint8Array(32);
    for (var h = 0; h < 32; h++) hashBytes[h] = parseInt(hashHex.substr(h * 2, 2), 16);
    writeBytes(hashBytes);

    var totalLen = parts.reduce(function (s, p) { return s + p.length; }, 0);
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
            btn.onclick = function () { onPageChange(page); };
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

// ── Chain Status Bar — auto-wire any page with id="chainBlockHeight" ──
(function initChainStatusBarShared() {
    if (typeof document === 'undefined') return;
    function wire() {
        // If the page has its own status-bar poller (e.g. wallet.js), yield to it
        if (window.__chainStatusBarOwned) return;
        var blockEl = document.getElementById('chainBlockHeight');
        if (!blockEl) return; // No status bar on this page
        var dotEl = document.getElementById('chainDot');
        var latEl = document.getElementById('chainLatency');
        var currentBlock = 0;

        function poll() {
            // Re-check every cycle: wallet.js may have claimed ownership after we started
            if (window.__chainStatusBarOwned) return;
            var t0 = (typeof performance !== 'undefined') ? performance.now() : Date.now();
            moltRpcCall('getSlot', []).then(function (slot) {
                var ms = Math.round(((typeof performance !== 'undefined') ? performance.now() : Date.now()) - t0);
                if (typeof slot === 'number' && slot > currentBlock) currentBlock = slot;
                blockEl.textContent = 'Block #' + currentBlock.toLocaleString();
                if (latEl) latEl.textContent = ms + ' ms';
                if (dotEl) { dotEl.classList.add('connected'); dotEl.classList.remove('disconnected'); }
            }).catch(function () {
                blockEl.textContent = 'Reconnecting\u2026';
                if (latEl) latEl.textContent = '';
                if (dotEl) { dotEl.classList.remove('connected'); dotEl.classList.add('disconnected'); }
            });
        }

        poll();
        setInterval(poll, 5000);
    }

    if (document.readyState === 'loading') {
        document.addEventListener('DOMContentLoaded', wire);
    } else {
        wire();
    }
})();

// ── Node.js / CommonJS exports (for test harnesses) ──
if (typeof module !== 'undefined' && module.exports) {
    module.exports = {
        SHELLS_PER_MOLT, MS_PER_SLOT, SLOTS_PER_EPOCH, SLOTS_PER_YEAR,
        SLOTS_PER_DAY, BASE_FEE_SHELLS, BASE_FEE_MOLT, FEE_SPLIT,
        ZK_COMPUTE_FEE, MAX_REPUTATION, MAX_REP_PROGRESS_BAR,
        TRUST_TIER_THRESHOLDS, ACHIEVEMENT_DEFS,
        getTrustTier, getTrustTierNumber,
        escapeHtml, formatNumber, formatHash, formatAddress, normalizeTxType,
        formatMolt, formatMoltShells, formatTime, timeAgo,
        formatBytes, formatSlot, formatTimeFull, formatTimeShort, formatShells,
        updatePagination,
        bs58encode, bs58decode, base58Encode, base58Decode,
        readLeU64, serializeMessageBincode,
        getMoltRpcUrl, moltRpcCall, rpcCall,
    };
}
