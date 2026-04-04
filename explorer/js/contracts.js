// Lichen Explorer — Smart Contracts (on-chain data ONLY)
// Shows contracts actually deployed on Lichen.
// Uses shared NETWORKS, RPC_URL, rpc from explorer.js

// Template → category mapping
var TEMPLATE_CATEGORIES = {
    mt20: 'token', token: 'token', fungible_token: 'token',
    wrapped: 'wrapped',
    dex: 'dex', amm: 'dex', orderbook: 'dex',
    nft: 'nft', mt721: 'nft', marketplace: 'nft', auction: 'nft',
    defi: 'defi', lending: 'defi', bridge: 'defi', oracle: 'defi',
    governance: 'governance', dao: 'governance',
    identity: 'infra', storage: 'infra',
    payments: 'infra', launchpad: 'infra', vault: 'infra',
    bounty: 'infra', compute: 'infra',
    staking: 'defi', vesting: 'defi', custody: 'defi', multisig: 'infra',
    faucet: 'infra', registry: 'infra', treasury: 'infra', escrow: 'infra',
    social: 'infra', content: 'infra', ai: 'infra', prediction: 'defi',
    insurance: 'defi', supply: 'infra', timelock: 'infra', crosschain: 'defi',
    shielded: 'infra',
};

// Template → Font Awesome icon class
var TEMPLATE_ICONS = {
    mt20: 'fa-coins', token: 'fa-coins', fungible_token: 'fa-coins',
    wrapped: 'fa-link',
    dex: 'fa-exchange-alt', amm: 'fa-exchange-alt', orderbook: 'fa-exchange-alt',
    governance: 'fa-users', dao: 'fa-landmark',
    defi: 'fa-chart-bar', lending: 'fa-hand-holding-usd', bridge: 'fa-bridge',
    oracle: 'fa-satellite-dish',
    nft: 'fa-image', mt721: 'fa-image',
    identity: 'fa-id-card', storage: 'fa-database',
    marketplace: 'fa-store', auction: 'fa-gavel',
    payments: 'fa-credit-card', launchpad: 'fa-rocket', vault: 'fa-vault',
    bounty: 'fa-bullseye', compute: 'fa-microchip',
    staking: 'fa-layer-group', vesting: 'fa-clock', custody: 'fa-shield-alt',
    multisig: 'fa-key', faucet: 'fa-faucet', registry: 'fa-list-alt',
    treasury: 'fa-piggy-bank', escrow: 'fa-handshake',
    social: 'fa-comments', content: 'fa-newspaper', ai: 'fa-brain',
    prediction: 'fa-chart-line', insurance: 'fa-umbrella',
    supply: 'fa-truck', timelock: 'fa-hourglass-half', crosschain: 'fa-globe',
};

var CATEGORY_LABELS = {
    token: 'Token', wrapped: 'Wrapped', nft: 'NFT', dex: 'DEX', defi: 'DeFi', governance: 'Governance', infra: 'Infra',
};

var allContracts = [];
var currentFilter = 'all';
var CONTRACTS_PER_PAGE = 25;
var currentPage = 1;

function bindStaticControls() {
    document.querySelectorAll('.tab-btn[data-contract-filter]').forEach(function (button) {
        button.addEventListener('click', function () {
            filterContracts(button.dataset.contractFilter || 'all');
        });
    });

    document.getElementById('prevPage')?.addEventListener('click', previousPage);
    document.getElementById('nextPage')?.addEventListener('click', nextPage);
    document.getElementById('contractsTableBody')?.addEventListener('click', function (event) {
        if (event.target.closest('a, button')) return;
        var row = event.target.closest('tr[data-contract-link]');
        if (!row || !row.dataset.contractLink) return;
        window.location.href = row.dataset.contractLink;
    });
}

// ── Fetch real on-chain data ───────────────────────────────────────

