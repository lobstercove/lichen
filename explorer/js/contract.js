// Lichen Explorer — Contract Detail Page (contract.html)
// Fetches full contract details: info, ABI, storage, registry, calls, events
// Uses shared NETWORKS, RPC_URL, rpc from explorer.js (loaded first).
// Font Awesome only — no emojis.

const TEMPLATE_CATEGORY = {
    mt20: 'token', fungible_token: 'token', token: 'token',
    wrapped: 'wrapped',
    mt721: 'nft', nft: 'nft', marketplace: 'nft', auction: 'nft',
    dex: 'dex', amm: 'dex', orderbook: 'dex', clob: 'dex',
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
    staking: 'fa-layer-group', vesting: 'fa-clock', custody: 'fa-shield-alt',
    multisig: 'fa-key', faucet: 'fa-faucet', registry: 'fa-list-alt',
    treasury: 'fa-piggy-bank', escrow: 'fa-handshake',
    social: 'fa-comments', content: 'fa-newspaper', ai: 'fa-brain',
    prediction: 'fa-chart-line', insurance: 'fa-umbrella',
    supply: 'fa-truck', timelock: 'fa-hourglass-half', crosschain: 'fa-globe',
};

const CATEGORY_BADGE = {
    token: '<span class="badge-category success"><i class="fas fa-coins"></i> MT-20 Token</span>',
    wrapped: '<span class="badge-category warning"><i class="fas fa-link"></i> Wrapped Token</span>',
    nft: '<span class="badge-category accent"><i class="fas fa-image"></i> NFT</span>',
    dex: '<span class="badge-category info"><i class="fas fa-exchange-alt"></i> DEX</span>',
    defi: '<span class="badge-category warning"><i class="fas fa-chart-bar"></i> DeFi</span>',
    governance: '<span class="badge-category info"><i class="fas fa-users"></i> Governance</span>',
    infra: '<span class="badge-category accent"><i class="fas fa-cogs"></i> Infrastructure</span>',
};

const CATEGORY_ICON_CLASS = {
    token: 'token', wrapped: 'wrapped', nft: 'nft', dex: 'dex', defi: 'defi', governance: 'governance', infra: 'infra',
};

let contractAddress = null;
let currentTab = 'abi';

// ── Copy address to clipboard ────────────────────────────────────

