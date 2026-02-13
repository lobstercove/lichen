// Reef Explorer — Contract Detail Page (contract.html)
// Fetches full contract details: info, ABI, storage, registry, calls, events
// Uses shared NETWORKS, RPC_URL, rpc from explorer.js (loaded first).
// Font Awesome only — no emojis.

const TEMPLATE_CATEGORY = {
    mt20: 'token', fungible_token: 'token', token: 'token',
    wrapped: 'wrapped',
    mt721: 'infra', nft: 'infra',
    dex: 'dex', amm: 'dex', orderbook: 'dex', clob: 'dex',
    defi: 'defi', lending: 'defi', bridge: 'defi', oracle: 'defi',
    governance: 'infra', dao: 'infra', identity: 'infra', storage: 'infra',
    payments: 'infra', launchpad: 'infra', vault: 'infra',
    bounty: 'infra', compute: 'infra', marketplace: 'infra', auction: 'infra',
};

const TEMPLATE_FA_ICON = {
    mt20: 'fa-coins', fungible_token: 'fa-coins', token: 'fa-coins',
    wrapped: 'fa-link',
    mt721: 'fa-image', nft: 'fa-image',
    dex: 'fa-exchange-alt', amm: 'fa-exchange-alt', orderbook: 'fa-exchange-alt',
    defi: 'fa-chart-bar', lending: 'fa-hand-holding-usd', bridge: 'fa-bridge',
    oracle: 'fa-satellite-dish',
    governance: 'fa-users', dao: 'fa-landmark', identity: 'fa-id-card',
    storage: 'fa-database', marketplace: 'fa-store', auction: 'fa-gavel',
    payments: 'fa-credit-card', launchpad: 'fa-rocket', vault: 'fa-vault',
    bounty: 'fa-bullseye', compute: 'fa-microchip',
};

const CATEGORY_BADGE = {
    token:   '<span class="badge-category success"><i class="fas fa-coins"></i> MT-20 Token</span>',
    wrapped: '<span class="badge-category warning"><i class="fas fa-link"></i> Wrapped Token</span>',
    dex:     '<span class="badge-category info"><i class="fas fa-exchange-alt"></i> DEX</span>',
    defi:    '<span class="badge-category warning"><i class="fas fa-chart-bar"></i> DeFi</span>',
    infra:   '<span class="badge-category accent"><i class="fas fa-cogs"></i> Infrastructure</span>',
};

const CATEGORY_ICON_CLASS = {
    token: 'token', wrapped: 'wrapped', dex: 'dex', defi: 'defi', infra: 'infra',
};

let contractAddress = null;
let currentTab = 'abi';

// ── Copy address to clipboard ────────────────────────────────────

function copyAddress() {
    if (!contractAddress) return;
    navigator.clipboard.writeText(contractAddress).then(() => {
        if (typeof showToast === 'function') showToast('Address copied!');
    }).catch(() => {});
}

// ── Tab switching ────────────────────────────────────────────────

function switchTab(tab) {
    currentTab = tab;
    ['abi', 'storage', 'calls', 'events'].forEach(t => {
        const panel = document.getElementById('panel-' + t);
        const btn = document.getElementById('tab-' + t);
        if (panel) panel.style.display = (t === tab) ? '' : 'none';
        if (btn) btn.classList.toggle('active', t === tab);
    });
}

// ── Main data loading ────────────────────────────────────────────

