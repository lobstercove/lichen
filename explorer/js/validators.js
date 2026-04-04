// Validators Page Logic
// Uses RPC_URL and LichenRPC class from explorer.js (loaded first)
// NOTE: formatNumber, formatLicn, formatHash, copyToClipboard, escapeHtml,
//       safeCopy, getTrustTier are provided by shared/utils.js (loaded before this file)

// Validator consensus reputation uses a 0-1000 scale (separate from LichenID 0-100000)
const VALIDATOR_MAX_REPUTATION = 1000;
const VALIDATORS_PER_PAGE = 25;
const VALIDATORS_MIN_REFRESH_MS = 3000;
let validatorsLoadInFlight = false;
let allValidators = [];
let validatorNameMap = {};
let validatorCurrentSlot = 0;
let validatorTotalStake = 0;
let currentPage = 1;
let lastValidatorsRefreshAt = 0;
let validatorsRefreshTimer = null;

function trustTierFromReputation(score) {
    return getTrustTier(score).label;
}

function bindStaticControls() {
    document.getElementById('prevPage')?.addEventListener('click', previousPage);
    document.getElementById('nextPage')?.addEventListener('click', nextPage);
    document.getElementById('validatorsTable')?.addEventListener('click', (event) => {
        const copyButton = event.target.closest('.copy-hash[data-copy]');
        if (!copyButton) return;
        safeCopy(copyButton);
    });
}

async function loadValidators() {
    const table = document.getElementById('validatorsTable');
    if (!table) return;
    if (validatorsLoadInFlight) return;
    validatorsLoadInFlight = true;

    try {
        if (typeof rpc === 'undefined') {
            table.innerHTML = '<tr><td colspan="8" style="text-align:center; color: #FF6B6B;">RPC client not available</td></tr>';
            updatePagination(0);
            return;
        }

        const validatorsResult = await rpc.getValidators();
        const validators = validatorsResult && validatorsResult.validators ? validatorsResult.validators : validatorsResult;
        const validatorCount = validatorsResult && validatorsResult.count !== undefined
            ? validatorsResult.count
            : (validators ? validators.length : 0);

        if (!validators || validators.length === 0) {
            allValidators = [];
            validatorNameMap = {};
            table.innerHTML = '<tr><td colspan="8" style="text-align:center; color: var(--text-muted);">No validators found</td></tr>';
            updatePagination(0);
            return;
        }

        validatorCurrentSlot = await rpc.getSlot();

        // Update stats
        document.getElementById('totalValidators').textContent = validatorCount;

        validatorTotalStake = validators.reduce((sum, v) => sum + (v.stake || 0), 0);
        document.getElementById('totalStake').textContent = formatLicn(validatorTotalStake);

        validatorNameMap = typeof batchResolveLichenNames === 'function'
            ? await batchResolveLichenNames(validators.map(v => v.pubkey).filter(Boolean))
            : {};

        allValidators = validators;
        const totalPages = Math.max(1, Math.ceil(allValidators.length / VALIDATORS_PER_PAGE));
        if (currentPage > totalPages) currentPage = totalPages;
        renderValidators();

    } catch (error) {
        console.error('Failed to load validators:', error);
        table.innerHTML = '<tr><td colspan="8" style="text-align:center; color: #FF6B6B;">Failed to load validators (RPC error)</td></tr>';
        updatePagination(0);
    } finally {
        validatorsLoadInFlight = false;
    }
}

