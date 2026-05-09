// Blocks Page Logic

const blocksPerPage = 50;
let currentBlocks = [];
let cursorStack = [];
let nextBeforeSlot = null;
let blocksPolling = null;
let blockRefreshTimer = null;
let hasRenderedBlocks = false;
let isLoadingBlocks = false;
let currentFilter = { fromSlot: null, toSlot: null };

function renderBlockValidator(validator) {
    if (!validator) return 'N/A';
    if (validator === '11111111111111111111111111111111' ||
        validator === '1111111111111111111111111111111111111111') {
        return formatValidator(validator);
    }
    return `<a href="address.html?address=${encodeURIComponent(validator)}" title="${escapeHtml(validator)}">${formatAddress(validator)}</a>`;
}

function bindStaticControls() {
    document.getElementById('blocksApplyFiltersBtn')?.addEventListener('click', applyFilters);
    document.getElementById('blocksClearFiltersBtn')?.addEventListener('click', clearFilters);
    document.getElementById('prevPage')?.addEventListener('click', previousPage);
    document.getElementById('nextPage')?.addEventListener('click', nextPage);
    document.getElementById('blocksTableFull')?.addEventListener('click', (event) => {
        const copyButton = event.target.closest('.copy-hash[data-copy]');
        if (!copyButton) return;
        safeCopy(copyButton);
    });
}

function currentPageCursor() {
    return cursorStack.length > 0 ? cursorStack[cursorStack.length - 1] : undefined;
}

function requestedBeforeSlot(explicitBeforeSlot) {
    if (explicitBeforeSlot !== undefined && explicitBeforeSlot !== null) {
        return explicitBeforeSlot;
    }
    if (currentFilter.toSlot !== null && currentFilter.toSlot !== undefined) {
        return currentFilter.toSlot + 1;
    }
    return undefined;
}

function blockMatchesFilter(block) {
    if (currentFilter.fromSlot !== null && block.slot < currentFilter.fromSlot) return false;
    if (currentFilter.toSlot !== null && block.slot > currentFilter.toSlot) return false;
    return true;
}

async function fetchBlocksPage(beforeSlot) {
    const params = { limit: blocksPerPage };
    const requestedBefore = requestedBeforeSlot(beforeSlot);
    if (requestedBefore !== undefined && requestedBefore !== null) {
        params.before_slot = requestedBefore;
    }

    if (typeof rpc.getRecentBlocks === 'function') {
        return rpc.getRecentBlocks(params);
    }
    return rpc.call('getRecentBlocks', [params]);
}

async function loadBlocksPage(beforeSlot, options = {}) {
    const table = document.getElementById('blocksTableFull');
    if (!table) return;

    if (isLoadingBlocks) return;
    isLoadingBlocks = true;

    if (options.showSpinner || !hasRenderedBlocks) {
        table.innerHTML = '<tr class="loading-row"><td colspan="6"><div class="loading-spinner"></div> Loading blocks...</td></tr>';
    }

    try {
        const result = await fetchBlocksPage(beforeSlot);
        const blocks = Array.isArray(result?.blocks) ? result.blocks : [];
        currentBlocks = blocks.filter(blockMatchesFilter);

        nextBeforeSlot = result?.has_more ? result.next_before_slot ?? null : null;
        if (
            currentFilter.fromSlot !== null &&
            nextBeforeSlot !== null &&
            nextBeforeSlot <= currentFilter.fromSlot
        ) {
            nextBeforeSlot = null;
        }

        renderBlocks();
        hasRenderedBlocks = true;
    } catch (error) {
        console.error('Failed to load blocks:', error);
        table.innerHTML = '<tr><td colspan="6" style="text-align:center; color: #FF6B6B;">Failed to load blocks</td></tr>';
    } finally {
        isLoadingBlocks = false;
    }
}

function renderBlocks() {
    const table = document.getElementById('blocksTableFull');
    if (!table) return;

    if (currentBlocks.length === 0) {
        table.innerHTML = '<tr><td colspan="6" style="text-align:center; color: var(--text-muted);">No blocks found</td></tr>';
        updatePagination();
        return;
    }

    table.innerHTML = currentBlocks.map(block => `
        <tr>
            <td><a href="block.html?slot=${block.slot}">#${formatSlot(block.slot)}</a></td>
            <td>
                <span class="hash-short" title="${escapeHtml(block.hash)}">${formatHash(block.hash)}</span>
                <i class="fas fa-copy copy-hash" data-copy="${escapeHtml(block.hash)}" title="Copy hash"></i>
            </td>
            <td>
                <span class="hash-short" title="${escapeHtml(block.parent_hash)}">${formatHash(block.parent_hash)}</span>
            </td>
            <td><span class="pill pill-info">${block.transaction_count || 0} txs</span></td>
            <td>${renderBlockValidator(block.validator)}</td>
            <td>${formatTime(block.timestamp)}</td>
        </tr>
    `).join('');

    updatePagination();
}

function updatePagination() {
    const info = document.getElementById('paginationInfo');
    const prev = document.getElementById('prevPage');
    const next = document.getElementById('nextPage');

    if (info) info.textContent = `Page ${cursorStack.length + 1}`;
    if (prev) prev.disabled = cursorStack.length === 0 || isLoadingBlocks;
    if (next) next.disabled = nextBeforeSlot === null || isLoadingBlocks;
}