async function loadContracts() {
    var tbody = document.getElementById('contractsTableBody');
    tbody.innerHTML = '<tr class="loading-row"><td colspan="7"><div class="loading-spinner"></div> Loading contracts...</td></tr>';

    try {
        // Fetch all deployed programs + symbol registry in parallel
        var results = await Promise.all([
            trustedRpcCall('getAllContracts', []).catch(function () { return null; }),
            trustedRpcCall('getAllSymbolRegistry', []).catch(function () { return null; }),
        ]);

        var programs = (results[0] && results[0].contracts) ? results[0].contracts : [];
        var registry = (results[1] && results[1].entries) ? results[1].entries : [];

        // Index registry by program address
        var regByProgram = {};
        for (var i = 0; i < registry.length; i++) {
            var r = registry[i];
            if (r.program) regByProgram[r.program] = r;
        }

        // Enrich each program with contract info + registry
        var enrichPromises = programs.map(async function (prog) {
            var pid = prog.program_id;
            var reg = regByProgram[pid] || null;
            var info = null;
            var abi = null;

            try {
                var fetched = await Promise.all([
                    trustedRpcCall('getContractInfo', [pid]).catch(function () { return null; }),
                    rpc.call('getContractAbi', [pid]).catch(function () { return null; }),
                ]);
                info = fetched[0];
                abi = fetched[1];
            } catch (e) { }

            var template = (reg && reg.template) || prog.template || (prog.metadata && prog.metadata.template) || '';
            var category = TEMPLATE_CATEGORIES[template] || 'infra';
            var iconClass = TEMPLATE_ICONS[template] || 'fa-file-code';

            // Registry name takes priority over ABI name (ABI extraction defaults to "unknown")
            var abiName = (abi && abi.name && abi.name !== 'unknown') ? abi.name : '';
            var name = (reg && reg.name) || abiName || (prog.metadata && prog.metadata.name) || '';
            var symbol = (reg && reg.symbol) || (prog.metadata && prog.metadata.symbol) || '';
            var displayName = name || symbol || formatHash(pid);

            return {
                address: pid,
                display: displayName,
                symbol: symbol,
                category: category,
                iconClass: iconClass,
                template: template,
                codeSize: (info && info.code_size) ? info.code_size : 0,
                abiFuncs: (info && info.abi_functions) ? info.abi_functions : ((abi && abi.functions) ? abi.functions.length : 0),
                owner: (info && info.owner) ? info.owner : ((reg && reg.owner) ? reg.owner : ''),
                deployedAt: (info && info.deployed_at) ? info.deployed_at : null,
            };
        });

        allContracts = await Promise.all(enrichPromises);

        // Sort by name
        allContracts.sort(function (a, b) {
            return a.display.localeCompare(b.display);
        });

    } catch (e) {
        console.error('Failed to load contracts:', e);
        allContracts = [];
    }

    updateStats();
    renderContracts();
}

// ── Rendering ──────────────────────────────────────────────────────

function renderContracts() {
    var tbody = document.getElementById('contractsTableBody');
    var filtered = currentFilter === 'all'
        ? allContracts
        : allContracts.filter(function (c) { return c.category === currentFilter; });

    var totalPages = Math.max(1, Math.ceil(filtered.length / CONTRACTS_PER_PAGE));
    if (currentPage > totalPages) currentPage = totalPages;
    var start = (currentPage - 1) * CONTRACTS_PER_PAGE;
    var paged = filtered.slice(start, start + CONTRACTS_PER_PAGE);

    if (filtered.length === 0) {
        var msg = allContracts.length === 0
            ? '<div class="empty-state-box">' +
            '<i class="fas fa-file-code empty-state-icon"></i>' +
            '<h3>No Contracts Deployed</h3>' +
            '<p>Smart contracts will appear here once they are deployed on-chain.<br>' +
            'Deploy contracts using the Lichen SDK or run <code>first-boot-deploy.sh</code> to deploy the DEX and token infrastructure.</p>' +
            '</div>'
            : '<div class="empty-state-box">' +
            '<i class="fas fa-filter empty-state-icon"></i>' +
            '<h3>No Contracts in This Category</h3>' +
            '<p>No deployed contracts match the selected filter.</p>' +
            '</div>';
        tbody.innerHTML = '<tr><td colspan="7">' + msg + '</td></tr>';
        updatePagination(0);
        return;
    }

    var renderRows = async function () {
        var ownerAddresses = paged.map(function (c) { return c.owner; }).filter(Boolean);
        var nameMap = (typeof batchResolveLichenNames === 'function')
            ? await batchResolveLichenNames(ownerAddresses)
            : {};

        tbody.innerHTML = paged.map(function (c) {
            var link = 'contract.html?address=' + encodeURIComponent(c.address);
            var addr = '<a href="' + link + '" class="hash-link hash-short" title="' + escapeHtml(c.address) + '">' + formatHash(c.address) + '</a>';
            var codeSize = c.codeSize > 0 ? formatBytes(c.codeSize) : '<span class="text-muted">\u2014</span>';
            var abiFuncs = c.abiFuncs > 0 ? c.abiFuncs : '<span class="text-muted">\u2014</span>';
            var ownerName = c.owner ? nameMap[c.owner] : null;
            var ownerLabel = ownerName ? (ownerName + '.lichen') : formatHash(c.owner);
            var owner = c.owner
                ? '<a href="address.html?address=' + encodeURIComponent(c.owner) + '" class="hash-link" title="' + escapeHtml(c.owner) + '">' + escapeHtml(ownerLabel) + '</a>'
                : '<span class="text-muted">\u2014</span>';
            var catLabel = escapeHtml(CATEGORY_LABELS[c.category] || c.category);
            var display = escapeHtml(c.display);
            var symbol = c.symbol ? escapeHtml(c.symbol) : '';

            return '<tr data-contract-link="' + link + '" style="cursor:pointer;">' +
                '<td><div class="contract-name-cell">' +
                '<span class="contract-icon-fa"><i class="fas ' + c.iconClass + '"></i></span>' +
                '<div><div class="contract-display">' + display + '</div>' +
                (symbol ? '<div class="contract-symbol">$' + symbol + '</div>' : '') +
                '</div></div></td>' +
                '<td><span class="badge-cat badge-' + c.category + '">' + catLabel + '</span></td>' +
                '<td>' + addr + '</td>' +
                '<td>' + codeSize + '</td>' +
                '<td>' + abiFuncs + '</td>' +
                '<td>' + owner + '</td>' +
                '<td><span class="status-success"><i class="fas fa-check-circle"></i> Live</span></td>' +
                '</tr>';
        }).join('');

        updatePagination(filtered.length);
    };

    renderRows().catch(function () {
        tbody.innerHTML = '<tr><td colspan="7" style="text-align:center; color: #FF6B6B;">Failed to resolve owner names</td></tr>';
        updatePagination(filtered.length);
    });
}

