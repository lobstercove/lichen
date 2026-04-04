// Block Detail Page - Lichen Explorer
// Uses `rpc` instance from explorer.js (loaded before this file)
// NOTE: formatHash, formatAddress, formatNumber, formatBytes, copyToClipboard,
//       escapeHtml, safeCopy, formatTimeShort, formatTimeFull are provided
//       by shared/utils.js (loaded before this file)

// Get block number from URL
function getBlockNumber() {
    const params = new URLSearchParams(window.location.search);
    return params.get('slot') || params.get('block');
}

function isShieldedType(typeRaw) {
    return typeRaw === 'Shield' || typeRaw === 'Unshield' || typeRaw === 'ShieldedTransfer';
}

function bindStaticControls() {
    document.getElementById('copyBlockHashBtn')?.addEventListener('click', function () {
        copyToClipboard((document.getElementById('blockHash')?.dataset?.full) || '');
    });
    document.getElementById('copyStateRootBtn')?.addEventListener('click', function () {
        copyToClipboard((document.getElementById('stateRoot')?.dataset?.full) || '');
    });
    document.getElementById('copyTxRootBtn')?.addEventListener('click', function () {
        copyToClipboard((document.getElementById('txRoot')?.dataset?.full) || '');
    });
    document.getElementById('copyBlockRawDataBtn')?.addEventListener('click', function () {
        copyToClipboard(document.getElementById('rawData')?.textContent || '');
    });
    ['prevBlock', 'nextBlock'].forEach(function (id) {
        document.getElementById(id)?.addEventListener('click', function (event) {
            var href = event.currentTarget?.dataset?.href || '';
            if (!href) return;
            window.location.href = href;
        });
    });
}

function setNavigationHref(buttonId, href) {
    var button = document.getElementById(buttonId);
    if (!button) return;
    if (href) {
        button.disabled = false;
        button.dataset.href = href;
        return;
    }
    button.disabled = true;
    delete button.dataset.href;
}

