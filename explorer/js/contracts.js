// Reef Explorer — Smart Contracts (on-chain data ONLY)
// Shows contracts actually deployed on MoltChain.
// Uses shared NETWORKS, RPC_URL, rpc from explorer.js

// Template → category mapping
var TEMPLATE_CATEGORIES = {
    mt20: 'token', token: 'token', fungible_token: 'token',
    wrapped: 'wrapped',
    dex: 'dex', amm: 'dex', orderbook: 'dex',
    governance: 'dex',
    defi: 'defi', lending: 'defi', bridge: 'defi', oracle: 'defi',
    dao: 'infra', identity: 'infra', storage: 'infra',
    marketplace: 'infra', auction: 'infra', nft: 'infra',
    payments: 'infra', launchpad: 'infra', vault: 'infra',
    bounty: 'infra', compute: 'infra',
};

// Template → Font Awesome icon class
var TEMPLATE_ICONS = {
    mt20: 'fa-coins', token: 'fa-coins', fungible_token: 'fa-coins',
    wrapped: 'fa-link',
    dex: 'fa-exchange-alt', amm: 'fa-exchange-alt', orderbook: 'fa-exchange-alt',
    governance: 'fa-users',
    defi: 'fa-chart-bar', lending: 'fa-hand-holding-usd', bridge: 'fa-bridge',
    oracle: 'fa-satellite-dish',
    dao: 'fa-landmark', identity: 'fa-id-card', storage: 'fa-database',
    marketplace: 'fa-store', auction: 'fa-gavel', nft: 'fa-image',
    payments: 'fa-credit-card', launchpad: 'fa-rocket', vault: 'fa-vault',
    bounty: 'fa-bullseye', compute: 'fa-microchip',
};

var CATEGORY_LABELS = {
    token: 'Token', wrapped: 'Wrapped', dex: 'DEX', defi: 'DeFi', infra: 'Infra',
};

var allContracts = [];
var currentFilter = 'all';

// ── Fetch real on-chain data ───────────────────────────────────────