function nextPage() {
    if (nextBeforeSlot === null || isLoadingBlocks) return;
    cursorStack.push(nextBeforeSlot);
    loadBlocksPage(nextBeforeSlot, { showSpinner: true });
    window.scrollTo({ top: 0, behavior: 'smooth' });
}

function previousPage() {
    if (cursorStack.length === 0 || isLoadingBlocks) return;
    cursorStack.pop();
    loadBlocksPage(currentPageCursor(), { showSpinner: true });
    window.scrollTo({ top: 0, behavior: 'smooth' });
}

function setSlotFilterError(message) {
    const errorEl = document.getElementById('slotFilterError');
    if (!errorEl) return;
    if (!message) {
        errorEl.style.display = 'none';
        errorEl.innerHTML = '';
        return;
    }
    errorEl.style.display = 'block';
    errorEl.innerHTML = `<span class="pill pill-error"><i class="fas fa-exclamation-triangle"></i> ${escapeHtml(message)}</span>`;
}

function clearSlotFilterValidation() {
    const fromInput = document.getElementById('slotFromFilter');
    const toInput = document.getElementById('slotToFilter');
    [fromInput, toInput].forEach((input) => {
        if (!input) return;
        input.setCustomValidity('');
        input.setAttribute('aria-invalid', 'false');
    });
    setSlotFilterError('');
}

function parseSlotInput(input, label) {
    const raw = String(input?.value || '').trim();
    if (!raw) return { ok: true, value: null };
    if (!/^\d+$/.test(raw)) {
        return { ok: false, message: `${label} must be a non-negative integer slot.` };
    }
    const value = Number(raw);
    if (!Number.isSafeInteger(value)) {
        return { ok: false, message: `${label} is too large.` };
    }
    return { ok: true, value };
}

function applyFilters() {
    const fromInput = document.getElementById('slotFromFilter');
    const toInput = document.getElementById('slotToFilter');
    if (!fromInput || !toInput) return;

    clearSlotFilterValidation();

    const fromParsed = parseSlotInput(fromInput, 'From slot');
    if (!fromParsed.ok) {
        fromInput.setCustomValidity(fromParsed.message);
        fromInput.setAttribute('aria-invalid', 'true');
        fromInput.reportValidity();
        setSlotFilterError(fromParsed.message);
        if (typeof showToast === 'function') showToast(fromParsed.message);
        return;
    }

    const toParsed = parseSlotInput(toInput, 'To slot');
    if (!toParsed.ok) {
        toInput.setCustomValidity(toParsed.message);
        toInput.setAttribute('aria-invalid', 'true');
        toInput.reportValidity();
        setSlotFilterError(toParsed.message);
        if (typeof showToast === 'function') showToast(toParsed.message);
        return;
    }

    const fromSlot = fromParsed.value;
    const toSlot = toParsed.value;
    if (fromSlot !== null && toSlot !== null && fromSlot > toSlot) {
        const message = 'From slot must be less than or equal to To slot.';
        fromInput.setCustomValidity(message);
        toInput.setCustomValidity(message);
        fromInput.setAttribute('aria-invalid', 'true');
        toInput.setAttribute('aria-invalid', 'true');
        fromInput.reportValidity();
        setSlotFilterError(message);
        if (typeof showToast === 'function') showToast(message);
        return;
    }

    currentFilter = { fromSlot, toSlot };
    cursorStack = [];
    nextBeforeSlot = null;
    loadBlocksPage(undefined, { showSpinner: true });
}

function clearFilters() {
    document.getElementById('slotFromFilter').value = '';
    document.getElementById('slotToFilter').value = '';
    clearSlotFilterValidation();
    currentFilter = { fromSlot: null, toSlot: null };
    cursorStack = [];
    nextBeforeSlot = null;
    loadBlocksPage(undefined, { showSpinner: true });
}

function scheduleBlocksRefresh(delayMs = 500) {
    if (blockRefreshTimer) return;
    blockRefreshTimer = setTimeout(() => {
        blockRefreshTimer = null;
        if (cursorStack.length === 0) {
            loadBlocksPage(undefined);
        }
    }, delayMs);
}

// Initialize
document.addEventListener('DOMContentLoaded', () => {
    bindStaticControls();
    loadBlocksPage(undefined, { showSpinner: true });

    const startPolling = () => {
        if (blocksPolling) return;
        blocksPolling = setInterval(() => {
            if (cursorStack.length === 0) {
                loadBlocksPage(undefined);
            }
        }, 5000);
    };

    const stopPolling = () => {
        if (blocksPolling) {
            clearInterval(blocksPolling);
            blocksPolling = null;
        }
    };

    if (typeof ws !== 'undefined') {
        ws.onOpen(() => {
            stopPolling();
            ws.subscribe('subscribeBlocks', () => scheduleBlocksRefresh());
        });

        ws.onClose(() => {
            startPolling();
        });

        ws.connect();
        setTimeout(() => {
            if (!ws.isConnected()) {
                startPolling();
            }
        }, 2000);
    } else {
        startPolling();
    }
});