function renderValidators() {
    const table = document.getElementById('validatorsTable');
    if (!table) return;
    if (!allValidators.length) {
        table.innerHTML = '<tr><td colspan="8" style="text-align:center; color: var(--text-muted);">No validators found</td></tr>';
        updatePagination(0);
        return;
    }

    const start = (currentPage - 1) * VALIDATORS_PER_PAGE;
    const pageValidators = allValidators.slice(start, start + VALIDATORS_PER_PAGE);

    table.innerHTML = pageValidators.map((validator, index) => {
        const validatorPubkey = validator.pubkey || validator.validator || validator.address || '';
        const stake = validator.stake || 0;
        const reputation = validator.reputation || 0;
        const blocksProduced = validator.blocks_proposed || validator.blocks_produced || 0;
        const txsProcessed = validator.transactions_processed || 0;
        const votingPower = validatorTotalStake > 0 ? ((stake / validatorTotalStake) * 100).toFixed(2) : '0.00';
        const lastActiveSlot = validator.last_active_slot || validator.lastActiveSlot || 0;
        const isOnline = validatorCurrentSlot - lastActiveSlot <= 100;
        const reputationScale = (reputation / VALIDATOR_MAX_REPUTATION) * 100;
        const lichenName = validatorNameMap[validatorPubkey] || null;
        const tier = trustTierFromReputation(reputation);
        const addressLabel = escapeHtml(lichenName
            ? `${lichenName}.lichen`
            : formatHash(validatorPubkey));
        const addressLink = validatorPubkey
            ? `<a href="address.html?address=${encodeURIComponent(validatorPubkey)}" class="hash-short" title="${escapeHtml(validatorPubkey)}">${addressLabel}</a>`
            : `<span class="hash-short">${addressLabel}</span>`;
        const copyIcon = validatorPubkey
            ? `<i class="fas fa-copy copy-hash" data-copy="${escapeHtml(validatorPubkey)}" title="Copy address"></i>`
            : '';

        return `
            <tr>
                <td><span style="font-weight: 700; color: var(--text-muted);">${start + index + 1}</span></td>
                <td>
                        ${addressLink}
                    ${copyIcon}
                </td>
                <td><span style="font-family: 'JetBrains Mono', monospace; font-weight: 600;">${formatLicn(stake)}</span></td>
                <td>
                    <div style="display: flex; align-items: center; gap: 0.5rem;">
                        <div style="flex: 1; background: var(--bg-darker); height: 6px; border-radius: 3px; overflow: hidden;">
                            <div style="background: var(--primary); height: 100%; width: ${Math.min(reputationScale, 100)}%;"></div>
                        </div>
                        <span style="font-family: 'JetBrains Mono', monospace; font-size: 0.85rem;">${Number(reputation).toFixed(4)}</span>
                        <span class="pill pill-info">${tier}</span>
                    </div>
                </td>
                <td><span class="pill pill-info">${formatNumber(blocksProduced)}</span></td>
                <td><span class="pill pill-info">${formatNumber(txsProcessed)}</span></td>
                <td>
                    <div style="display: flex; align-items: center; gap: 0.5rem;">
                        <div style="flex: 1; background: var(--bg-darker); height: 6px; border-radius: 3px; overflow: hidden;">
                            <div style="background: var(--success); height: 100%; width: ${votingPower}%;"></div>
                        </div>
                        <span style="font-family: 'JetBrains Mono', monospace; font-size: 0.85rem;">${votingPower}%</span>
                    </div>
                </td>
                <td><span class="pill ${isOnline ? 'pill-success' : 'pill-error'}"><i class="fas fa-circle"></i> ${isOnline ? 'Online' : 'Offline'}</span></td>
            </tr>
        `}).join('');

    updatePagination(allValidators.length);
}

function updatePagination(totalItems) {
    const totalPages = Math.max(1, Math.ceil(totalItems / VALIDATORS_PER_PAGE));
    const info = document.getElementById('paginationInfo');
    if (info) info.textContent = `Page ${currentPage} of ${totalPages}`;

    const prevBtn = document.getElementById('prevPage');
    const nextBtn = document.getElementById('nextPage');
    if (prevBtn) prevBtn.disabled = currentPage <= 1 || totalItems === 0;
    if (nextBtn) nextBtn.disabled = currentPage >= totalPages || totalItems === 0;
}

function nextPage() {
    const totalPages = Math.max(1, Math.ceil(allValidators.length / VALIDATORS_PER_PAGE));
    if (currentPage >= totalPages) return;
    currentPage += 1;
    renderValidators();
    window.scrollTo({ top: 0, behavior: 'smooth' });
}

function previousPage() {
    if (currentPage <= 1) return;
    currentPage -= 1;
    renderValidators();
    window.scrollTo({ top: 0, behavior: 'smooth' });
}

function scheduleValidatorsRefresh() {
    const now = Date.now();
    const elapsed = now - lastValidatorsRefreshAt;
    if (elapsed >= VALIDATORS_MIN_REFRESH_MS) {
        lastValidatorsRefreshAt = now;
        loadValidators();
        return;
    }

    const waitMs = VALIDATORS_MIN_REFRESH_MS - elapsed;
    if (validatorsRefreshTimer) return;
    validatorsRefreshTimer = setTimeout(() => {
        validatorsRefreshTimer = null;
        lastValidatorsRefreshAt = Date.now();
        loadValidators();
    }, waitMs);
}

// Initialize
document.addEventListener('DOMContentLoaded', () => {
    bindStaticControls();
    lastValidatorsRefreshAt = Date.now();
    loadValidators();

    let validatorPolling = null;
    let lastWsSlotAt = 0;

    const startPolling = () => {
        if (validatorPolling) return;
        validatorPolling = setInterval(loadValidators, 10000);
    };

    const stopPolling = () => {
        if (validatorPolling) {
            clearInterval(validatorPolling);
            validatorPolling = null;
        }
    };

    if (typeof ws !== 'undefined') {
        ws.onOpen(() => {
            stopPolling();
            lastWsSlotAt = Date.now();
            ws.subscribe('subscribeSlots', () => {
                lastWsSlotAt = Date.now();
                scheduleValidatorsRefresh();
            });
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

        setInterval(() => {
            if (!ws.isConnected()) {
                startPolling();
                return;
            }
            stopPolling();
            if (lastWsSlotAt && (Date.now() - lastWsSlotAt) > 30000) {
                scheduleValidatorsRefresh();
            }
        }, 15000);
    } else {
        startPolling();
    }
});