async function loadContracts() {
    var tbody = document.getElementById('contractsTableBody');
    tbody.innerHTML = '<tr class="loading-row"><td colspan="7"><div class="loading-spinner"></div> Loading contracts...</td></tr>';

    try {
        // Fetch all deployed programs + symbol registry in parallel
        var results = await Promise.all([
            rpc.call('getAllContracts', []).catch(function() { return null; }),
            rpc.call('getAllSymbolRegistry', []).catch(function() { return null; }),
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
        var enrichPromises = programs.map(async function(prog) {
            var pid = prog.program_id;
            var reg = regByProgram[pid] || null;
            var info = null;
            var abi = null;

            try {
                var fetched = await Promise.all([
                    rpc.call('getContractInfo', [pid]).catch(function() { return null; }),
                    rpc.call('getContractAbi', [pid]).catch(function() { return null; }),
                ]);
                info = fetched[0];
                abi = fetched[1];
            } catch (e) {}

            var template = (reg && reg.template) || (prog.metadata && prog.metadata.template) || '';
            var category = TEMPLATE_CATEGORIES[template] || 'infra';
            var iconClass = TEMPLATE_ICONS[template] || 'fa-file-code';

            // Registry name takes priority over ABI name (ABI extraction defaults to "unknown")
            var abiName = (abi && abi.name && abi.name !== 'unknown') ? abi.name : '';
            var name = (reg && reg.name) || abiName || (prog.metadata && prog.metadata.name) || '';
            var symbol = (reg && reg.symbol) || (prog.metadata && prog.metadata.symbol) || '';
            var displayName = name || symbol || formatHash(pid, 14);

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
        allContracts.sort(function(a, b) {
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
        : allContracts.filter(function(c) { return c.category === currentFilter; });

    if (filtered.length === 0) {
        var msg = allContracts.length === 0
            ? '<div class="empty-state-box">' +
              '<i class="fas fa-file-code empty-state-icon"></i>' +
              '<h3>No Contracts Deployed</h3>' +
              '<p>Smart contracts will appear here once they are deployed on-chain.<br>' +
              'Deploy contracts using the MoltChain SDK or run <code>first-boot-deploy.sh</code> to deploy the DEX and token infrastructure.</p>' +
              '</div>'
            : '<div class="empty-state-box">' +
              '<i class="fas fa-filter empty-state-icon"></i>' +
              '<h3>No Contracts in This Category</h3>' +
              '<p>No deployed contracts match the selected filter.</p>' +
              '</div>';
        tbody.innerHTML = '<tr><td colspan="7">' + msg + '</td></tr>';
        return;
    }

    tbody.innerHTML = filtered.map(function(c) {
        var link = 'contract.html?address=' + c.address;
        var addr = '<a href="' + link + '" class="hash-link">' + formatHash(c.address, 10) + '</a>';
        var codeSize = c.codeSize > 0 ? formatBytes(c.codeSize) : '<span class="text-muted">\u2014</span>';
        var abiFuncs = c.abiFuncs > 0 ? c.abiFuncs : '<span class="text-muted">\u2014</span>';
        var owner = c.owner
            ? '<a href="address.html?address=' + c.owner + '" class="hash-link">' + formatHash(c.owner, 8) + '</a>'
            : '<span class="text-muted">\u2014</span>';
        var catLabel = CATEGORY_LABELS[c.category] || c.category;

        return '<tr onclick="window.location=\'' + link + '\'" style="cursor:pointer;">' +
            '<td><div class="contract-name-cell">' +
                '<span class="contract-icon-fa"><i class="fas ' + c.iconClass + '"></i></span>' +
                '<div><div class="contract-display">' + c.display + '</div>' +
                (c.symbol ? '<div class="contract-symbol">$' + c.symbol + '</div>' : '') +
            '</div></div></td>' +
            '<td><span class="badge-cat badge-' + c.category + '">' + catLabel + '</span></td>' +
            '<td>' + addr + '</td>' +
            '<td>' + codeSize + '</td>' +
            '<td>' + abiFuncs + '</td>' +
            '<td>' + owner + '</td>' +
            '<td><span class="status-success"><i class="fas fa-check-circle"></i> Live</span></td>' +
        '</tr>';
    }).join('');
}

function filterContracts(cat) {
    currentFilter = cat;
    document.querySelectorAll('.tab-btn').forEach(function(b) { b.classList.remove('active'); });
    var btn = document.getElementById('tab-' + cat);
    if (btn) btn.classList.add('active');
    renderContracts();
}

function updateStats() {
    var total = allContracts.length;
    var tokens = allContracts.filter(function(c) { return c.category === 'token' || c.category === 'wrapped'; }).length;
    var dex = allContracts.filter(function(c) { return c.category === 'dex'; }).length;
    var defi = allContracts.filter(function(c) { return c.category === 'defi'; }).length;

    document.getElementById('statTotal').textContent = total;
    document.getElementById('statTokens').textContent = tokens;
    document.getElementById('statDex').textContent = dex;
    document.getElementById('statDefi').textContent = defi + allContracts.filter(function(c) { return c.category === 'infra'; }).length;
}

// ── Init ──────────────────────────────────────────────────────────

function initSearch() {
    var input = document.getElementById('searchInput');
    if (!input) return;
    input.addEventListener('keydown', function(e) {
        if (e.key === 'Enter') {
            var q = input.value.trim();
            if (!q) return;
            if (/^\d+$/.test(q)) window.location.href = 'block.html?slot=' + q;
            else if (q.length === 64) window.location.href = 'transaction.html?sig=' + q;
            else window.location.href = 'address.html?address=' + q;
        }
    });
}

document.addEventListener('DOMContentLoaded', function() {
    if (typeof initExplorerNetworkSelector === 'function') initExplorerNetworkSelector();
    initSearch();
    var navToggle = document.getElementById('navToggle');
    var navMenu = document.querySelector('.nav-menu');
    if (navToggle && navMenu) {
        navToggle.addEventListener('click', function() {
            navMenu.classList.toggle('active');
            navToggle.classList.toggle('active');
        });
    }
    loadContracts();
});