function updatePagination(totalItems) {
    var totalPages = Math.max(1, Math.ceil(totalItems / CONTRACTS_PER_PAGE));
    var info = document.getElementById('paginationInfo');
    if (info) info.textContent = 'Page ' + currentPage + ' of ' + totalPages;

    var prevBtn = document.getElementById('prevPage');
    var nextBtn = document.getElementById('nextPage');
    if (prevBtn) prevBtn.disabled = currentPage <= 1 || totalItems === 0;
    if (nextBtn) nextBtn.disabled = currentPage >= totalPages || totalItems === 0;
}

function nextPage() {
    var filteredCount = currentFilter === 'all'
        ? allContracts.length
        : allContracts.filter(function (c) { return c.category === currentFilter; }).length;
    var totalPages = Math.max(1, Math.ceil(filteredCount / CONTRACTS_PER_PAGE));
    if (currentPage >= totalPages) return;
    currentPage += 1;
    renderContracts();
    window.scrollTo({ top: 0, behavior: 'smooth' });
}

function previousPage() {
    if (currentPage <= 1) return;
    currentPage -= 1;
    renderContracts();
    window.scrollTo({ top: 0, behavior: 'smooth' });
}

function filterContracts(cat) {
    currentFilter = cat;
    currentPage = 1;
    document.querySelectorAll('.tab-btn').forEach(function (b) { b.classList.remove('active'); });
    var btn = document.getElementById('tab-' + cat);
    if (btn) btn.classList.add('active');
    renderContracts();
}

function updateStats() {
    var total = allContracts.length;
    var tokens = allContracts.filter(function (c) { return c.category === 'token' || c.category === 'wrapped'; }).length;
    var dex = allContracts.filter(function (c) { return c.category === 'dex'; }).length;
    var nft = allContracts.filter(function (c) { return c.category === 'nft'; }).length;
    var defi = allContracts.filter(function (c) { return c.category === 'defi'; }).length;
    var infra = allContracts.filter(function (c) { return c.category === 'infra' || c.category === 'governance'; }).length;

    document.getElementById('statTotal').textContent = total;
    document.getElementById('statTokens').textContent = tokens;
    document.getElementById('statDex').textContent = dex;
    document.getElementById('statNft').textContent = nft;
    document.getElementById('statDefi').textContent = defi + infra;
}

// ── Init ──────────────────────────────────────────────────────────

function initSearch() {
    var input = document.getElementById('searchInput');
    if (!input) return;
    input.addEventListener('keydown', async function (e) {
        if (e.key === 'Enter') {
            var q = input.value.trim();
            if (!q) return;
            if (typeof navigateExplorerSearch === 'function') {
                await navigateExplorerSearch(q);
                return;
            }
            window.location.href = 'address.html?address=' + q;
        }
    });
}

document.addEventListener('DOMContentLoaded', function () {
    if (typeof initExplorerNetworkSelector === 'function') initExplorerNetworkSelector();
    initSearch();
    bindStaticControls();
    var navToggle = document.getElementById('navToggle');
    var navMenu = document.querySelector('.nav-menu');
    var navActions = document.querySelector('.nav-actions');
    var navContainer = document.querySelector('.nav-container');
    if (navToggle && navMenu) {
        navToggle.addEventListener('click', function () {
            var isOpen = !navMenu.classList.contains('active');
            navMenu.classList.toggle('active', isOpen);
            navMenu.classList.toggle('open', isOpen);
            if (navActions) {
                navActions.classList.toggle('active', isOpen);
                navActions.classList.toggle('open', isOpen);
            }
            navToggle.classList.toggle('active', isOpen);
            if (navContainer) {
                navContainer.style.setProperty('--nav-menu-height', isOpen ? navMenu.offsetHeight + 'px' : '0px');
            }
        });
    }
    loadContracts();
});