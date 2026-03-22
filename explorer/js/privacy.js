// Privacy Layer Page - Molt Explorer
// Displays shielded pool statistics, ZK transaction history, and nullifier lookup
// Uses `rpc` instance from explorer.js

// ===== State =====
let shieldedTxs = [];
let poolStats = null;

// ===== Initialization =====
document.addEventListener('DOMContentLoaded', () => {
    loadPrivacyData();
    // Refresh every 10 seconds
    setInterval(loadPrivacyData, 10000);

    // EX-14: Delegated click handler for copy buttons — no inline onclick (XSS safe)
    document.addEventListener('click', (e) => {
        const btn = e.target.closest('.copy-hash-btn');
        if (btn && btn.dataset.hash) {
            copyToClipboard(btn.dataset.hash);
        }
    });
});

// ===== Data Loading =====

async function loadPrivacyData() {
    try {
        await Promise.all([
            loadPoolStats(),
            loadShieldedTransactions(),
        ]);
    } catch (err) {
        console.error('Failed to load privacy data:', err);
    }
}

async function loadPoolStats() {
    if (!rpc) return;

    // Current shielded RPC endpoint
    const stats = await rpc.call('getShieldedPoolState');

    if (stats) {
        const vkInitialized = Boolean(
            (stats.vkShieldHash && !/^0+$/.test(stats.vkShieldHash)) &&
            (stats.vkUnshieldHash && !/^0+$/.test(stats.vkUnshieldHash)) &&
            (stats.vkTransferHash && !/^0+$/.test(stats.vkTransferHash))
        );

        poolStats = {
            ...stats,
            vk_initialized: vkInitialized,
        };
        updatePoolStatsUI(poolStats);
    } else {
        // Endpoint not available yet — show defaults
        updatePoolStatsUI({
            merkleRoot: '0'.repeat(64),
            commitmentCount: 0,
            totalShielded: 0,
            pool_balance_molt: 0,
            nullifierCount: 0,
            nullifier_count: 0,
            shieldCount: 0,
            unshieldCount: 0,
            transferCount: 0,
            vk_initialized: false,
            shield_count: 0,
            unshield_count: 0,
            transfer_count: 0,
        });
    }
}

async function loadShieldedTransactions() {
    if (!rpc) return;

    const resp = await rpc.call('getShieldedCommitments', [{ from: 0, limit: 50 }]);

    const txs = (resp && Array.isArray(resp.commitments) ? resp.commitments : []).map((entry) => ({
        type: 'shield',
        commitment: entry.commitment,
        amount: null,
        slot: '-',
        timestamp: null,
        proof_valid: true,
    }));

    if (txs.length > 0) {
        shieldedTxs = txs;
        renderShieldedTxs(txs);
    } else {
        renderShieldedTxs([]);
    }
}

// ===== UI Updates =====

