// Block Detail Page - Reef Explorer
// Uses `rpc` instance from explorer.js (loaded before this file)
// NOTE: formatHash, formatAddress, formatNumber, formatBytes, copyToClipboard,
//       escapeHtml, safeCopy are provided by utils.js (loaded before this file)

// Block-specific time format (full date + relative)
function formatTimeShort(timestamp) {
    if (timestamp === null || timestamp === undefined) return 'N/A';
    if (timestamp <= 0) return 'Genesis';
    return new Date(timestamp * 1000).toLocaleString();
}

function formatTimeFull(timestamp) {
    if (timestamp === null || timestamp === undefined) return 'N/A';
    if (timestamp <= 0) return 'Genesis';
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

// Get block number from URL
function getBlockNumber() {
    const params = new URLSearchParams(window.location.search);
    return params.get('slot') || params.get('block');
}


// Load and display block
async function loadBlock() {
    const blockNumber = getBlockNumber();
    
    if (blockNumber === null || blockNumber === undefined || blockNumber === '') {
        document.getElementById('blockSlot').textContent = 'Invalid';
        document.getElementById('blockStatus').innerHTML = '<i class="fas fa-exclamation-circle"></i> Block not found';
        return;
    }
    
    if (!rpc) {
        document.getElementById('blockSlot').textContent = 'Unavailable';
        document.getElementById('blockStatus').innerHTML = '<i class="fas fa-exclamation-circle"></i> RPC unavailable';
        return;
    }

    // Try RPC first, show error if not found
    let block = await rpc.getBlock(parseInt(blockNumber));
    
    if (!block) {
        document.getElementById('blockSlot').textContent = blockNumber;
        document.getElementById('blockStatus').innerHTML = '<span class="badge warning">Block not found</span>';
        return;
    }
    
    // Update page
    await displayBlock(block);
}

async function displayBlock(block) {
    // Handle both old format (block.header) and new format (block.slot, block.hash)
    const slot = block.slot ?? block.header?.slot;
    const hash = block.hash ?? block.header?.hash ?? 'unknown';
    const parentHash = block.parent_hash ?? block.header?.parent_hash;
    const stateRoot = block.state_root ?? block.header?.state_root;
    const timestamp = block.timestamp ?? block.header?.timestamp;
    const validator = block.validator ?? block.header?.validator;
    const txCount = block.transaction_count ?? block.transactions?.length ?? 0;
    const transactions = block.transactions || [];
    const reward = block.block_reward;
    const size = block.size || JSON.stringify(block).length;
    
    // Header
    document.getElementById('blockSlot').textContent = formatNumber(slot);
    document.getElementById('blockNumber').textContent = 'Block #' + formatNumber(slot);
    document.getElementById('blockTimestamp').textContent = formatTimeShort(timestamp);
    document.getElementById('blockTxCount').textContent = txCount;
    document.getElementById('blockSize').textContent = formatBytes(size);
    
    // Calculate block time from previous block
    const blockTimeEl = document.getElementById('blockTime');
    if (slot > 0 && timestamp) {
        try {
            const prevBlock = await rpc.getBlock(slot - 1);
            const prevTs = prevBlock?.timestamp ?? prevBlock?.header?.timestamp;
            if (prevTs && timestamp >= prevTs) {
                const deltaSec = timestamp - prevTs;
                if (deltaSec > 0) {
                    const deltaMs = deltaSec * 1000;
                    blockTimeEl.textContent = deltaMs >= 1000 ? (deltaMs / 1000).toFixed(1) + 's' : deltaMs + 'ms';
                } else {
                    // Same-second timestamps (sub-second block production)
                    blockTimeEl.textContent = '<1s';
                }
            } else {
                blockTimeEl.textContent = '—';
            }
        } catch (e) {
            blockTimeEl.textContent = '—';
        }
    } else {
        blockTimeEl.textContent = slot === 0 ? 'Genesis' : '—';
    }
    
    // Detail grid
    document.getElementById('detailSlot').textContent = formatNumber(slot);
    document.getElementById('blockHash').textContent = formatHash(hash);
    document.getElementById('blockHash').dataset.full = hash;
    document.getElementById('parentHash').textContent = formatHash(parentHash);
    document.getElementById('parentHash').dataset.full = parentHash;
    document.getElementById('stateRoot').textContent = formatHash(stateRoot);
    document.getElementById('stateRoot').dataset.full = stateRoot;
    document.getElementById('detailTimestamp').textContent = formatTimeFull(timestamp);
    const addressNames = typeof batchResolveMoltNames === 'function'
        ? await batchResolveMoltNames([
            validator,
            ...transactions.flatMap(tx => [tx.from, tx.to]),
            reward?.recipient
        ])
        : {};

    const validatorDisplay = addressNames[validator] && typeof formatAddressWithMoltName === 'function'
        ? formatAddressWithMoltName(validator, addressNames[validator])
        : formatAddress(validator);
    document.getElementById('validator').innerHTML = validatorDisplay;
    document.getElementById('detailTxCount').textContent = formatNumber(txCount);
    document.getElementById('detailSize').textContent = formatBytes(size);
    
    // Set links
    if (slot > 0) {
        document.getElementById('parentLink').href = `block.html?slot=${slot - 1}`;
        document.getElementById('prevBlock').disabled = false;
        document.getElementById('prevBlock').onclick = () => {
            window.location.href = `block.html?slot=${slot - 1}`;
        };
    }
    
    document.getElementById('nextBlock').disabled = false;
    document.getElementById('nextBlock').onclick = () => {
        window.location.href = `block.html?slot=${slot + 1}`;
    };
    
    document.getElementById('validatorLink').href = `address.html?address=${validator}`;
    
    // Transactions table
    document.getElementById('txCount').textContent = txCount;
    const tbody = document.getElementById('transactionsTable');
    
    if (txCount === 0) {
        tbody.innerHTML = `
            <tr>
                <td colspan="6" class="empty-state">
                    <i class="fas fa-inbox"></i>
                    <div>No transactions in this block</div>
                </td>
            </tr>
        `;
    } else {
        tbody.innerHTML = transactions.map(tx => {
            const safeSig = typeof escapeHtml === 'function' ? escapeHtml(tx.signature) : tx.signature;
            const safeFrom = typeof escapeHtml === 'function' ? escapeHtml(tx.from) : tx.from;
            const safeTo = typeof escapeHtml === 'function' ? escapeHtml(tx.to) : tx.to;
            const safeType = typeof escapeHtml === 'function' ? escapeHtml(tx.type || 'Transfer') : (tx.type || 'Transfer');
            const safeStatus = typeof escapeHtml === 'function' ? escapeHtml(tx.status || 'Success') : (tx.status || 'Success');
            const fromDisplay = addressNames[tx.from] && typeof formatAddressWithMoltName === 'function'
                ? formatAddressWithMoltName(tx.from, addressNames[tx.from])
                : formatAddress(tx.from);
            const toDisplay = addressNames[tx.to] && typeof formatAddressWithMoltName === 'function'
                ? formatAddressWithMoltName(tx.to, addressNames[tx.to])
                : formatAddress(tx.to);
            return `
            <tr>
                <td>
                    <a href="transaction.html?tx=${encodeURIComponent(tx.signature)}" class="hash-link" title="${safeSig}">
                        ${formatHash(tx.signature)}
                    </a>
                </td>
                <td>
                    <a href="address.html?address=${encodeURIComponent(tx.from)}" class="hash-link">
                        ${fromDisplay}
                    </a>
                </td>
                <td>
                    <a href="address.html?address=${encodeURIComponent(tx.to)}" class="hash-link">
                        ${toDisplay}
                    </a>
                </td>
                <td><span class="badge badge-info">${safeType}</span></td>
                <td>
                    <span class="badge ${tx.status === 'Success' ? 'badge-success' : 'badge-error'}">
                        ${safeStatus}
                    </span>
                </td>
                <td>
                    <a href="transaction.html?tx=${encodeURIComponent(tx.signature)}" class="btn btn-small">
                        <i class="fas fa-eye"></i> View
                    </a>
                </td>
            </tr>
        `;
        }).join('');
    }
    
    // Block Reward (protocol-level coinbase)
    const rewardCard = document.getElementById('rewardCard');
    if (reward && reward.amount > 0 && rewardCard && slot > 0) {
        rewardCard.style.display = '';
        document.getElementById('rewardAmount').textContent =
            reward.amount_molt.toFixed(3) + ' MOLT (' + formatNumber(reward.amount) + ' shells)';
        const typeLabel = reward.type === 'heartbeat' ? 'Heartbeat' : 'Transaction Block';
        document.getElementById('rewardType').innerHTML =
            '<span class="badge badge-info">' + typeLabel + '</span>';
        const rewardDisplay = addressNames[reward.recipient] && typeof formatAddressWithMoltName === 'function'
            ? formatAddressWithMoltName(reward.recipient, addressNames[reward.recipient])
            : formatAddress(reward.recipient);
        document.getElementById('rewardRecipient').innerHTML = rewardDisplay;
        document.getElementById('rewardRecipientLink').href = 'address.html?address=' + reward.recipient;
    }

    // Fee Distribution (only for blocks with transactions)
    const feeCard = document.getElementById('feeCard');
    if (feeCard && txCount > 0) {
        // Sum up all tx fees in this block
        let totalFee = 0;
        for (const tx of transactions) {
            const fee = tx.fee_shells !== undefined ? tx.fee_shells : (tx.fee || 0);
            totalFee += fee;
        }
        if (totalFee > 0) {
            feeCard.style.display = '';
            const burned = Math.floor(totalFee * 0.5);
            const producer = Math.floor(totalFee * 0.3);
            const voters = Math.floor(totalFee * 0.1);
            const treasury = totalFee - burned - producer - voters;
            const fmt = (shells) => {
                const molt = shells / 1_000_000_000;
                return molt.toLocaleString(undefined, { minimumFractionDigits: 0, maximumFractionDigits: 9 }) + ' MOLT';
            };
            document.getElementById('feeTotalDisplay').textContent = fmt(totalFee) + ' (' + formatNumber(totalFee) + ' shells)';
            document.getElementById('feeBurnedDisplay').textContent = fmt(burned);
            document.getElementById('feeProducerDisplay').textContent = fmt(producer);
            document.getElementById('feeVotersDisplay').textContent = fmt(voters);
            document.getElementById('feeTreasuryDisplay').textContent = fmt(treasury);
        }
    }

    // Raw data
    document.getElementById('rawData').textContent = JSON.stringify(block, null, 2);
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
    loadBlock();
});
