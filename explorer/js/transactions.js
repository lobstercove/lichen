// MoltChain Explorer – Transactions Page (server-side pagination)
// Uses getRecentTransactions RPC with cursor-based prev/next.

const TXS_PER_PAGE = 50;
let txPolling = null;
let currentPageData = [];  // transactions for current page
let cursorStack = [];      // stack of before_slot cursors for prev navigation
let nextCursor = null;     // cursor for the next page
let currentFilter = { type: '', status: '' };

async function fetchPage(beforeSlot) {
    const params = { limit: TXS_PER_PAGE };
    if (beforeSlot !== undefined && beforeSlot !== null) {
        params.before_slot = beforeSlot;
    }

    const result = await rpc.call('getRecentTransactions', [params]);
    if (!result) return { transactions: [], has_more: false, next_before_slot: null };
    return result;
}

async function loadPage(beforeSlot) {
    const table = document.getElementById('transactionsTable');
    if (!table) return;

    table.innerHTML = '<tr class="loading-row"><td colspan="9"><div class="loading-spinner"></div> Loading transactions...</td></tr>';

    try {
        const page = await fetchPage(beforeSlot);
        currentPageData = page.transactions || [];
        nextCursor = page.next_before_slot || null;

        await renderTransactions();
        updatePaginationUI();
    } catch (error) {
        console.error('Failed to load transactions:', error);
        table.innerHTML = '<tr><td colspan="9" style="text-align:center; color: #FF6B6B;">Failed to load transactions</td></tr>';
    }
}

async function renderTransactions() {
    const table = document.getElementById('transactionsTable');
    if (!table) return;

    let txs = currentPageData;

    // Client-side type filter
    if (currentFilter.type) {
        txs = txs.filter(tx => {
            const instruction = tx.message?.instructions?.[0] || null;
            const type = resolveTxType(tx, instruction).toLowerCase();
            return type === currentFilter.type;
        });
    }

    if (txs.length === 0) {
        table.innerHTML = '<tr><td colspan="9" style="text-align:center; color: var(--text-muted);">No transactions found</td></tr>';
        return;
    }

    const addresses = [];
    txs.forEach(tx => {
        const instruction = tx.message?.instructions?.[0] || null;
        const from = tx.from || instruction?.accounts?.[0] || null;
        const to = tx.to || instruction?.accounts?.[1] || null;
        if (from) addresses.push(from);
        if (to) addresses.push(to);
    });
    const nameMap = typeof batchResolveMoltNames === 'function'
        ? await batchResolveMoltNames(addresses)
        : {};

    table.innerHTML = txs.map(tx => {
        const signature = tx.signature || tx.hash || 'unknown';
        const instruction = tx.message?.instructions?.[0] || null;
        const type = resolveTxType(tx, instruction);
        const from = tx.from || instruction?.accounts?.[0] || 'N/A';
        const to = tx.to || instruction?.accounts?.[1] || 'N/A';
        const amountShells = tx.amount_shells !== undefined
            ? tx.amount_shells
            : (tx.amount !== undefined ? Math.round(tx.amount * 1_000_000_000) : null);
        const amount = amountShells !== null ? formatMolt(amountShells) : '-';
        const feeShells = tx.fee_shells !== undefined
            ? tx.fee_shells
            : (tx.fee !== undefined ? tx.fee : null);
        const fee = feeShells !== null ? formatMolt(feeShells) : '-';
        const slot = tx.slot;
        const timestamp = tx.timestamp;

        const fromDisplay = typeof formatAddressWithMoltName === 'function'
            ? formatAddressWithMoltName(from, nameMap[from])
            : formatAddress(from);
        const toDisplay = typeof formatAddressWithMoltName === 'function'
            ? formatAddressWithMoltName(to, nameMap[to])
            : formatAddress(to);

        return `
        <tr>
            <td>
                <a href="transaction.html?sig=${encodeURIComponent(signature)}" title="${escapeHtml(signature)}">${formatHash(signature)}</a>
                <i class="fas fa-copy copy-hash" data-copy="${escapeHtml(signature)}" onclick="safeCopy(this)" title="Copy signature"></i>
            </td>
            <td><a href="block.html?slot=${slot}">#${formatSlot(slot)}</a></td>
            <td><span class="pill pill-${type.toLowerCase()}">${type}</span></td>
            <td><span class="hash-short">${fromDisplay}</span></td>
            <td><span class="hash-short">${toDisplay}</span></td>
            <td>${amount}</td>
            <td>${fee}</td>
            <td><span class="pill pill-success"><i class="fas fa-check"></i> Success</span></td>
            <td>${formatTime(timestamp)}</td>
        </tr>`;
    }).join('');
}

function updatePaginationUI() {
    const info = document.getElementById('paginationInfo');
    const pageNum = cursorStack.length + 1;
    if (info) info.textContent = `Page ${pageNum}`;

    const prevBtn = document.getElementById('prevPage');
    const nextBtn = document.getElementById('nextPage');
    if (prevBtn) prevBtn.disabled = cursorStack.length === 0;
    if (nextBtn) nextBtn.disabled = !nextCursor;
}

function nextPage() {
    if (!nextCursor) return;
    // Push current cursor so we can go back
    const currentCursor = currentPageData.length > 0
        ? currentPageData[currentPageData.length - 1].slot
        : null;
    cursorStack.push(nextCursor);
    loadPage(nextCursor);
    window.scrollTo({ top: 0, behavior: 'smooth' });
}

function previousPage() {
    if (cursorStack.length === 0) return;
    cursorStack.pop();
    const prevCursor = cursorStack.length > 0 ? cursorStack[cursorStack.length - 1] : undefined;
    loadPage(prevCursor);
    window.scrollTo({ top: 0, behavior: 'smooth' });
}

function applyFilters() {
    currentFilter.type = document.getElementById('typeFilter').value;
    currentFilter.status = document.getElementById('statusFilter').value;
    cursorStack = [];
    nextCursor = null;
    loadPage(undefined);
}

function clearFilters() {
    document.getElementById('typeFilter').value = '';
    document.getElementById('statusFilter').value = '';
    currentFilter = { type: '', status: '' };
    cursorStack = [];
    nextCursor = null;
    loadPage(undefined);
}

// Initialize
document.addEventListener('DOMContentLoaded', () => {
    loadPage(undefined);

    const startPolling = () => {
        if (txPolling) return;
        txPolling = setInterval(() => {
            // Only auto-refresh when on first page
            if (cursorStack.length === 0) loadPage(undefined);
        }, 5000);
    };

    const stopPolling = () => {
        if (txPolling) { clearInterval(txPolling); txPolling = null; }
    };

    if (typeof ws !== 'undefined') {
        ws.onOpen(() => {
            stopPolling();
            ws.subscribe('subscribeBlocks', () => {
                if (cursorStack.length === 0) loadPage(undefined);
            });
        });
        ws.onClose(() => startPolling());
        ws.connect();
        setTimeout(() => { if (!ws.isConnected()) startPolling(); }, 2000);
    } else {
        startPolling();
    }
});