function updatePoolStatsUI(stats) {
    const pick = (...vals) => vals.find(v => v !== undefined && v !== null);

    // Shielded balance
    const totalShielded = pick(stats.totalShielded, stats.pool_balance, 0);
    const balanceMolt = pick(
        stats.totalShieldedMolt,
        stats.pool_balance_molt,
        (totalShielded / SHELLS_PER_MOLT)
    );
    const el = (id) => document.getElementById(id);

    const shieldedBalanceEl = el('shieldedBalance');
    const shieldedShellsEl = el('shieldedBalanceShells');
    if (shieldedBalanceEl) {
        const balanceShells = Math.round(Number(balanceMolt || 0) * SHELLS_PER_MOLT);
        shieldedBalanceEl.textContent = formatMolt(balanceShells);
    }
    if (shieldedShellsEl) shieldedShellsEl.textContent = formatNumber(totalShielded) + ' shells';

    // Commitment count
    const commitmentCount = pick(stats.commitmentCount, stats.commitment_count, 0);
    const commitmentCountEl = el('commitmentCount');
    if (commitmentCountEl) commitmentCountEl.textContent = formatNumber(commitmentCount);

    // Nullifier count
    const nullifierCountEl = el('nullifierCount');
    if (nullifierCountEl) {
        nullifierCountEl.textContent = formatNumber(
            pick(stats.nullifierCount, stats.nullifier_count, 0)
        );
    }

    // Shielded tx count
    const shieldCount = pick(stats.shieldCount, stats.shield_count, 0);
    const unshieldCount = pick(stats.unshieldCount, stats.unshield_count, 0);
    const transferCount = pick(stats.transferCount, stats.transfer_count, 0);
    const totalTxs = shieldCount + unshieldCount + transferCount || commitmentCount;
    const txCountEl = el('shieldedTxCount');
    const txBreakdownEl = el('shieldedTxBreakdown');
    if (txCountEl) txCountEl.textContent = formatNumber(totalTxs);
    if (txBreakdownEl) txBreakdownEl.textContent =
        `Shield: ${formatNumber(shieldCount)} | ` +
        `Unshield: ${formatNumber(unshieldCount)} | ` +
        `Transfer: ${formatNumber(transferCount)}`;

    // Merkle root
    const merkleRoot = pick(stats.merkleRoot, stats.merkle_root, '0'.repeat(64));
    const merkleRootEl = el('merkleRoot');
    if (merkleRootEl) merkleRootEl.textContent = '0x' + merkleRoot;

    // Tree utilization — TREE_DEPTH=20 → 2^20 = 1,048,576 max commitments
    const maxCapacity = 1_048_576; // 2^20
    const utilPct = (commitmentCount / maxCapacity) * 100;
    const utilizationBarEl = el('treeUtilizationBar');
    const utilizationPctEl = el('treeUtilizationPct');
    if (utilizationBarEl) utilizationBarEl.style.width = Math.max(utilPct, 0.1) + '%';
    if (utilizationPctEl) utilizationPctEl.textContent = utilPct < 0.01 ? '<0.01%' : utilPct.toFixed(4) + '%';

    // VK status
    const vkText = el('vkStatusText');
    if (!vkText) return;
    const ZERO_HASH = '0'.repeat(64);
    const vkLoaded = stats.vk_shield_hash && stats.vk_shield_hash !== ZERO_HASH
        && stats.vk_unshield_hash && stats.vk_unshield_hash !== ZERO_HASH
        && stats.vk_transfer_hash && stats.vk_transfer_hash !== ZERO_HASH;
    if (vkLoaded) {
        vkText.textContent = 'Initialized';
        vkText.style.background = 'rgba(6, 214, 160, 0.2)';
        vkText.style.color = '#06d6a0';
    } else {
        vkText.textContent = 'Pending Setup';
        vkText.style.background = 'rgba(255, 210, 63, 0.2)';
        vkText.style.color = '#ffd23f';
    }
}

function renderShieldedTxs(txs) {
    const tbody = document.getElementById('shieldedTxsBody');
    const empty = document.getElementById('shieldedTxsEmpty');

    if (!txs || txs.length === 0) {
        tbody.innerHTML = '';
        empty.style.display = 'block';
        return;
    }

    empty.style.display = 'none';
    tbody.innerHTML = txs.map(tx => {
        const typeInfo = getShieldedTxType(tx.type || tx.tx_type);
        const hashDisplay = tx.commitment || tx.nullifier || tx.signature || '';
        const truncated = hashDisplay.length > 16
            ? hashDisplay.slice(0, 8) + '...' + hashDisplay.slice(-8)
            : hashDisplay;

        const amountDisplay = tx.amount != null
            ? formatMolt(tx.amount)
            : '<span style="color: var(--text-muted);">Hidden</span>';

        const proofStatus = tx.proof_valid !== false
            ? '<span style="color: #06d6a0;"><i class="fas fa-check-circle"></i> Valid</span>'
            : '<span style="color: #f43f5e;"><i class="fas fa-times-circle"></i> Invalid</span>';

        return `
            <tr>
                <td>
                    <span class="badge" style="background: ${typeInfo.bg}; color: ${typeInfo.color};">
                        <i class="${typeInfo.icon}"></i> ${typeInfo.label}
                    </span>
                </td>
                <td style="font-family: 'JetBrains Mono', monospace; font-size: 0.8rem;">
                    <span title="${escapeHtml(hashDisplay)}" style="cursor: pointer;" class="copy-hash-btn" data-hash="${escapeHtml(hashDisplay)}">
                        ${escapeHtml(truncated)} <i class="fas fa-copy" style="font-size: 0.7rem; opacity: 0.5;"></i>
                    </span>
                </td>
                <td>${proofStatus}</td>
                <td>${amountDisplay}</td>
                <td>${tx.slot || tx.block || '-'}</td>
                <td style="color: var(--text-muted); font-size: 0.85rem;">${tx.timestamp ? formatTimeFull(tx.timestamp) : '-'}</td>
            </tr>
        `;
    }).join('');
}