async function loadContract() {
    const params = new URLSearchParams(window.location.search);
    contractAddress = params.get('address');

    if (!contractAddress) {
        document.getElementById('contractTitle').textContent = 'Contract Not Found';
        return;
    }

    document.getElementById('contractAddress').textContent = contractAddress;
    document.title = 'Contract ' + contractAddress.slice(0, 12) + '... - Reef Explorer';

    // Fetch all data in parallel
    const [info, registry, abi, program, calls, events] = await Promise.all([
        rpc.call('getContractInfo', [contractAddress]).catch(() => null),
        rpc.call('getSymbolRegistryByProgram', [contractAddress]).catch(() => null),
        rpc.call('getContractAbi', [contractAddress]).catch(() => null),
        rpc.call('getProgram', [contractAddress]).catch(() => null),
        rpc.call('getProgramCalls', [contractAddress, { limit: 50 }]).catch(() => null),
        rpc.call('getContractLogs', [contractAddress, 50]).catch(() => null),
    ]);

    // Determine template/category
    const template = registry?.template || (abi?.template && abi.template !== 'unknown' ? abi.template : '') || '';
    const category = TEMPLATE_CATEGORY[template] || 'infra';
    const faIcon   = TEMPLATE_FA_ICON[template] || 'fa-file-code';

    // Header — registry name takes priority over ABI name (ABI defaults to "unknown")
    const abiName = (abi?.name && abi.name !== 'unknown') ? abi.name : '';
    const name    = registry?.name || abiName || '';
    const symbol  = registry?.symbol || '';
    const title   = name ? name + (symbol ? ' (' + symbol + ')' : '') : (symbol || formatHash(contractAddress, 16));

    // Set icon (Font Awesome only)
    const iconBox = document.getElementById('contractIconBox');
    const iconEl = document.getElementById('contractIcon');
    iconBox.className = 'contract-header-icon ' + (CATEGORY_ICON_CLASS[category] || 'infra');
    iconEl.className = 'fas ' + faIcon;

    document.getElementById('contractTitle').textContent = title;
    document.getElementById('contractSymbol').textContent = symbol ? '$' + symbol : '';
    document.getElementById('contractBadge').innerHTML = CATEGORY_BADGE[category] || '';
    document.title = title + ' - Reef Explorer';

    // Overview stats
    const owner = info?.owner || program?.owner || registry?.owner || '';
    if (owner) {
        document.getElementById('statOwner').innerHTML =
            '<a href="address.html?address=' + owner + '">' + formatHash(owner, 10) + '</a>';
    }
    document.getElementById('statCodeSize').textContent =
        info?.code_size ? formatBytes(info.code_size) : (program?.code_size ? formatBytes(program.code_size) : '--');
    document.getElementById('statAbiFunctions').textContent =
        info?.abi_functions || abi?.functions?.length || 0;
    document.getElementById('statStorage').textContent =
        program?.storage_entries ?? info?.storage_entries ?? '--';

    // Token info section (MT-20 / wrapped tokens)
    const isToken = (category === 'token' || category === 'wrapped');
    if (isToken) {
        const sec = document.getElementById('tokenInfoSection');
        sec.style.display = '';

        // Token metadata: merge from registry metadata + getContractInfo token_metadata
        const regMeta = registry?.metadata || {};
        const infoMeta = info?.token_metadata || {};
        const decimals = regMeta.decimals ?? infoMeta.decimals ?? 9;
        let supply = regMeta.total_supply ?? regMeta.supply ?? infoMeta.total_supply ?? null;

        document.getElementById('tokenDecimals').textContent = decimals;
        document.getElementById('tokenTemplate').textContent = template || 'mt20';

        // For native MOLT token, fetch live supply from getMetrics
        const isNative = info?.is_native || regMeta.is_native || (symbol === 'MOLT');
        if (isNative) {
            try {
                const metrics = await rpc.call('getMetrics');
                if (metrics?.total_supply) {
                    supply = metrics.total_supply;
                }
                // Holders = total accounts for native token
                if (metrics?.total_accounts !== undefined) {
                    document.getElementById('tokenHolders').textContent = formatNumber(metrics.total_accounts);
                }
            } catch (e) {}
        }

        if (supply !== null && supply !== undefined) {
            document.getElementById('tokenSupply').textContent =
                formatNumber(supply / Math.pow(10, decimals)) + (symbol ? ' ' + symbol : '');
        }

        // Mintable/burnable: check registry metadata first, then contract info, then ABI
        const mintable = regMeta.mintable ?? infoMeta.mintable
            ?? (abi?.functions?.some(f => f.name === 'mint') || false);
        const burnable = regMeta.burnable ?? infoMeta.burnable
            ?? (abi?.functions?.some(f => f.name === 'burn') || false);

        document.getElementById('tokenMintable').innerHTML =
            mintable === true ? '<span style="color:#4caf50;"><i class="fas fa-check"></i> Yes</span>'
            : mintable === false ? '<span style="color:var(--text-muted);"><i class="fas fa-times"></i> No</span>'
            : '<span style="color:var(--text-muted);">--</span>';
        document.getElementById('tokenBurnable').innerHTML =
            burnable === true ? '<span style="color:#4caf50;"><i class="fas fa-check"></i> Yes</span>'
            : burnable === false ? '<span style="color:var(--text-muted);"><i class="fas fa-times"></i> No</span>'
            : '<span style="color:var(--text-muted);">--</span>';

        // Token holders (non-native tokens: use getTokenHolders RPC)
        if (!isNative) {
            try {
                const holders = await rpc.call('getTokenHolders', [contractAddress, 1]).catch(() => null);
                if (holders?.count !== undefined) {
                    document.getElementById('tokenHolders').textContent = formatNumber(holders.count);
                }
            } catch (e) {}
        }
    }

    // Contract metadata section (for non-tokens or any contract with extra metadata)
    if (registry?.metadata && Object.keys(registry.metadata).length > 0) {
        const metaSection = document.getElementById('metadataSection');
        const metaGrid = document.getElementById('metadataGrid');
        const skipKeys = isToken ? ['decimals', 'total_supply', 'supply', 'mintable', 'burnable'] : [];
        const entries = Object.entries(registry.metadata).filter(([k]) => !skipKeys.includes(k));

        if (entries.length > 0) {
            metaSection.style.display = '';
            metaGrid.innerHTML = entries.map(([key, val]) => {
                const displayKey = key.replace(/_/g, ' ').replace(/\b\w/g, c => c.toUpperCase());
                const displayVal = typeof val === 'boolean' ? (val ? 'Yes' : 'No')
                    : typeof val === 'number' ? formatNumber(val)
                    : String(val);
                return '<div class="token-stat">' +
                    '<div class="label">' + displayKey + '</div>' +
                    '<div class="value">' + displayVal + '</div>' +
                '</div>';
            }).join('');
        }
    }

    // Render tabs
    renderAbi(abi);
    renderStorage(program);
    renderCalls(calls);
    renderEvents(events);
}

