// Transaction Detail Page - Reef Explorer
// Uses `rpc` instance from explorer.js (loaded before this file)

const BASE_FEE = 1000000; // shells (from core/src/processor.rs — 0.001 MOLT)

// Utility Functions
function formatNumber(num) {
    if (num === null || num === undefined || Number.isNaN(num)) {
        return '0';
    }
    return Number(num).toLocaleString();
}

function formatMolt(shells) {
    const molt = shells / 1_000_000_000;
    const raw = molt.toLocaleString(undefined, {
        minimumFractionDigits: 0,
        maximumFractionDigits: 9,
    });
    return raw + ' MOLT';
}

function formatShells(shells) {
    return formatNumber(shells) + ' shells';
}

// NOTE: formatHash, formatAddress, etc. are provided by utils.js (loaded before this file).
// Only override formatHash locally if utils.js is NOT loaded.

function formatTime(timestamp) {
    if (!timestamp || timestamp <= 0) return 'Genesis';
    const date = new Date(timestamp * 1000);
    const now = new Date();
    const diff = Math.floor((now - date) / 1000);
    
    let timeAgo = '';
    if (diff < 60) timeAgo = diff + ' seconds ago';
    else if (diff < 3600) timeAgo = Math.floor(diff / 60) + ' minutes ago';
    else if (diff < 86400) timeAgo = Math.floor(diff / 3600) + ' hours ago';
    else timeAgo = Math.floor(diff / 86400) + ' days ago';
    
    return date.toLocaleString() + ' (' + timeAgo + ')';
}

function copyToClipboard(elementIdOrText) {
    const element = document.getElementById(elementIdOrText);
    const text = element ? (element.dataset.full || element.textContent) : elementIdOrText;
    navigator.clipboard.writeText(text).then(() => {
        if (element) {
            const original = element.innerHTML;
            element.innerHTML = '<i class="fas fa-check"></i> Copied!';
            element.style.color = 'var(--success)';
            setTimeout(() => {
                element.innerHTML = original;
                element.style.color = '';
            }, 2000);
        }
    });
}

// Get transaction hash from URL
function getTxHash() {
    const params = new URLSearchParams(window.location.search);
    return params.get('sig') || params.get('tx') || params.get('hash') || params.get('signature');
}

// Load and display transaction
async function loadTransaction() {
    const txHash = getTxHash();
    
    if (!txHash) {
        document.getElementById('txHash').textContent = 'Invalid';
        document.getElementById('txStatus').innerHTML = '<i class="fas fa-exclamation-circle"></i> Transaction not found';
        return;
    }

    // Detect airdrop signatures (format: airdrop-<timestamp_ms>)
    if (txHash.startsWith('airdrop-')) {
        displayAirdrop(txHash);
        return;
    }
    
    if (!rpc) {
        document.getElementById('txHash').textContent = txHash;
        document.getElementById('txStatus').innerHTML = '<i class="fas fa-exclamation-circle"></i> RPC unavailable';
        return;
    }

    const tx = await rpc.getTransaction(txHash);
    if (!tx) {
        document.getElementById('txHash').textContent = txHash;
        document.getElementById('txStatus').innerHTML = '<i class="fas fa-exclamation-circle"></i> Transaction not found';
        return;
    }
    
    // Update page
    await displayTransaction(tx);
}