function getShieldedTxType(type) {
    switch (type) {
        case 'shield':
            return {
                label: 'Shield',
                icon: 'fas fa-arrow-down',
                bg: 'rgba(6, 214, 160, 0.2)',
                color: '#06d6a0',
            };
        case 'unshield':
            return {
                label: 'Unshield',
                icon: 'fas fa-arrow-up',
                bg: 'rgba(245, 158, 11, 0.2)',
                color: '#f59e0b',
            };
        case 'transfer':
            return {
                label: 'Transfer',
                icon: 'fas fa-exchange-alt',
                bg: 'rgba(168, 85, 247, 0.2)',
                color: '#c084fc',
            };
        default:
            return {
                label: 'Unknown',
                icon: 'fas fa-question',
                bg: 'rgba(107, 122, 153, 0.2)',
                color: '#6b7a99',
            };
    }
}

// ===== Tab Switching =====

function switchPrivacyTab(tabName) {
    // Update tab buttons
    document.querySelectorAll('.address-tab').forEach(tab => {
        tab.classList.toggle('active', tab.dataset.tab === tabName);
    });

    // Show/hide tab panes
    document.querySelectorAll('.address-pane').forEach(pane => {
        pane.classList.toggle('active', pane.dataset.pane === tabName);
    });
}

// ===== Nullifier Lookup =====

async function lookupNullifier() {
    const input = document.getElementById('nullifierInput');
    const resultDiv = document.getElementById('nullifierResult');
    const hash = input.value.trim().replace(/^0x/, '');

    if (!hash || hash.length !== 64 || !/^[a-fA-F0-9]+$/.test(hash)) {
        resultDiv.style.display = 'block';
        resultDiv.innerHTML = `
            <div style="padding: 1rem; background: rgba(244, 63, 94, 0.1); border-radius: 8px; border: 1px solid rgba(244, 63, 94, 0.3);">
                <i class="fas fa-exclamation-triangle" style="color: #f43f5e;"></i>
                <strong>Invalid format.</strong> Enter exactly 64 hex characters (32 bytes).
            </div>
        `;
        return;
    }

    resultDiv.style.display = 'block';
    resultDiv.innerHTML = '<div style="padding: 1rem; color: var(--text-muted);"><i class="fas fa-spinner fa-spin"></i> Checking...</div>';

    if (!rpc) {
        resultDiv.innerHTML = `
            <div style="padding: 1rem; background: rgba(244, 63, 94, 0.1); border-radius: 8px;">
                <i class="fas fa-exclamation-circle" style="color: #f43f5e;"></i> RPC not connected
            </div>
        `;
        return;
    }

    const result = await rpc.call('isNullifierSpent', [hash]);

    if (result && result.spent) {
        resultDiv.innerHTML = `
            <div style="padding: 1.25rem; background: rgba(244, 63, 94, 0.1); border-radius: 8px; border: 1px solid rgba(244, 63, 94, 0.2);">
                <div style="display: flex; align-items: center; gap: 0.5rem; margin-bottom: 0.75rem;">
                    <i class="fas fa-ban" style="color: #f43f5e; font-size: 1.25rem;"></i>
                    <strong style="color: #f43f5e;">SPENT</strong>
                </div>
                <p style="margin: 0; color: var(--text-secondary); font-size: 0.9rem;">
                    This nullifier has been recorded on-chain. The associated note has already been consumed
                    and cannot be spent again.
                </p>
                ${result.spent_at_slot ? `<p style="margin: 0.5rem 0 0; font-size: 0.85rem; color: var(--text-muted);">Spent at slot: ${result.spent_at_slot}</p>` : ''}
            </div>
        `;
    } else {
        resultDiv.innerHTML = `
            <div style="padding: 1.25rem; background: rgba(6, 214, 160, 0.1); border-radius: 8px; border: 1px solid rgba(6, 214, 160, 0.2);">
                <div style="display: flex; align-items: center; gap: 0.5rem; margin-bottom: 0.75rem;">
                    <i class="fas fa-check-circle" style="color: #06d6a0; font-size: 1.25rem;"></i>
                    <strong style="color: #06d6a0;">UNSPENT</strong>
                </div>
                <p style="margin: 0; color: var(--text-secondary); font-size: 0.9rem;">
                    This nullifier has not been recorded. The associated note (if it exists) has not been spent.
                </p>
            </div>
        `;
    }
}

// ===== Refresh =====

function refreshShieldedTxs() {
    loadShieldedTransactions();
}
