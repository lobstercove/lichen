// Privacy Layer Page - Lichen Explorer
// Displays shielded pool statistics, ZK transaction history, and nullifier lookup
// Uses `rpc` instance from explorer.js

// ===== State =====
let shieldedTxs = [];
let poolStats = null;
const SHIELDED_TX_TARGET = 50;
const SHIELDED_TX_RPC_LIMIT = 100;
const SHIELDED_TX_MAX_PAGES = 10;

function isShieldedType(typeRaw) {
    return typeRaw === 'Shield' || typeRaw === 'Unshield' || typeRaw === 'ShieldedTransfer';
}

function formatShieldedTypeLabel(typeRaw) {
    if (typeRaw === 'ShieldedTransfer') return 'Shielded Transfer';
    return typeRaw || 'Unknown';
}

function bindStaticControls() {
    document.querySelectorAll('.address-tab[data-tab]').forEach((tab) => {
        tab.addEventListener('click', () => {
            switchPrivacyTab(tab.dataset.tab);
        });
    });

    document.getElementById('refreshShieldedTxsBtn')?.addEventListener('click', refreshShieldedTxs);
    document.getElementById('lookupNullifierBtn')?.addEventListener('click', lookupNullifier);
    document.getElementById('nullifierInput')?.addEventListener('keydown', (event) => {
        if (event.key !== 'Enter') return;
        event.preventDefault();
        lookupNullifier();
    });

    document.addEventListener('click', (event) => {
        const copyButton = event.target.closest('.copy-hash[data-copy]');
        if (!copyButton) return;
        safeCopy(copyButton);
    });
}

// ===== Initialization =====
document.addEventListener('DOMContentLoaded', () => {
    bindStaticControls();
    loadPrivacyData();
    // Refresh every 10 seconds
    setInterval(loadPrivacyData, 10000);
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
        poolStats = {
            ...stats,
        };
        updatePoolStatsUI(poolStats);
    } else {
        // Endpoint not available yet — show defaults
        updatePoolStatsUI({
            merkleRoot: '0'.repeat(64),
            commitmentCount: 0,
            totalShielded: 0,
            pool_balance_licn: 0,
            nullifierCount: 0,
            nullifier_count: 0,
            shieldCount: 0,
            unshieldCount: 0,
            transferCount: 0,
            shield_count: 0,
            unshield_count: 0,
            transfer_count: 0,
            zkScheme: 'plonky3-fri-poseidon2',
            zk_scheme: 'plonky3-fri-poseidon2',
        });
    }
}

async function loadShieldedTransactions() {
    if (!rpc) return;

    const txs = [];
    let beforeSlot = null;
    let pageCount = 0;

    while (txs.length < SHIELDED_TX_TARGET && pageCount < SHIELDED_TX_MAX_PAGES) {
        const params = { limit: SHIELDED_TX_RPC_LIMIT };
        if (beforeSlot !== null) {
            params.before_slot = beforeSlot;
        }

        const resp = await rpc.call('getRecentTransactions', [params]);
        const recent = Array.isArray(resp?.transactions) ? resp.transactions : [];

        for (const tx of recent) {
            const instruction = tx.message?.instructions?.[0] || null;
            const type = resolveTxType(tx, instruction);
            if (!isShieldedType(type)) continue;
            txs.push(tx);
            if (txs.length >= SHIELDED_TX_TARGET) break;
        }

        pageCount += 1;
        if (!resp?.has_more || !resp?.next_before_slot) break;
        beforeSlot = resp.next_before_slot;
    }

    shieldedTxs = txs;
    renderShieldedTxs(txs);
}

// ===== UI Updates =====

function updatePoolStatsUI(stats) {
    const pick = (...vals) => vals.find(v => v !== undefined && v !== null);

    // Shielded balance
    const totalShielded = pick(stats.totalShielded, stats.pool_balance, 0);
    const balanceLicn = pick(
        stats.totalShieldedLicn,
        stats.pool_balance_licn,
        (totalShielded / SPORES_PER_LICN)
    );
    const el = (id) => document.getElementById(id);

    const shieldedBalanceEl = el('shieldedBalance');
    const shieldedSporesEl = el('shieldedBalanceSpores');
    if (shieldedBalanceEl) {
        const balanceSpores = Math.round(Number(balanceLicn || 0) * SPORES_PER_LICN);
        shieldedBalanceEl.textContent = formatLicn(balanceSpores);
    }
    if (shieldedSporesEl) shieldedSporesEl.textContent = formatNumber(totalShielded) + ' spores';

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

    // Proof runtime status
    const vkText = el('vkStatusText');
    if (!vkText) return;
    vkText.textContent = 'Transparent STARK path';
    vkText.style.background = 'rgba(6, 214, 160, 0.2)';
    vkText.style.color = '#06d6a0';
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
        const signature = tx.signature || tx.hash || 'unknown';
        const instruction = tx.message?.instructions?.[0] || null;
        const type = resolveTxType(tx, instruction);
        const amountSpores = tx.amount_spores !== undefined
            ? tx.amount_spores
            : (tx.amount !== undefined ? Math.round(tx.amount * SPORES_PER_LICN) : null);
        const amount = tx.token_symbol
            ? formatNumber(tx.token_amount || 0) + ' ' + tx.token_symbol
            : (amountSpores !== null ? formatLicn(amountSpores) : '-');
        const feeSpores = tx.fee_spores !== undefined
            ? tx.fee_spores
            : (tx.fee !== undefined ? tx.fee : null);
        const fee = feeSpores !== null ? formatLicn(feeSpores) : '-';
        const slot = tx.slot;
        const timestamp = tx.timestamp;
        const statusRaw = tx.status || (tx.success === false ? 'Error' : 'Success');
        const isError = tx.success === false || String(statusRaw).toLowerCase().includes('fail');
        const statusLabel = isError ? 'Error' : 'Success';
        const statusIcon = isError ? 'times' : 'check';
        const statusClass = isError ? 'error' : 'success';
        const blockCell = slot !== undefined && slot !== null
            ? `<a href="block.html?slot=${slot}">#${formatSlot(slot)}</a>`
            : '<span class="hash-short">-</span>';

        return `
            <tr>
                <td>${blockCell}</td>
                <td>
                    <a href="transaction.html?sig=${encodeURIComponent(signature)}" title="${escapeHtml(signature)}">${formatHash(signature)}</a>
                    <i class="fas fa-copy copy-hash" data-copy="${escapeHtml(signature)}" title="Copy signature"></i>
                </td>
                <td><span class="pill pill-${type.toLowerCase()}">${formatShieldedTypeLabel(type)}</span></td>
                <td><span class="hash-short">Shielded Note(s) (private)</span></td>
                <td><span class="hash-short">Shielded Note(s) (private)</span></td>
                <td>${amount}</td>
                <td>${fee}</td>
                <td><span class="pill pill-${statusClass}" title="${escapeHtml(String(statusRaw))}"><i class="fas fa-${statusIcon}"></i> ${statusLabel}</span></td>
                <td>${timestamp ? formatTime(timestamp) : '-'}</td>
            </tr>
        `;
    }).join('');
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