function upsertParticipants(from, to, nameMap = {}) {
    const grid = document.querySelector('.detail-card .detail-card-body .detail-grid');
    const amountEl = document.getElementById('detailAmount');
    const amountRow = amountEl ? amountEl.closest('.detail-row') : null;
    if (!grid || !amountRow) return;

    let fromRow = document.getElementById('detailFromRow');
    let toRow = document.getElementById('detailToRow');

    if (!fromRow) {
        fromRow = document.createElement('div');
        fromRow.className = 'detail-row';
        fromRow.id = 'detailFromRow';
        amountRow.insertAdjacentElement('afterend', fromRow);
    }
    if (!toRow) {
        toRow = document.createElement('div');
        toRow.className = 'detail-row';
        toRow.id = 'detailToRow';
        fromRow.insertAdjacentElement('afterend', toRow);
    }

    const fromLabel = (typeof formatAddressWithMoltName === 'function' && from)
        ? formatAddressWithMoltName(from, nameMap[from], { includeAddressInLabel: true })
        : (from || 'N/A');
    const toLabel = (typeof formatAddressWithMoltName === 'function' && to)
        ? formatAddressWithMoltName(to, nameMap[to], { includeAddressInLabel: true })
        : (to || 'N/A');
    const fromIsAddress = typeof isLikelyMoltAddress === 'function' ? isLikelyMoltAddress(from) : false;
    const toIsAddress = typeof isLikelyMoltAddress === 'function' ? isLikelyMoltAddress(to) : false;

    fromRow.innerHTML = `
        <div class="detail-label">From</div>
        <div class="detail-value">${from ? (fromIsAddress ? `<a href="address.html?address=${from}" class="detail-link">${fromLabel}</a>` : fromLabel) : 'N/A'}</div>
    `;
    toRow.innerHTML = `
        <div class="detail-label">To</div>
        <div class="detail-value">${to ? (toIsAddress ? `<a href="address.html?address=${to}" class="detail-link">${toLabel}</a>` : toLabel) : 'N/A'}</div>
    `;
}