function redactShieldedBlockForRaw(blockObj) {
    if (!blockObj || typeof blockObj !== 'object') return blockObj;
    const clone = JSON.parse(JSON.stringify(blockObj));
    if (Array.isArray(clone.transactions)) {
        clone.transactions = clone.transactions.map((tx) => {
            const txType = tx?.type || tx?.tx_type || tx?.transaction_type || 'Transfer';
            if (!isShieldedType(txType)) return tx;
            const txClone = { ...tx, from: null, to: null };
            if (txClone.message && Array.isArray(txClone.message.instructions)) {
                txClone.message.instructions = txClone.message.instructions.map((inst) => {
                    const instClone = { ...inst };
                    if (Array.isArray(instClone.accounts) && instClone.accounts.length > 0) {
                        instClone.accounts = [`<redacted:${instClone.accounts.length}>`];
                    }
                    return instClone;
                });
            }
            return txClone;
        });
    }
    return clone;
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
    const txRoot = block.tx_root ?? block.header?.tx_root;
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
    if (txRoot) {
        document.getElementById('txRoot').textContent = formatHash(txRoot);
        document.getElementById('txRoot').dataset.full = txRoot;
    } else {
        document.getElementById('txRoot').textContent = txCount === 0 ? '(empty block)' : '-';
    }
    document.getElementById('detailTimestamp').textContent = formatTimeFull(timestamp);
    const nonShieldedParticipants = transactions
        .filter(tx => !isShieldedType(tx?.type || tx?.tx_type || tx?.transaction_type || 'Transfer'))
        .flatMap(tx => [tx.from, tx.to]);
    const addressNames = typeof batchResolveLichenNames === 'function'
        ? await batchResolveLichenNames([
            validator,
            ...nonShieldedParticipants,
            reward?.recipient
        ])
        : {};

    const validatorDisplay = addressNames[validator] && typeof formatAddressWithLichenName === 'function'
        ? formatAddressWithLichenName(validator, addressNames[validator])
        : formatAddress(validator);
    document.getElementById('validator').innerHTML = validatorDisplay;
    document.getElementById('detailTxCount').textContent = formatNumber(txCount);
    document.getElementById('detailSize').textContent = formatBytes(size);

    // Set links
    if (slot > 0) {
        var previousBlockHref = `block.html?slot=${slot - 1}`;
        document.getElementById('parentLink').href = previousBlockHref;
        setNavigationHref('prevBlock', previousBlockHref);
    } else {
        document.getElementById('parentLink').href = '#';
        setNavigationHref('prevBlock', '');
    }

    setNavigationHref('nextBlock', `block.html?slot=${slot + 1}`);

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
            const typeRaw = tx?.type || tx?.tx_type || tx?.transaction_type || 'Transfer';
            const shieldedTx = isShieldedType(typeRaw);
            const safeSig = typeof escapeHtml === 'function' ? escapeHtml(tx.signature) : tx.signature;
            const safeFrom = typeof escapeHtml === 'function' ? escapeHtml(tx.from) : tx.from;
            const safeTo = typeof escapeHtml === 'function' ? escapeHtml(tx.to) : tx.to;
            const safeType = typeof escapeHtml === 'function' ? escapeHtml(typeRaw) : typeRaw;
            const safeStatus = typeof escapeHtml === 'function' ? escapeHtml(tx.status || 'Success') : (tx.status || 'Success');
            const fromDisplay = addressNames[tx.from] && typeof formatAddressWithLichenName === 'function'
                ? formatAddressWithLichenName(tx.from, addressNames[tx.from])
                : formatAddress(tx.from);
            const toDisplay = addressNames[tx.to] && typeof formatAddressWithLichenName === 'function'
                ? formatAddressWithLichenName(tx.to, addressNames[tx.to])
                : formatAddress(tx.to);
            return `
            <tr>
                <td>
                    <a href="transaction.html?tx=${encodeURIComponent(tx.signature)}" class="hash-link" title="${safeSig}">
                        ${formatHash(tx.signature)}
                    </a>
                </td>
                <td>
                    ${shieldedTx
                    ? '<span class="hash-link" title="Shielded sender redacted">Private</span>'
                    : `<a href="address.html?address=${encodeURIComponent(tx.from)}" class="hash-link">${fromDisplay}</a>`}
                </td>
                <td>
                    ${shieldedTx
                    ? '<span class="hash-link" title="Shielded recipient redacted">Private</span>'
                    : `<a href="address.html?address=${encodeURIComponent(tx.to)}" class="hash-link">${toDisplay}</a>`}
                </td>
                <td><span class="pill pill-${safeType.toLowerCase()}">${safeType}</span></td>
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

    // Epoch reward projection: this is a per-slot estimate only.
    // Actual inflation settles at epoch boundaries across all stakers.
    const rewardCard = document.getElementById('rewardCard');
    if (reward && rewardCard && slot > 0) {
        const projectedPerSlot = reward.projected_per_slot || reward.amount || 0;
        const projectedLicn = reward.projected_per_slot_licn || reward.amount_licn || 0;
        if (projectedPerSlot > 0) {
            rewardCard.style.display = '';
            document.getElementById('rewardAmount').textContent =
                projectedLicn.toFixed(6) + ' LICN/slot estimate';
            const typeLabel = reward.type === 'heartbeat' ? 'Heartbeat slot' : 'Transaction slot';
            const epochLabel = reward.epoch !== undefined ? ' · Epoch ' + reward.epoch : '';
            document.getElementById('rewardType').innerHTML =
                '<span class="badge badge-info">' + typeLabel + '</span>' +
                '<span class="badge badge-secondary" style="margin-left:4px;">settles ' + (reward.distribution || 'epoch') + epochLabel + '</span>';
            const rewardDisplay = addressNames[reward.recipient] && typeof formatAddressWithLichenName === 'function'
                ? formatAddressWithLichenName(reward.recipient, addressNames[reward.recipient])
                : formatAddress(reward.recipient);
            document.getElementById('rewardRecipient').innerHTML = rewardDisplay;
            document.getElementById('rewardRecipientLink').href = 'address.html?address=' + reward.recipient;
        }
    }

    // Fee Distribution (only for blocks with transactions)
    const feeCard = document.getElementById('feeCard');
    if (feeCard && txCount > 0) {
        // Sum up all tx fees in this block
        let totalFee = 0;
        for (const tx of transactions) {
            const fee = tx.fee_spores !== undefined ? tx.fee_spores : (tx.fee || 0);
            totalFee += fee;
        }
        if (totalFee > 0) {
            feeCard.style.display = '';
            const burned = Math.floor(totalFee * FEE_SPLIT.burned);
            const producer = Math.floor(totalFee * FEE_SPLIT.producer);
            const voters = Math.floor(totalFee * FEE_SPLIT.voters);
            const treasury = Math.floor(totalFee * FEE_SPLIT.treasury);
            const community = totalFee - burned - producer - voters - treasury;
            const fmt = (spores) => {
                const licn = spores / SPORES_PER_LICN;
                return licn.toLocaleString(undefined, { minimumFractionDigits: 0, maximumFractionDigits: 9 }) + ' LICN';
            };
            const pct = (v) => Math.round(v * 100) + '%';
            document.getElementById('feeTotalDisplay').textContent = fmt(totalFee) + ' (' + formatNumber(totalFee) + ' spores)';
            document.getElementById('feeBurnedBlockLabel').textContent = 'Fee Burned (' + pct(FEE_SPLIT.burned) + ')';
            document.getElementById('feeProducerBlockLabel').textContent = 'Fee to Producer (' + pct(FEE_SPLIT.producer) + ')';
            document.getElementById('feeVotersBlockLabel').textContent = 'Fee to Voters (' + pct(FEE_SPLIT.voters) + ')';
            document.getElementById('feeTreasuryBlockLabel').textContent = 'Fee to Treasury (' + pct(FEE_SPLIT.treasury) + ')';
            document.getElementById('feeCommunityBlockLabel').textContent = 'Fee to Community (' + pct(FEE_SPLIT.community) + ')';
            document.getElementById('feeBurnedDisplay').textContent = fmt(burned);
            document.getElementById('feeProducerDisplay').textContent = fmt(producer);
            document.getElementById('feeVotersDisplay').textContent = fmt(voters);
            document.getElementById('feeTreasuryDisplay').textContent = fmt(treasury);
            if (document.getElementById('feeCommunityDisplay')) {
                document.getElementById('feeCommunityDisplay').textContent = fmt(community);
            }
        }
    }

    // Raw data
    const rawBlock = redactShieldedBlockForRaw(block);
    document.getElementById('rawData').textContent = JSON.stringify(rawBlock, null, 2);
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
    bindStaticControls();
    loadBlock();
});