function copyAddress() {
    if (!contractAddress) return;
    navigator.clipboard.writeText(contractAddress).then(() => {
        if (typeof showToast === 'function') showToast('Address copied!');
    }).catch(() => { });
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

function isLikelyUrl(value) {
    return typeof value === 'string' && /^https?:\/\//i.test(value.trim());
}

function formatMetadataValue(value) {
    if (value === null || value === undefined) {
        return '<span style="color:var(--text-muted);">--</span>';
    }

    if (typeof value === 'boolean') {
        return value ? 'Yes' : 'No';
    }

    if (typeof value === 'number') {
        return formatNumber(value);
    }

    if (typeof value === 'string') {
        const text = value.trim();
        if (!text) return '<span style="color:var(--text-muted);">--</span>';
        if (isLikelyUrl(text)) {
            const safeUrl = escapeHtml(text);
            return '<a href="' + safeUrl + '" target="_blank" rel="noopener noreferrer">' + safeUrl + '</a>';
        }
        return escapeHtml(text);
    }

    if (Array.isArray(value)) {
        if (value.length === 0) return '<span style="color:var(--text-muted);">--</span>';
        return value.map((entry) => formatMetadataValue(entry)).join('<br>');
    }

    if (typeof value === 'object') {
        const entries = Object.entries(value);
        if (!entries.length) return '<span style="color:var(--text-muted);">--</span>';
        return entries.map(([k, v]) => {
            return '<div><strong>' + escapeHtml(String(k).replace(/_/g, ' ')) + ':</strong> ' + formatMetadataValue(v) + '</div>';
        }).join('');
    }

    return escapeHtml(String(value));
}

function normalizeProfileMetadata(registryMetadata, tokenMetadata) {
    const reg = registryMetadata && typeof registryMetadata === 'object' ? registryMetadata : {};
    const token = tokenMetadata && typeof tokenMetadata === 'object' ? tokenMetadata : {};

    const merged = { ...token, ...reg };

    const logoUrl = merged.logo_url || merged.logo || merged.icon || merged.icon_url || merged.image || '';
    const website = merged.website || merged.homepage || merged.project_url || '';
    const twitter = merged.twitter || merged.x || merged.x_url || merged.social_urls?.twitter || '';
    const telegram = merged.telegram || merged.social_urls?.telegram || '';
    const discord = merged.discord || merged.social_urls?.discord || '';
    const description = merged.description || merged.desc || '';

    return {
        ...merged,
        logo_url: logoUrl,
        website,
        twitter,
        telegram,
        discord,
        description,
    };
}

// ── Main data loading ────────────────────────────────────────────

async function loadContract() {
    const params = new URLSearchParams(window.location.search);
    contractAddress = params.get('address');

    if (!contractAddress) {
        document.getElementById('contractTitle').textContent = 'Contract Not Found';
        return;
    }

    document.getElementById('contractAddress').textContent = formatHash(contractAddress);
    document.getElementById('contractAddress').title = contractAddress;
    document.getElementById('contractAddress').dataset.full = contractAddress;
    document.title = 'Contract ' + contractAddress.slice(0, 12) + '... - Lichen Explorer';

    // Fetch all data in parallel
    const [info, registry, abi, program, calls, events] = await Promise.all([
        rpc.call('getContractInfo', [contractAddress]).catch(() => null),
        rpc.call('getSymbolRegistryByProgram', [contractAddress]).catch(() => null),
        rpc.call('getContractAbi', [contractAddress]).catch(() => null),
        rpc.call('getProgram', [contractAddress]).catch(() => null),
        rpc.call('getProgramCalls', [contractAddress, { limit: 200 }]).catch(() => null),
        rpc.call('getContractLogs', [contractAddress, 200]).catch(() => null),
    ]);

    // Determine template/category
    const template = registry?.template || (abi?.template && abi.template !== 'unknown' ? abi.template : '') || '';
    const category = TEMPLATE_CATEGORY[template] || 'infra';
    const faIcon = TEMPLATE_FA_ICON[template] || 'fa-file-code';

    // Header — registry name takes priority over ABI name (ABI defaults to "unknown")
    const abiName = (abi?.name && abi.name !== 'unknown') ? abi.name : '';
    const name = registry?.name || abiName || '';
    const symbol = registry?.symbol || '';
    const title = name ? name + (symbol ? ' (' + symbol + ')' : '') : (symbol || formatHash(contractAddress));

    // Set icon (Font Awesome only)
    const iconBox = document.getElementById('contractIconBox');
    const iconEl = document.getElementById('contractIcon');
    iconBox.className = 'contract-header-icon ' + (CATEGORY_ICON_CLASS[category] || 'infra');
    iconEl.className = 'fas ' + faIcon;

    document.getElementById('contractTitle').textContent = title;
    document.getElementById('contractSymbol').textContent = symbol ? '$' + symbol : '';
    document.getElementById('contractBadge').innerHTML = CATEGORY_BADGE[category] || '';
    document.title = title + ' - Lichen Explorer';

    // Overview stats
    const owner = info?.owner || program?.owner || registry?.owner || '';
    const addressNames = (typeof batchResolveLichenNames === 'function')
        ? await batchResolveLichenNames([
            owner,
            ...(calls?.calls || calls?.activities || []).map(c => c.caller).filter(Boolean)
        ])
        : {};
    if (owner) {
        const ownerLabel = addressNames[owner] ? `${addressNames[owner]}.lichen` : formatHash(owner);
        document.getElementById('statOwner').innerHTML =
            '<a href="address.html?address=' + owner + '" title="' + owner + '">' + ownerLabel + '</a>';
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

        // Token metadata: primary source is getContractInfo token_metadata (backed by registry)
        const regMeta = registry?.metadata || {};
        const infoMeta = info?.token_metadata || {};
        const decimals = registry?.decimals ?? infoMeta.decimals ?? 9;
        let supply = infoMeta.total_supply ?? null;

        // Fallback: if supply is 0 or null, try calling the contract's total_supply view
        const isNative = info?.is_native || regMeta.is_native || (symbol === 'LICN');
        if ((!supply || supply === 0) && !isNative) {
            try {
                const viewResult = await rpc.call('callContract', [contractAddress, 'total_supply']);
                if (viewResult?.return_data) {
                    // return_data is base64-encoded u64 LE bytes
                    const decoded = atob(viewResult.return_data);
                    if (decoded.length >= 8) {
                        const bytes = new Uint8Array(decoded.length);
                        for (let i = 0; i < decoded.length; i++) bytes[i] = decoded.charCodeAt(i);
                        const dv = new DataView(bytes.buffer);
                        const val = Number(dv.getBigUint64(0, true));
                        if (val > 0) supply = val;
                    }
                }
            } catch (e) { /* contract may not have total_supply function */ }
        }

        document.getElementById('tokenDecimals').textContent = decimals;
        document.getElementById('tokenTemplate').textContent = template || 'mt20';

        // For native LICN token, fetch live supply from getMetrics
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
            } catch (e) { }
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
            } catch (e) { }
        }
    }

    // Contract metadata section (for non-tokens or any contract with extra metadata)
    const PROFILE_KEYS = ['logo_url', 'website', 'twitter', 'telegram', 'discord', 'social_urls', 'description'];
    const regMeta2 = normalizeProfileMetadata(registry?.metadata, info?.token_metadata);
    if (Object.keys(regMeta2).length > 0) {

        // Token profile section (logo, description, social links)
        const hasProfile = PROFILE_KEYS.some(k => regMeta2[k]);
        if (hasProfile) {
            const profileEl = document.getElementById('tokenProfileSection');
            profileEl.style.display = '';

            const logoUrl = regMeta2.logo_url || '';
            const description = regMeta2.description || '';
            const website = regMeta2.website || '';
            const twitter = regMeta2.twitter || (regMeta2.social_urls?.twitter) || '';
            const telegram = regMeta2.telegram || (regMeta2.social_urls?.telegram) || '';
            const discord = regMeta2.discord || (regMeta2.social_urls?.discord) || '';

            let html = '';
            if (logoUrl) {
                html += '<img src="' + escapeHtml(logoUrl) + '" alt="Token Logo" class="token-profile-logo" onerror="this.style.display=\'none\';this.nextElementSibling.style.display=\'flex\'">';
                html += '<div class="token-profile-logo-placeholder" style="display:none;"><i class="fas fa-coins"></i></div>';
            } else {
                html += '<div class="token-profile-logo-placeholder"><i class="fas fa-coins"></i></div>';
            }

            html += '<div class="token-profile-body">';
            if (description) {
                html += '<div class="token-profile-desc">' + escapeHtml(description) + '</div>';
            }

            const links = [];
            if (website) links.push('<a href="' + escapeHtml(website) + '" target="_blank" rel="noopener noreferrer"><i class="fas fa-globe"></i> Website</a>');
            if (twitter) links.push('<a href="' + escapeHtml(twitter) + '" target="_blank" rel="noopener noreferrer"><i class="fab fa-x-twitter"></i> Twitter</a>');
            if (telegram) links.push('<a href="' + escapeHtml(telegram) + '" target="_blank" rel="noopener noreferrer"><i class="fab fa-telegram"></i> Telegram</a>');
            if (discord) links.push('<a href="' + escapeHtml(discord) + '" target="_blank" rel="noopener noreferrer"><i class="fab fa-discord"></i> Discord</a>');

            if (links.length > 0) {
                html += '<div class="token-profile-links">' + links.join('') + '</div>';
            }
            html += '</div>';
            profileEl.innerHTML = html;
        }

        const metaSection = document.getElementById('metadataSection');
        const metaGrid = document.getElementById('metadataGrid');
        const INTERNAL_KEYS = ['icon_class', 'genesis_deploy', 'wasm_size', 'is_native'];
        const skipKeys = isToken
            ? ['decimals', 'total_supply', 'supply', 'mintable', 'burnable', ...PROFILE_KEYS, ...INTERNAL_KEYS]
            : [...PROFILE_KEYS, ...INTERNAL_KEYS];
        const entries = Object.entries(registry?.metadata || {}).filter(([k]) => !skipKeys.includes(k));

        if (entries.length > 0) {
            metaSection.style.display = '';
            metaGrid.innerHTML = entries.map(([key, val]) => {
                const displayKey = escapeHtml(key.replace(/_/g, ' ').replace(/\b\w/g, c => c.toUpperCase()));
                const displayVal = formatMetadataValue(val);
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
    renderCalls(calls, addressNames);
    renderEvents(events);
}

// ── ABI rendering ────────────────────────────────────────────────

function renderAbi(abi) {
    const tbody = document.getElementById('abiTable');

    if (!abi || !abi.functions || abi.functions.length === 0) {
        tbody.innerHTML = '<tr><td colspan="4" class="empty-state"><i class="fas fa-file-code"></i><div>No ABI available for this contract</div></td></tr>';
        return;
    }

    // Heuristic: functions that are read-only (no state mutation)
    const VIEW_FN = new Set(['balance_of', 'total_supply', 'allowance', 'name', 'symbol', 'decimals', 'owner', 'supply', 'uri', 'token_uri', 'metadata', 'nonce']);
    const VIEW_PREFIX = ['get_', 'is_', 'has_', 'can_', 'check_', 'query_', 'view_', 'read_', 'count_', 'list_'];
    function isViewFn(name) {
        if (VIEW_FN.has(name)) return true;
        return VIEW_PREFIX.some(p => name.startsWith(p));
    }

    tbody.innerHTML = abi.functions.map(fn => {
        const safeName = escapeHtml(fn.name);
        const params = fn.params && fn.params.length > 0
            ? fn.params.map(p => '<span style="color:var(--accent);">' + escapeHtml(p.param_type || p.type) + '</span> ' + escapeHtml(p.name)).join(', ')
            : '<span style="color:var(--text-muted);">none</span>';

        const returns = fn.returns
            ? '<span style="color:var(--accent);">' + escapeHtml(fn.returns.return_type || fn.returns.type || fn.returns) + '</span>'
            : '<span style="color:var(--text-muted);">void</span>';

        const readOnly = fn.readonly || isViewFn(fn.name)
            ? '<span class="badge info" style="font-size:0.75rem;"><i class="fas fa-eye"></i> View</span>'
            : '<span class="badge" style="background:rgba(255,170,0,0.15);color:#00D4E8;font-size:0.75rem;"><i class="fas fa-pen"></i> Write</span>';

        return '<tr>' +
            '<td style="font-weight:600;font-family:\'JetBrains Mono\',monospace;color:var(--text-primary);">' + safeName + '</td>' +
            '<td style="font-family:\'JetBrains Mono\',monospace;font-size:0.85rem;">' + params + '</td>' +
            '<td style="font-family:\'JetBrains Mono\',monospace;font-size:0.85rem;">' + returns + '</td>' +
            '<td>' + readOnly + '</td>' +
            '</tr>';
    }).join('');

    if (abi.events && abi.events.length > 0) {
        const evtRow = '<tr style="border-top:2px solid var(--border);"><td colspan="4" style="color:var(--text-muted);font-size:0.85rem;padding-top:1rem;">' +
            '<i class="fas fa-bell" style="color:var(--accent);"></i> ' + abi.events.length + ' event' + (abi.events.length > 1 ? 's' : '') + ' defined: ' +
            abi.events.map(e => '<span style="color:var(--text-primary);font-weight:500;">' + escapeHtml(e.name) + '</span>').join(', ') +
            '</td></tr>';
        tbody.innerHTML += evtRow;
    }
}

// ── Storage rendering (paginated) ────────────────────────────────

const STORAGE_PAGE_SIZE = 25;
let storageOffset = 0;
let storageTotal = 0;

function renderStorage(program) {
    const tbody = document.getElementById('storageTable');
    const paginationEl = document.getElementById('storagePagination');

    if (!program || !program.storage_entries || program.storage_entries === 0) {
        tbody.innerHTML = '<tr><td colspan="3" class="empty-state"><i class="fas fa-database"></i><div>No storage data</div></td></tr>';
        if (paginationEl) paginationEl.style.display = 'none';
        return;
    }

    loadStoragePage(0);
}

async function loadStoragePage(offset) {
    const tbody = document.getElementById('storageTable');
    tbody.innerHTML = '<tr><td colspan="3" class="empty-state"><i class="fas fa-spinner fa-spin"></i><div>Loading...</div></td></tr>';

    try {
        const res = await rpc.call('getProgramStorage', [contractAddress, { limit: STORAGE_PAGE_SIZE, offset }]);
        const entries = res?.entries || [];
        storageTotal = res?.total || entries.length;
        storageOffset = offset;

        if (entries.length === 0) {
            tbody.innerHTML = '<tr><td colspan="3" class="empty-state"><i class="fas fa-database"></i><div>Storage is empty</div></td></tr>';
            updateStoragePagination();
            return;
        }

        tbody.innerHTML = entries.map(entry => {
            const keyDecoded = entry.key_decoded || null;
            const keyHex = entry.key_hex || entry.key || '--';
            const keyDisplay = keyDecoded
                ? '<span title="' + keyHex + '">' + escapeHtml(keyDecoded) + '</span>'
                : '<span title="' + keyHex + '">' + (keyHex.length > 24 ? keyHex.slice(0, 24) + '...' : keyHex) + '</span>';
            const valPreview = entry.value_preview || entry.value_hex?.slice(0, 40) || entry.value?.slice(0, 40) || '--';
            const size = entry.size != null && entry.size > 0 ? formatBytes(entry.size) : '--';
            return '<tr>' +
                '<td style="font-family:\'JetBrains Mono\',monospace;font-size:0.85rem;">' + keyDisplay + '</td>' +
                '<td style="font-family:\'JetBrains Mono\',monospace;font-size:0.85rem;max-width:300px;overflow:hidden;text-overflow:ellipsis;" title="' + escapeHtml(entry.value_hex || entry.value || '') + '">' + escapeHtml(valPreview) + '</td>' +
                '<td>' + size + '</td>' +
                '</tr>';
        }).join('');

        updateStoragePagination();
    } catch (e) {
        tbody.innerHTML = '<tr><td colspan="3" class="empty-state"><i class="fas fa-database"></i><div>Could not load storage</div></td></tr>';
    }
}

function updateStoragePagination() {
    let paginationEl = document.getElementById('storagePagination');
    if (!paginationEl) {
        const panel = document.getElementById('panel-storage');
        if (panel) {
            paginationEl = document.createElement('div');
            paginationEl.id = 'storagePagination';
            paginationEl.className = 'tab-pagination';
            panel.appendChild(paginationEl);
        } else return;
    }

    if (storageTotal <= STORAGE_PAGE_SIZE) {
        paginationEl.style.display = 'none';
        return;
    }

    const totalPages = Math.ceil(storageTotal / STORAGE_PAGE_SIZE);
    const currentPage = Math.floor(storageOffset / STORAGE_PAGE_SIZE) + 1;

    paginationEl.style.display = 'flex';
    paginationEl.innerHTML =
        '<span class="pagination-info">Page ' + currentPage + ' of ' + totalPages + ' (' + storageTotal + ' entries)</span>' +
        '<div class="pagination-btns">' +
        '<button class="btn btn-secondary btn-small" data-page-action="prev" data-offset="' + Math.max(0, storageOffset - STORAGE_PAGE_SIZE) + '"' + (storageOffset <= 0 ? ' disabled' : '') + '><i class="fas fa-arrow-left"></i> Prev</button>' +
        '<button class="btn btn-secondary btn-small" data-page-action="next" data-offset="' + (storageOffset + STORAGE_PAGE_SIZE) + '"' + (storageOffset + STORAGE_PAGE_SIZE >= storageTotal ? ' disabled' : '') + '>Next <i class="fas fa-arrow-right"></i></button>' +
        '</div>';

    if (!paginationEl.dataset.bound) {
        paginationEl.addEventListener('click', (event) => {
            const button = event.target.closest('button[data-page-action]');
            if (!button || button.disabled) return;
            const offset = Number(button.dataset.offset);
            if (Number.isFinite(offset)) {
                loadStoragePage(offset);
            }
        });
        paginationEl.dataset.bound = '1';
    }
}

// ── Calls rendering (paginated) ──────────────────────────────────

const CALLS_PAGE_SIZE = 25;
let allCalls = [];
let callsPage = 1;
let callsAddressNames = {};

function renderCalls(calls, addressNames = {}) {
    const list = calls?.calls || calls?.activities || [];
    allCalls = list;
    callsAddressNames = addressNames;
    callsPage = 1;
    renderCallsPage();
}

function renderCallsPage() {
    const tbody = document.getElementById('callsTable');
    if (allCalls.length === 0) {
        tbody.innerHTML = '<tr><td colspan="5" class="empty-state"><i class="fas fa-terminal"></i><div>No calls recorded yet</div></td></tr>';
        updateCallsPagination();
        return;
    }

    const start = (callsPage - 1) * CALLS_PAGE_SIZE;
    const pageItems = allCalls.slice(start, start + CALLS_PAGE_SIZE);

    tbody.innerHTML = pageItems.map(call => {
        const time = call.timestamp ? timeAgo(call.timestamp) : (call.slot !== undefined ? 'Slot ' + formatNumber(call.slot) : '--');
        const callerLabel = call.caller
            ? (callsAddressNames[call.caller] ? `${escapeHtml(callsAddressNames[call.caller])}.lichen` : formatHash(call.caller))
            : '--';
        const caller = call.caller
            ? '<a href="address.html?address=' + encodeURIComponent(call.caller) + '" class="table-link" style="font-family:\'JetBrains Mono\',monospace;font-size:0.85rem;" title="' + escapeHtml(call.caller) + '">' + callerLabel + '</a>'
            : '--';
        const fn_name = escapeHtml(call.function_name || call.function || call.method || '--');
        const fee = call.fee !== undefined ? formatLicn(call.fee) : (call.gas_used !== undefined ? formatNumber(call.gas_used) + ' spores' : '--');
        const status = call.success !== false
            ? '<span class="badge success" style="font-size:0.75rem;"><i class="fas fa-check"></i> OK</span>'
            : '<span class="badge" style="background:rgba(255,70,70,0.15);color:#ff4646;font-size:0.75rem;"><i class="fas fa-times"></i> Failed</span>';

        return '<tr>' +
            '<td>' + time + '</td>' +
            '<td>' + caller + '</td>' +
            '<td style="font-family:\'JetBrains Mono\',monospace;font-weight:600;">' + fn_name + '</td>' +
            '<td>' + fee + '</td>' +
            '<td>' + status + '</td>' +
            '</tr>';
    }).join('');

    updateCallsPagination();
}

function updateCallsPagination() {
    let paginationEl = document.getElementById('callsPagination');
    if (!paginationEl) {
        const panel = document.getElementById('panel-calls');
        if (panel) {
            paginationEl = document.createElement('div');
            paginationEl.id = 'callsPagination';
            paginationEl.className = 'tab-pagination';
            panel.appendChild(paginationEl);
        } else return;
    }

    if (allCalls.length <= CALLS_PAGE_SIZE) {
        paginationEl.style.display = 'none';
        return;
    }

    const totalPages = Math.ceil(allCalls.length / CALLS_PAGE_SIZE);
    paginationEl.style.display = 'flex';
    paginationEl.innerHTML =
        '<span class="pagination-info">Page ' + callsPage + ' of ' + totalPages + ' (' + allCalls.length + ' calls)</span>' +
        '<div class="pagination-btns">' +
        '<button class="btn btn-secondary btn-small" data-page-action="prev"' + (callsPage <= 1 ? ' disabled' : '') + '><i class="fas fa-arrow-left"></i> Prev</button>' +
        '<button class="btn btn-secondary btn-small" data-page-action="next"' + (callsPage >= totalPages ? ' disabled' : '') + '>Next <i class="fas fa-arrow-right"></i></button>' +
        '</div>';

    if (!paginationEl.dataset.bound) {
        paginationEl.addEventListener('click', (event) => {
            const button = event.target.closest('button[data-page-action]');
            if (!button || button.disabled) return;
            if (button.dataset.pageAction === 'prev') {
                callsPage = Math.max(1, callsPage - 1);
            } else {
                callsPage = Math.min(totalPages, callsPage + 1);
            }
            renderCallsPage();
        });
        paginationEl.dataset.bound = '1';
    }
}

// ── Events rendering (paginated) ─────────────────────────────────

const EVENTS_PAGE_SIZE = 25;
let allEvents = [];
let eventsPage = 1;

function renderEvents(events) {
    const list = events?.logs || events?.events || [];
    allEvents = list;
    eventsPage = 1;
    renderEventsPage();
}

function renderEventsPage() {
    const tbody = document.getElementById('eventsTable');
    if (allEvents.length === 0) {
        tbody.innerHTML = '<tr><td colspan="3" class="empty-state"><i class="fas fa-bell"></i><div>No events emitted</div></td></tr>';
        updateEventsPagination();
        return;
    }

    const start = (eventsPage - 1) * EVENTS_PAGE_SIZE;
    const pageItems = allEvents.slice(start, start + EVENTS_PAGE_SIZE);

    tbody.innerHTML = pageItems.map(evt => {
        const slot = evt.slot !== undefined ? '<a href="block.html?slot=' + evt.slot + '" class="table-link">' + formatNumber(evt.slot) + '</a>' : '--';
        const name = escapeHtml(evt.name || evt.event || '--');
        const data = typeof evt.data === 'object' ? JSON.stringify(evt.data) : (evt.data || '--');
        const dataDisplay = data.length > 80 ? data.slice(0, 80) + '...' : data;

        return '<tr>' +
            '<td>' + slot + '</td>' +
            '<td style="font-family:\'JetBrains Mono\',monospace;font-weight:600;color:var(--text-primary);">' + name + '</td>' +
            '<td style="font-family:\'JetBrains Mono\',monospace;font-size:0.85rem;max-width:400px;overflow:hidden;text-overflow:ellipsis;">' + escapeHtml(dataDisplay) + '</td>' +
            '</tr>';
    }).join('');

    updateEventsPagination();
}

function updateEventsPagination() {
    let paginationEl = document.getElementById('eventsPagination');
    if (!paginationEl) {
        const panel = document.getElementById('panel-events');
        if (panel) {
            paginationEl = document.createElement('div');
            paginationEl.id = 'eventsPagination';
            paginationEl.className = 'tab-pagination';
            panel.appendChild(paginationEl);
        } else return;
    }

    if (allEvents.length <= EVENTS_PAGE_SIZE) {
        paginationEl.style.display = 'none';
        return;
    }

    const totalPages = Math.ceil(allEvents.length / EVENTS_PAGE_SIZE);
    paginationEl.style.display = 'flex';
    paginationEl.innerHTML =
        '<span class="pagination-info">Page ' + eventsPage + ' of ' + totalPages + ' (' + allEvents.length + ' events)</span>' +
        '<div class="pagination-btns">' +
        '<button class="btn btn-secondary btn-small" data-page-action="prev"' + (eventsPage <= 1 ? ' disabled' : '') + '><i class="fas fa-arrow-left"></i> Prev</button>' +
        '<button class="btn btn-secondary btn-small" data-page-action="next"' + (eventsPage >= totalPages ? ' disabled' : '') + '>Next <i class="fas fa-arrow-right"></i></button>' +
        '</div>';

    if (!paginationEl.dataset.bound) {
        paginationEl.addEventListener('click', (event) => {
            const button = event.target.closest('button[data-page-action]');
            if (!button || button.disabled) return;
            if (button.dataset.pageAction === 'prev') {
                eventsPage = Math.max(1, eventsPage - 1);
            } else {
                eventsPage = Math.min(totalPages, eventsPage + 1);
            }
            renderEventsPage();
        });
        paginationEl.dataset.bound = '1';
    }
}

// ── Init ─────────────────────────────────────────────────────────

function initSearch() {
    const input = document.getElementById('searchInput');
    if (!input) return;
    input.addEventListener('keydown', async (e) => {
        if (e.key === 'Enter') {
            const q = input.value.trim();
            if (!q) return;
            if (typeof navigateExplorerSearch === 'function') {
                await navigateExplorerSearch(q);
                return;
            }
            window.location.href = 'address.html?address=' + q;
        }
    });
}

document.addEventListener('DOMContentLoaded', () => {
    if (typeof initExplorerNetworkSelector === 'function') initExplorerNetworkSelector();
    initSearch();
    const navToggle = document.getElementById('navToggle');
    const navMenu = document.querySelector('.nav-menu');
    const navActions = document.querySelector('.nav-actions');
    const navContainer = document.querySelector('.nav-container');
    if (navToggle && navMenu) {
        navToggle.addEventListener('click', () => {
            const isOpen = !navMenu.classList.contains('active');
            navMenu.classList.toggle('active', isOpen);
            navMenu.classList.toggle('open', isOpen);
            navActions?.classList.toggle('active', isOpen);
            navActions?.classList.toggle('open', isOpen);
            navToggle.classList.toggle('active', isOpen);
            navContainer?.style.setProperty('--nav-menu-height', isOpen ? `${navMenu.offsetHeight}px` : '0px');
        });
    }
    loadContract();
});