// Display airdrop details (airdrops are off-chain treasury operations, not indexed transactions)
async function displayAirdrop(txHash) {
    const params = new URLSearchParams(window.location.search);
    let recipient = params.get('to') || null;
    let amountMolt = parseFloat(params.get('amount')) || null;

    // Try fetching from faucet backend API (has airdrop history)
    if (!recipient || !amountMolt) {
        try {
            const faucetUrl = (typeof MOLT_CONFIG !== 'undefined' && MOLT_CONFIG.faucet) ? MOLT_CONFIG.faucet : 'http://localhost:9100';
            const resp = await fetch(`${faucetUrl}/faucet/airdrop/${encodeURIComponent(txHash)}`);
            if (resp.ok) {
                const record = await resp.json();
                if (!recipient && record.recipient) recipient = record.recipient;
                if (!amountMolt && record.amount_molt) amountMolt = record.amount_molt;
            }
        } catch (e) { /* faucet API unavailable */ }
    }

    if (!recipient) recipient = 'Unknown';
    if (!amountMolt) amountMolt = null;

    const timestampMs = parseInt(txHash.replace('airdrop-', ''), 10);
    const timestampSec = Math.floor(timestampMs / 1000);
    const amountDisplay = amountMolt
        ? formatMolt(Math.round(amountMolt * 1_000_000_000)) + ' (' + formatShells(Math.round(amountMolt * 1_000_000_000)) + ')'
        : 'Unknown';

    // Header
    document.getElementById('txHash').textContent = formatHash(txHash);
    document.getElementById('txHash').dataset.full = txHash;
    const statusBadge = '<span class="badge badge-success"><i class="fas fa-check-circle"></i> Success</span>';
    document.getElementById('txStatus').innerHTML = statusBadge;
    document.getElementById('detailStatus').innerHTML = statusBadge;

    // Block — airdrops have no block
    document.getElementById('blockLink').textContent = 'N/A (off-chain)';
    document.getElementById('detailBlockLink').textContent = 'N/A (off-chain)';

    // Timestamp
    document.getElementById('txTimestamp').textContent = formatTime(timestampSec);
    document.getElementById('detailTimestamp').textContent = formatTime(timestampSec);

    // Type
    document.getElementById('txType').textContent = 'Airdrop';
    document.getElementById('detailType').textContent = 'Airdrop';

    // Amount
    document.getElementById('txAmount').textContent = amountDisplay;
    document.getElementById('detailAmount').textContent = amountDisplay;

    let nameMap = {};
    try {
        if (typeof batchResolveMoltNames === 'function') {
            nameMap = await Promise.race([
                batchResolveMoltNames([recipient]),
                new Promise(r => setTimeout(() => r({}), 3000))
            ]);
        }
    } catch (e) { /* name resolution unavailable */ }
    upsertParticipants('Treasury', recipient, nameMap);

    // Fee — airdrops are fee-free
    document.getElementById('txFee').textContent = '0 MOLT';
    document.getElementById('totalFee').textContent = '0 MOLT (fee-free airdrop)';
    document.getElementById('baseFee').textContent = '0 MOLT (airdrop — no fee)';
    document.getElementById('feeNote').textContent = 'Airdrops are direct treasury operations with no transaction fees';
    document.getElementById('feeBurned').textContent = '0 MOLT';
    document.getElementById('feeProducer').textContent = '0 MOLT';
    document.getElementById('feeVoters').textContent = '0 MOLT';
    document.getElementById('feeTreasury').textContent = '0 MOLT';

    // Recent blockhash
    document.getElementById('recentBlockhash').textContent = 'N/A';

    // Instructions — show airdrop details instead
    document.getElementById('instructionCount').textContent = '1';
    const recipientDisplay = (typeof formatAddressWithMoltName === 'function' && recipient !== 'Unknown')
        ? formatAddressWithMoltName(recipient, nameMap[recipient], { includeAddressInLabel: true })
        : recipient;
    const recipientLink = recipient !== 'Unknown'
        ? `<a href="address.html?address=${recipient}" class="detail-link">${recipientDisplay}</a>`
        : 'Unknown';
    document.getElementById('instructionsList').innerHTML = `
        <div class="instruction-item">
            <div class="instruction-header">
                <strong><i class="fas fa-parachute-box"></i> Airdrop Details</strong>
            </div>
            <div class="detail-grid">
                <div class="detail-row">
                    <div class="detail-label">Type</div>
                    <div class="detail-value">Testnet Faucet Airdrop</div>
                </div>
                <div class="detail-row">
                    <div class="detail-label">Source</div>
                    <div class="detail-value">Treasury</div>
                </div>
                <div class="detail-row">
                    <div class="detail-label">Recipient</div>
                    <div class="detail-value">${recipientLink}</div>
                </div>
                <div class="detail-row">
                    <div class="detail-label">Amount</div>
                    <div class="detail-value">${amountMolt} MOLT</div>
                </div>
                <div class="detail-row">
                    <div class="detail-label">Note</div>
                    <div class="detail-value">Airdrop is a direct balance credit from the Treasury. It does not produce an on-chain transaction or consume a block slot.</div>
                </div>
            </div>
        </div>
    `;

    // Signatures — none for airdrops
    document.getElementById('signatureCount').textContent = '0';
    document.getElementById('signaturesList').innerHTML = '<div class="empty-state"><i class="fas fa-info-circle"></i> Airdrops do not produce cryptographic signatures</div>';

    // Raw data
    document.getElementById('rawData').textContent = JSON.stringify({
        type: 'Airdrop',
        signature: txHash,
        recipient: recipient,
        amount_molt: amountMolt,
        amount_shells: amountMolt ? Math.round(amountMolt * 1_000_000_000) : null,
        timestamp: timestampMs,
        source: 'Treasury',
        fee: 0,
        note: 'Off-chain treasury operation via requestAirdrop RPC'
    }, null, 2);
}