// ── ABI rendering ────────────────────────────────────────────────

function renderAbi(abi) {
    const tbody = document.getElementById('abiTable');

    if (!abi || !abi.functions || abi.functions.length === 0) {
        tbody.innerHTML = '<tr><td colspan="4" class="empty-state"><i class="fas fa-file-code"></i><div>No ABI available for this contract</div></td></tr>';
        return;
    }

    tbody.innerHTML = abi.functions.map(fn => {
        const params = fn.params && fn.params.length > 0
            ? fn.params.map(p => '<span style="color:var(--accent);">' + (p.param_type || p.type) + '</span> ' + p.name).join(', ')
            : '<span style="color:var(--text-muted);">none</span>';

        const returns = fn.returns
            ? '<span style="color:var(--accent);">' + (fn.returns.return_type || fn.returns.type || fn.returns) + '</span>'
            : '<span style="color:var(--text-muted);">void</span>';

        const readOnly = fn.readonly
            ? '<span class="badge info" style="font-size:0.75rem;"><i class="fas fa-eye"></i> View</span>'
            : '<span class="badge" style="background:rgba(255,170,0,0.15);color:#ffaa00;font-size:0.75rem;"><i class="fas fa-pen"></i> Write</span>';

        return '<tr>' +
            '<td style="font-weight:600;font-family:\'JetBrains Mono\',monospace;color:var(--text-primary);">' + fn.name + '</td>' +
            '<td style="font-family:\'JetBrains Mono\',monospace;font-size:0.85rem;">' + params + '</td>' +
            '<td style="font-family:\'JetBrains Mono\',monospace;font-size:0.85rem;">' + returns + '</td>' +
            '<td>' + readOnly + '</td>' +
        '</tr>';
    }).join('');

    if (abi.events && abi.events.length > 0) {
        const evtRow = '<tr style="border-top:2px solid var(--border);"><td colspan="4" style="color:var(--text-muted);font-size:0.85rem;padding-top:1rem;">' +
            '<i class="fas fa-bell" style="color:var(--accent);"></i> ' + abi.events.length + ' event' + (abi.events.length > 1 ? 's' : '') + ' defined: ' +
            abi.events.map(e => '<span style="color:var(--text-primary);font-weight:500;">' + e.name + '</span>').join(', ') +
        '</td></tr>';
        tbody.innerHTML += evtRow;
    }
}

// ── Storage rendering ────────────────────────────────────────────

function renderStorage(program) {
    const tbody = document.getElementById('storageTable');

    if (!program || !program.storage_entries || program.storage_entries === 0) {
        tbody.innerHTML = '<tr><td colspan="3" class="empty-state"><i class="fas fa-database"></i><div>No storage data</div></td></tr>';
        return;
    }

    rpc.call('getProgramStorage', [contractAddress]).then(res => {
        const entries = res?.entries || [];
        if (entries.length === 0) {
            tbody.innerHTML = '<tr><td colspan="3" class="empty-state"><i class="fas fa-database"></i><div>Storage is empty</div></td></tr>';
            return;
        }
        tbody.innerHTML = entries.map(entry => {
            const keyDisplay = entry.key_hex ? entry.key_hex.slice(0, 24) + (entry.key_hex.length > 24 ? '...' : '') : entry.key || '--';
            const valDisplay = entry.value_preview || entry.value_hex?.slice(0, 40) || '--';
            const size = entry.size || entry.value_hex?.length / 2 || 0;
            return '<tr>' +
                '<td style="font-family:\'JetBrains Mono\',monospace;font-size:0.85rem;">' + keyDisplay + '</td>' +
                '<td style="font-family:\'JetBrains Mono\',monospace;font-size:0.85rem;max-width:300px;overflow:hidden;text-overflow:ellipsis;">' + valDisplay + '</td>' +
                '<td>' + (size > 0 ? formatBytes(size) : '--') + '</td>' +
            '</tr>';
        }).join('');
    }).catch(() => {
        tbody.innerHTML = '<tr><td colspan="3" class="empty-state"><i class="fas fa-database"></i><div>Could not load storage</div></td></tr>';
    });
}