async function displayTransaction(tx) {
    const hash = tx.signature;
    const status = tx.status || 'Success';
    const typeRaw = tx.type === 'DebtRepay' ? 'GrantRepay' : (tx.type || 'Unknown');
    // Display-friendly type names
    const typeDisplayMap = {
        'ReefStakeDeposit': 'ReefStake Deposit',
        'ReefStakeUnstake': 'ReefStake Unstake',
        'ReefStakeClaim':   'ReefStake Claim',
        'ReefStakeTransfer': 'ReefStake Transfer',
        'DeployContract':   'Deploy Contract',
        'SetContractABI':   'Set Contract ABI',
        'FaucetAirdrop':    'Faucet Airdrop',
        'RegisterSymbol':   'Register Symbol',
        'RegisterEvmAddress': 'EVM Registration',
        'CreateCollection': 'Create Collection',
        'MintNFT':          'Mint NFT',
        'TransferNFT':      'Transfer NFT',
        'ClaimUnstake':     'Claim Unstake',
        'GrantRepay':       'Grant Repay',
        'GenesisTransfer':  'Genesis Transfer',
        'GenesisMint':      'Genesis Mint',
    };
    const type = typeDisplayMap[typeRaw] || typeRaw;
    const slot = tx.slot;
    const timestamp = tx.block_time || Math.floor(Date.now() / 1000);
    const fee = tx.fee_shells !== undefined ? tx.fee_shells : (tx.fee || BASE_FEE);
    const amountShells = tx.amount_shells !== undefined
        ? tx.amount_shells
        : Math.round((tx.amount || 0) * 1_000_000_000);
    const amountDisplay = amountShells > 0
        ? formatMolt(amountShells) + ' (' + formatShells(amountShells) + ')'
        : '-';
    const recentBlockhash = tx.message.recent_blockhash;
    const instructions = tx.message.instructions || [];
    const signatures = tx.signatures || [];
    const isFeeFree = fee === 0;
    const fromAddress = tx.from || instructions[0]?.accounts?.[0] || null;
    const toAddress = tx.to || instructions[0]?.accounts?.[1] || null;
    const instructionAccounts = instructions.flatMap(inst => inst.accounts || []);
    const nameMap = typeof batchResolveMoltNames === 'function'
        ? await batchResolveMoltNames([fromAddress, toAddress, ...instructionAccounts].filter(Boolean))
        : {};
    
    // Header
    document.getElementById('txHash').textContent = formatHash(hash);
    document.getElementById('txHash').dataset.full = hash;
    
    // Status
    const statusBadge = status === 'Success' 
        ? '<span class="badge badge-success"><i class="fas fa-check-circle"></i> Success</span>'
        : '<span class="badge badge-error"><i class="fas fa-times-circle"></i> Failed</span>';
    
    document.getElementById('txStatus').innerHTML = statusBadge;
    document.getElementById('detailStatus').innerHTML = statusBadge;
    
    // Block link
    if (slot !== undefined && slot !== null) {
        document.getElementById('blockLink').textContent = '#' + formatNumber(slot);
        document.getElementById('blockLink').href = `block.html?slot=${slot}`;
        document.getElementById('detailBlockLink').textContent = '#' + formatNumber(slot);
        document.getElementById('detailBlockLink').href = `block.html?slot=${slot}`;
    } else {
        document.getElementById('blockLink').textContent = '-';
        document.getElementById('detailBlockLink').textContent = '-';
    }
    
    // Timestamp
    document.getElementById('txTimestamp').textContent = formatTime(timestamp);
    document.getElementById('detailTimestamp').textContent = formatTime(timestamp);

    document.getElementById('txType').textContent = type;
    document.getElementById('detailType').textContent = type;
    document.getElementById('txAmount').textContent = amountDisplay;
    document.getElementById('detailAmount').textContent = amountDisplay;
    upsertParticipants(fromAddress, toAddress, nameMap);
    
    // Fee details
    document.getElementById('txFee').textContent = formatMolt(fee);
    document.getElementById('totalFee').textContent = formatMolt(fee) + ' (' + formatShells(fee) + ')';
    document.getElementById('baseFee').textContent = isFeeFree
        ? '0.000000000 MOLT (fee-free system tx)'
        : '0.001 MOLT (1,000,000 shells)';
    document.getElementById('feeNote').textContent = isFeeFree
        ? 'System reward/repay transactions are fee-free'
        : 'Fee split is applied to this transaction';
    const feeBurned = Math.floor(fee * 0.5);
    const feeProducer = Math.floor(fee * 0.3);
    const feeVoters = Math.floor(fee * 0.1);
    const feeTreasury = fee - feeBurned - feeProducer - feeVoters;
    document.getElementById('feeBurned').textContent = formatMolt(feeBurned) + ' (50%)';
    document.getElementById('feeProducer').textContent = formatMolt(feeProducer) + ' (30%)';
    document.getElementById('feeVoters').textContent = formatMolt(feeVoters) + ' (10%)';
    document.getElementById('feeTreasury').textContent = formatMolt(feeTreasury) + ' (10%)';
    
    // Recent blockhash
    document.getElementById('recentBlockhash').textContent = formatHash(recentBlockhash);
    document.getElementById('recentBlockhash').dataset.full = recentBlockhash;
    
    // Instructions
    document.getElementById('instructionCount').textContent = instructions.length;
    const instructionsList = document.getElementById('instructionsList');
    
    if (instructions.length === 0) {
        instructionsList.innerHTML = '<div class="empty-state"><i class="fas fa-inbox"></i> No instructions</div>';
    } else {
        instructionsList.innerHTML = instructions.map((inst, idx) => `
            <div class="instruction-item">
                <div class="instruction-header">
                    <strong>Instruction #${idx + 1}</strong>
                </div>
                <div class="detail-grid">
                    <div class="detail-row">
                        <div class="detail-label">Program ID</div>
                        <div class="detail-value">
                            <code title="${inst.program_id}">${formatHash(inst.program_id)}</code>
                            <a href="address.html?address=${inst.program_id}" class="detail-link">
                                <i class="fas fa-external-link-alt"></i>
                            </a>
                        </div>
                    </div>
                    <div class="detail-row">
                        <div class="detail-label">Accounts</div>
                        <div class="detail-value">
                            ${inst.accounts.map(acc => {
                                const accountDisplay = (typeof formatAddressWithMoltName === 'function')
                                    ? formatAddressWithMoltName(acc, nameMap[acc], { includeAddressInLabel: true })
                                    : acc;
                                return `
                                <div>
                                    <code>${accountDisplay}</code>
                                    <a href="address.html?address=${acc}" class="detail-link">
                                        <i class="fas fa-external-link-alt"></i>
                                    </a>
                                </div>
                                `;
                            }).join('')}
                        </div>
                    </div>
                    <div class="detail-row">
                        <div class="detail-label">Data</div>
                        <div class="detail-value">
                            <code>${inst.data.length} bytes: [${inst.data.slice(0, 20).join(', ')}${inst.data.length > 20 ? '...' : ''}]</code>
                        </div>
                    </div>
                </div>
            </div>
        `).join('');
    }
    
    // Signatures
    document.getElementById('signatureCount').textContent = signatures.length;
    const signaturesList = document.getElementById('signaturesList');
    
    if (signatures.length === 0) {
        signaturesList.innerHTML = '<div class="empty-state"><i class="fas fa-inbox"></i> No signatures</div>';
    } else {
        signaturesList.innerHTML = signatures.map((sig, idx) => {
            const sigHex = Array.isArray(sig) ? 
                '0x' + sig.map(b => b.toString(16).padStart(2, '0')).join('') :
                sig;
            return `
                <div class="signature-item">
                    <div class="detail-row">
                        <div class="detail-label">Signature #${idx + 1}</div>
                        <div class="detail-value">
                            <code title="${sigHex}">${formatHash(sigHex)}</code>
                            <button class="copy-icon" onclick="navigator.clipboard.writeText('${sigHex}')">
                                <i class="fas fa-copy"></i>
                            </button>
                        </div>
                    </div>
                </div>
            `;
        }).join('');
    }
    
    // Raw data
    document.getElementById('rawData').textContent = JSON.stringify(tx, null, 2);
}

// Search functionality
document.getElementById('searchInput')?.addEventListener('keypress', async (e) => {
    if (e.key === 'Enter') {
        const query = e.target.value.trim();
        if (query) {
            if (typeof navigateExplorerSearch === 'function') {
                await navigateExplorerSearch(query);
                return;
            }
            window.location.href = `address.html?address=${query}`;
        }
    }
});

// Initialize
window.addEventListener('DOMContentLoaded', () => {
    loadTransaction();
});