// ── Calls rendering ──────────────────────────────────────────────

function renderCalls(calls) {
    const tbody = document.getElementById('callsTable');
    const list = calls?.calls || calls?.activities || [];
    if (list.length === 0) {
        tbody.innerHTML = '<tr><td colspan="5" class="empty-state"><i class="fas fa-phone-alt"></i><div>No calls recorded</div></td></tr>';
        return;
    }

    tbody.innerHTML = list.map(call => {
        const time = call.timestamp ? timeAgo(call.timestamp) : (call.slot !== undefined ? 'Slot ' + formatNumber(call.slot) : '--');
        const caller = call.caller
            ? '<a href="address.html?address=' + call.caller + '" class="table-link" style="font-family:\'JetBrains Mono\',monospace;font-size:0.85rem;">' + formatHash(call.caller, 8) + '</a>'
            : '--';
        const fn = call.function_name || call.method || '--';
        const gas = call.gas_used !== undefined ? formatNumber(call.gas_used) : '--';
        const status = call.success !== false
            ? '<span class="badge success" style="font-size:0.75rem;"><i class="fas fa-check"></i> OK</span>'
            : '<span class="badge" style="background:rgba(255,70,70,0.15);color:#ff4646;font-size:0.75rem;"><i class="fas fa-times"></i> Failed</span>';

        return '<tr>' +
            '<td>' + time + '</td>' +
            '<td>' + caller + '</td>' +
            '<td style="font-family:\'JetBrains Mono\',monospace;font-weight:600;">' + fn + '</td>' +
            '<td>' + gas + '</td>' +
            '<td>' + status + '</td>' +
        '</tr>';
    }).join('');
}

// ── Events rendering ─────────────────────────────────────────────

function renderEvents(events) {
    const tbody = document.getElementById('eventsTable');
    const list = events?.logs || events?.events || [];
    if (list.length === 0) {
        tbody.innerHTML = '<tr><td colspan="3" class="empty-state"><i class="fas fa-bell"></i><div>No events emitted</div></td></tr>';
        return;
    }

    tbody.innerHTML = list.map(evt => {
        const slot = evt.slot !== undefined ? '<a href="block.html?slot=' + evt.slot + '" class="table-link">' + formatNumber(evt.slot) + '</a>' : '--';
        const name = evt.name || evt.event || '--';
        const data = typeof evt.data === 'object' ? JSON.stringify(evt.data) : (evt.data || '--');
        const dataDisplay = data.length > 80 ? data.slice(0, 80) + '...' : data;

        return '<tr>' +
            '<td>' + slot + '</td>' +
            '<td style="font-family:\'JetBrains Mono\',monospace;font-weight:600;color:var(--text-primary);">' + name + '</td>' +
            '<td style="font-family:\'JetBrains Mono\',monospace;font-size:0.85rem;max-width:400px;overflow:hidden;text-overflow:ellipsis;">' + dataDisplay + '</td>' +
        '</tr>';
    }).join('');
}

// ── Init ─────────────────────────────────────────────────────────

function initSearch() {
    const input = document.getElementById('searchInput');
    if (!input) return;
    input.addEventListener('keydown', (e) => {
        if (e.key === 'Enter') {
            const q = input.value.trim();
            if (!q) return;
            if (/^\d+$/.test(q)) window.location.href = 'block.html?slot=' + q;
            else if (q.length === 64) window.location.href = 'transaction.html?sig=' + q;
            else window.location.href = 'address.html?address=' + q;
        }
    });
}

document.addEventListener('DOMContentLoaded', () => {
    if (typeof initExplorerNetworkSelector === 'function') initExplorerNetworkSelector();
    initSearch();
    const navToggle = document.getElementById('navToggle');
    const navMenu = document.querySelector('.nav-menu');
    if (navToggle && navMenu) {
        navToggle.addEventListener('click', () => {
            navMenu.classList.toggle('active');
            navToggle.classList.toggle('active');
        });
    }
    loadContract();
});
