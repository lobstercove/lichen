// ============================================================
// Lichen Developer Portal — Core JavaScript
// Pure vanilla JS, no external dependencies
// ============================================================

document.addEventListener('DOMContentLoaded', () => {
    ensureDeveloperNavLinks();
    rewriteProgramsLinks();
    initSidebar();
    initScrollSpy();
    initCodeCopy();
    initLangTabs();
    initSearch();
    initNetworkSelector();
    initNavHighlight();
    initMobileNav();
});

function ensureDeveloperNavLinks() {
    const requiredLinks = [
        { href: 'architecture.html', label: 'Architecture' },
        { href: 'validator.html', label: 'Validator' },
        { href: 'changelog.html', label: 'Changelog' }
    ];

    document.querySelectorAll('.nav-menu').forEach((menu) => {
        requiredLinks.forEach((linkDef) => {
            const exists = Array.from(menu.querySelectorAll('a')).some(
                (anchor) => anchor.getAttribute('href') === linkDef.href
            );
            if (exists) return;

            const li = document.createElement('li');
            const anchor = document.createElement('a');
            anchor.href = linkDef.href;
            anchor.textContent = linkDef.label;
            li.appendChild(anchor);
            menu.appendChild(li);
        });
    });
}

function rewriteProgramsLinks() {
    if (typeof LICHEN_CONFIG === 'undefined' || !LICHEN_CONFIG.programs) return;

    document.querySelectorAll('a[href^="../programs/"]').forEach((anchor) => {
        const href = anchor.getAttribute('href') || '';
        const path = href.replace(/^\.\.\/programs/, '');
        anchor.href = LICHEN_CONFIG.programs + path;
    });

    document.querySelectorAll('iframe[src^="../programs/"]').forEach((frame) => {
        const src = frame.getAttribute('src') || '';
        const path = src.replace(/^\.\.\/programs/, '');
        frame.src = LICHEN_CONFIG.programs + path;
    });
}


// ============================================================
// 1. SIDEBAR TOGGLE — Mobile hamburger menu
// ============================================================

function initSidebar() {
    const sidebar = document.querySelector('.docs-sidebar');
    const toggle = document.querySelector('.sidebar-toggle');
    const backdrop = document.querySelector('.sidebar-backdrop');

    if (!sidebar) return;

    // Toggle button
    if (toggle) {
        toggle.addEventListener('click', () => {
            sidebar.classList.toggle('open');
            if (backdrop) backdrop.classList.toggle('active');
        });
    }

    // Close on backdrop click
    if (backdrop) {
        backdrop.addEventListener('click', () => {
            sidebar.classList.remove('open');
            backdrop.classList.remove('active');
        });
    }

    // Close sidebar on link click (mobile)
    sidebar.querySelectorAll('.sidebar-link').forEach(link => {
        link.addEventListener('click', () => {
            if (window.innerWidth <= 768) {
                sidebar.classList.remove('open');
                if (backdrop) backdrop.classList.remove('active');
            }
        });
    });

    // Collapsible sidebar sections
    sidebar.querySelectorAll('.sidebar-group-toggle').forEach(btn => {
        btn.addEventListener('click', () => {
            const links = btn.nextElementSibling;
            if (!links || !links.classList.contains('sidebar-links')) return;

            btn.classList.toggle('collapsed');
            if (links.classList.contains('collapsed')) {
                links.classList.remove('collapsed');
                links.style.maxHeight = links.scrollHeight + 'px';
            } else {
                links.style.maxHeight = links.scrollHeight + 'px';
                // Force reflow before collapsing
                links.offsetHeight; // eslint-disable-line no-unused-expressions
                links.classList.add('collapsed');
                links.style.maxHeight = '0';
            }
        });

        // Initialize open sections with proper max-height
        const links = btn.nextElementSibling;
        if (links && links.classList.contains('sidebar-links') && !links.classList.contains('collapsed')) {
            links.style.maxHeight = links.scrollHeight + 'px';
        }
    });

    // Smooth scroll for sidebar links pointing to anchors
    sidebar.querySelectorAll('.sidebar-link[href^="#"]').forEach(link => {
        link.addEventListener('click', (e) => {
            const targetId = link.getAttribute('href').slice(1);
            const target = document.getElementById(targetId);
            if (target) {
                e.preventDefault();
                target.scrollIntoView({ behavior: 'smooth', block: 'start' });
                history.pushState(null, '', '#' + targetId);
            }
        });
    });
}


// ============================================================
// 2. SCROLL SPY — Highlight active sidebar + TOC links
// ============================================================

function initScrollSpy() {
    const sidebarLinks = document.querySelectorAll('.sidebar-link[href^="#"]');
    const tocLinks = document.querySelectorAll('.toc-link[href^="#"]');

    if (sidebarLinks.length === 0 && tocLinks.length === 0) return;

    // Gather all target headings
    const allLinks = [...sidebarLinks, ...tocLinks];
    const headingIds = [...new Set(allLinks.map(l => l.getAttribute('href').slice(1)))];
    const headings = headingIds
        .map(id => document.getElementById(id))
        .filter(Boolean);

    if (headings.length === 0) return;

    let ticking = false;

    function updateActiveLink() {
        const scrollY = window.scrollY + 100; // offset for fixed nav
        let currentId = headings[0]?.id || '';

        for (const heading of headings) {
            if (heading.offsetTop <= scrollY) {
                currentId = heading.id;
            }
        }

        // Update sidebar links
        sidebarLinks.forEach(link => {
            const isActive = link.getAttribute('href') === '#' + currentId;
            link.classList.toggle('active', isActive);
        });

        // Update TOC links
        tocLinks.forEach(link => {
            const isActive = link.getAttribute('href') === '#' + currentId;
            link.classList.toggle('active', isActive);
        });

        ticking = false;
    }

    window.addEventListener('scroll', () => {
        if (!ticking) {
            requestAnimationFrame(updateActiveLink);
            ticking = true;
        }
    }, { passive: true });

    // Initial highlight
    updateActiveLink();
}


// ============================================================
// 3. CODE COPY — Copy code block contents to clipboard
// ============================================================

function initCodeCopy() {
    document.querySelectorAll('.code-copy-btn').forEach(btn => {
        btn.addEventListener('click', () => {
            const codeBlock = btn.closest('.code-block');
            if (!codeBlock) return;

            const code = codeBlock.querySelector('code');
            if (!code) return;

            const text = code.textContent;
            const originalHTML = btn.innerHTML;
            const hasIcon = btn.querySelector('i');

            function showCopied() {
                btn.innerHTML = hasIcon ? '<i class="fas fa-check"></i>' : 'Copied!';
                btn.classList.add('copied');
                setTimeout(() => {
                    btn.innerHTML = originalHTML;
                    btn.classList.remove('copied');
                }, 2000);
            }

            navigator.clipboard.writeText(text).then(() => {
                showCopied();
            }).catch(() => {
                // Fallback for older browsers
                const textarea = document.createElement('textarea');
                textarea.value = text;
                textarea.style.position = 'fixed';
                textarea.style.opacity = '0';
                document.body.appendChild(textarea);
                textarea.select();
                try {
                    document.execCommand('copy');
                    showCopied();
                } catch (_) { /* silently fail */ }
                document.body.removeChild(textarea);
            });
        });
    });
}


// ============================================================
// 4. LANGUAGE TABS — Switch between JS/Python/Rust/CLI
// ============================================================

const LANG_STORAGE_KEY = 'lichendev_lang';

function initLangTabs() {
    const tabGroups = document.querySelectorAll('.lang-tabs');
    if (tabGroups.length === 0) return;

    // Restore saved language preference
    const savedLang = localStorage.getItem(LANG_STORAGE_KEY);

    tabGroups.forEach(group => {
        const tabs = group.querySelectorAll('.lang-tab');
        // Find the tab content container (sibling or parent's children)
        const parent = group.parentElement;
        if (!parent) return;

        const contents = parent.querySelectorAll('.lang-tab-content');

        // Apply saved preference or default to first tab
        if (savedLang) {
            const savedTab = group.querySelector(`.lang-tab[data-lang="${savedLang}"]`);
            if (savedTab) {
                activateTab(savedTab, tabs, contents);
            }
        }

        tabs.forEach(tab => {
            tab.addEventListener('click', () => {
                const lang = tab.getAttribute('data-lang');
                activateTab(tab, tabs, contents);

                // Persist choice
                if (lang) {
                    localStorage.setItem(LANG_STORAGE_KEY, lang);
                }

                // Sync all tab groups on the page to the same language
                syncAllTabGroups(lang);
            });
        });
    });

    function activateTab(activeTab, allTabs, allContents) {
        const lang = activeTab.getAttribute('data-lang');

        allTabs.forEach(t => t.classList.remove('active'));
        activeTab.classList.add('active');

        allContents.forEach(c => {
            c.classList.toggle('active', c.getAttribute('data-lang') === lang);
        });
    }

    function syncAllTabGroups(lang) {
        if (!lang) return;
        tabGroups.forEach(group => {
            const tab = group.querySelector(`.lang-tab[data-lang="${lang}"]`);
            if (!tab || tab.classList.contains('active')) return;

            const parent = group.parentElement;
            if (!parent) return;

            const tabs = group.querySelectorAll('.lang-tab');
            const contents = parent.querySelectorAll('.lang-tab-content');
            activateTab(tab, tabs, contents);
        });
    }
}


// ============================================================
// 5. SEARCH OVERLAY — Cmd+K / Ctrl+K search modal
// ============================================================

// Search index — major pages and sections
const SEARCH_INDEX = [
    // Getting Started
    { title: 'Quick Start Guide', desc: 'Set up your development environment in minutes', url: 'getting-started.html', category: 'Guide' },
    { title: 'Install CLI', desc: 'Install the Lichen SDK and CLI tools', url: 'getting-started.html#install-cli', category: 'Guide' },
    { title: 'First Transfer Tutorial', desc: 'Send your first Lichen transaction', url: 'getting-started.html#first-transfer', category: 'Tutorial' },
    { title: 'Create Wallet', desc: 'Generate a wallet and get testnet tokens', url: 'getting-started.html#create-wallet', category: 'Guide' },

    // Architecture
    { title: 'Architecture Overview', desc: 'Understanding Lichen\'s design and components', url: 'architecture.html', category: 'Concepts' },
    { title: 'Consensus Mechanism', desc: 'Proof-of-Evolution consensus explained', url: 'architecture.html#consensus', category: 'Concepts' },
    { title: 'Transaction Lifecycle', desc: 'How transactions flow through the network', url: 'architecture.html#tx-lifecycle', category: 'Concepts' },
    { title: 'WASM Runtime', desc: 'WebAssembly smart contract execution engine', url: 'architecture.html#wasm', category: 'Concepts' },
    { title: 'State Model', desc: 'Account-based state management', url: 'architecture.html#state', category: 'Concepts' },
    { title: 'LichenID Integration', desc: 'On-chain identity and reputation system', url: 'architecture.html#lichenid-integration', category: 'Concepts' },

    // JSON-RPC API
    { title: 'JSON-RPC API Reference', desc: 'Complete list of all RPC endpoints', url: 'rpc-reference.html', category: 'API' },
    { title: 'licn_getBalance', desc: 'Get account balance by address', url: 'rpc-reference.html#getBalance', category: 'API' },
    { title: 'licn_getBlock', desc: 'Retrieve a block by number or hash', url: 'rpc-reference.html#getBlock', category: 'API' },
    { title: 'licn_sendTransaction', desc: 'Submit a signed transaction', url: 'rpc-reference.html#sendTransaction', category: 'API' },
    { title: 'licn_getTransaction', desc: 'Get transaction details by hash', url: 'rpc-reference.html#getTransaction', category: 'API' },
    { title: 'licn_simulateTransaction', desc: 'Simulate a transaction without sending', url: 'rpc-reference.html#simulateTransaction', category: 'API' },
    { title: 'licn_getAccountInfo', desc: 'Get full account info including data', url: 'rpc-reference.html#getAccountInfo', category: 'API' },
    { title: 'WebSocket Subscriptions', desc: 'Subscribe to real-time events via WebSocket', url: 'ws-reference.html#subscribeSlots', category: 'API' },
    { title: 'licn_getContractInfo', desc: 'Get contract info and ABI', url: 'rpc-reference.html#getContractInfo', category: 'API' },
    { title: 'licn_getContractLogs', desc: 'Query contract event logs', url: 'rpc-reference.html#getContractLogs', category: 'API' },

    // SDK
    { title: 'JavaScript SDK', desc: 'Client library for Node.js and browsers', url: 'sdk-js.html', category: 'SDK' },
    { title: 'Python SDK', desc: 'Client library for Python applications', url: 'sdk-python.html', category: 'SDK' },
    { title: 'Rust SDK', desc: 'Native Rust client with async support', url: 'sdk-rust.html', category: 'SDK' },
    { title: 'Keypair Generation', desc: 'Generate and manage keypairs', url: 'sdk-js.html#keypair-generate', category: 'SDK' },
    { title: 'Transaction Builder', desc: 'Build, sign, and send transactions', url: 'sdk-js.html#transaction-builder', category: 'SDK' },
    { title: 'Account Methods', desc: 'Query account balances and info', url: 'sdk-js.html#connection-account', category: 'SDK' },

    // Smart Contracts
    { title: 'Smart Contracts Overview', desc: 'Writing and deploying contracts on Lichen', url: 'contracts.html', category: 'Contracts' },
    { title: 'Contract Setup', desc: 'Tools and patterns for contract development', url: 'contracts.html#setup', category: 'Contracts' },
    { title: 'Deploying Contracts', desc: 'Deploy smart contracts to Lichen networks', url: 'contracts.html#deploy', category: 'Contracts' },
    { title: 'Cross-Contract Calls', desc: 'Call between contracts on-chain', url: 'contracts.html#crosscall', category: 'Contracts' },
    { title: 'Contract Reference', desc: 'Full reference for all 27 on-chain contracts', url: 'contract-reference.html', category: 'Contracts' },
    { title: 'LichenID Identity', desc: 'On-chain identity, naming, and reputation', url: 'lichenid.html', category: 'Contracts' },

    // CLI
    { title: 'CLI Reference', desc: 'Complete command-line tool documentation', url: 'cli-reference.html', category: 'CLI' },
    { title: 'lichen call', desc: 'Invoke a smart contract function', url: 'cli-reference.html#call', category: 'CLI' },
    { title: 'lichen deploy', desc: 'Deploy a contract to the network', url: 'cli-reference.html#deploy', category: 'CLI' },
    { title: 'lichen transfer', desc: 'Send LICN between addresses', url: 'cli-reference.html#transfer', category: 'CLI' },
    { title: 'lichen wallet-create', desc: 'Generate a new wallet keypair', url: 'cli-reference.html#wallet-create', category: 'CLI' },
    { title: 'lichen balance', desc: 'Check address balance', url: 'cli-reference.html#balance', category: 'CLI' },

    // Tokens
    { title: 'Token Standard (MRC-20)', desc: 'Fungible token standard specification', url: 'contract-reference.html#lusd-token', category: 'Tokens' },
    { title: 'NFT Standard (MRC-721)', desc: 'Non-fungible token standard', url: 'contract-reference.html#lichenpunks', category: 'Tokens' },
    { title: 'Creating a Token', desc: 'Step-by-step token creation guide', url: 'contracts.html#token', category: 'Tutorial' },

    // Validators
    { title: 'Running a Validator', desc: 'Set up and operate a validator node', url: 'validator.html', category: 'Validators' },
    { title: 'Staking Guide', desc: 'Stake LICN and earn rewards', url: 'validator.html#staking', category: 'Validators' },
    { title: 'Validator Requirements', desc: 'Hardware and network requirements', url: 'validator.html#requirements', category: 'Validators' },

    // WebSocket / Realtime
    { title: 'WebSocket Reference', desc: 'Realtime subscriptions and event payloads', url: 'ws-reference.html', category: 'API' },
    { title: 'subscribeSlots', desc: 'Subscribe to slot updates over WebSocket', url: 'ws-reference.html#subscribeSlots', category: 'API' },
    { title: 'subscribeBlocks', desc: 'Subscribe to finalized block notifications', url: 'ws-reference.html#subscribeBlocks', category: 'API' },
    { title: 'subscribeTransactions', desc: 'Subscribe to transaction stream updates', url: 'ws-reference.html#subscribeTransactions', category: 'API' },
    { title: 'subscribeAccount', desc: 'Subscribe to account state changes', url: 'ws-reference.html#subscribeAccount', category: 'API' },
    { title: 'subscribeLogs', desc: 'Subscribe to contract/runtime logs', url: 'ws-reference.html#subscribeLogs', category: 'API' },
    { title: 'subscribeBridgeLocks', desc: 'Subscribe to bridge lock events', url: 'ws-reference.html#subscribeBridgeLocks', category: 'API' },
    { title: 'subscribeBridgeMints', desc: 'Subscribe to bridge mint events', url: 'ws-reference.html#subscribeBridgeMints', category: 'API' },
    { title: 'subscribeSignatureStatus', desc: 'Track transaction confirmation status', url: 'ws-reference.html#subscribeSignatureStatus', category: 'API' },

    // Privacy & Identity
    { title: 'ZK Privacy Guide', desc: 'Shielded transfers, notes, nullifiers, and privacy model', url: 'zk-privacy.html', category: 'Privacy' },
    { title: 'Shielded Notes', desc: 'How encrypted notes and commitments are formed', url: 'zk-privacy.html#notes', category: 'Privacy' },
    { title: 'Nullifiers', desc: 'Preventing double-spends in shielded pools', url: 'zk-privacy.html#nullifiers', category: 'Privacy' },
    { title: 'Merkle Tree', desc: 'Merkle roots and path verification for private spends', url: 'zk-privacy.html#merkle', category: 'Privacy' },
    { title: 'LichenID Trust Tiers', desc: 'Identity reputation tiers and network effects', url: 'lichenid.html#trust-tiers', category: 'Identity' },
    { title: 'LichenID Name Service', desc: 'Register and resolve .lichen names', url: 'lichenid.html#name-service', category: 'Identity' },

    // Governance / Changelog / Ops
    { title: 'Changelog', desc: 'Release notes and platform changes', url: 'changelog.html', category: 'Ops' },
    { title: 'Validator Operations', desc: 'Validator setup, config and maintenance', url: 'validator.html#operations', category: 'Validators' },
    { title: 'Architecture Performance', desc: 'Throughput, finality, and runtime performance model', url: 'architecture.html#performance', category: 'Concepts' },

    // Playground / Contracts
    { title: 'Programs Playground', desc: 'Interactive browser IDE for contracts', url: 'playground.html#live-playground', category: 'Tools' },
    { title: 'Contract Reference', desc: 'Production contract catalog and interfaces', url: 'contract-reference.html', category: 'Contracts' },
    { title: 'LichenBridge Contract', desc: 'Bridge lock/mint program reference', url: 'contract-reference.html#lichenbridge', category: 'Contracts' },
    { title: 'DEX Contracts', desc: 'Core DEX programs, governance and rewards', url: 'contract-reference.html#dex-core', category: 'Contracts' },

    // Tools
    { title: 'Block Explorer', desc: 'Browse blocks, transactions, and accounts', url: (typeof LICHEN_CONFIG !== 'undefined' && LICHEN_CONFIG.explorer) || '../explorer/index.html', category: 'Tools' },
    { title: 'Faucet', desc: 'Get testnet LICN tokens', url: (typeof LICHEN_CONFIG !== 'undefined' && LICHEN_CONFIG.faucet) || '../faucet/index.html', category: 'Tools' },
    { title: 'Marketplace', desc: 'Lichen NFT and token marketplace', url: (typeof LICHEN_CONFIG !== 'undefined' && LICHEN_CONFIG.marketplace) || '../marketplace/index.html', category: 'Tools' },
];

function initSearch() {
    const overlay = document.querySelector('.search-overlay');
    const input = document.querySelector('.search-modal-input');
    const resultsContainer = document.querySelector('.search-results');
    const navInput = document.getElementById('searchInput');

    if (!overlay || !input || !resultsContainer) return;

    let selectedIndex = -1;
    let filteredResults = [];

    // Open on Cmd+K / Ctrl+K
    document.addEventListener('keydown', (e) => {
        if ((e.metaKey || e.ctrlKey) && e.key === 'k') {
            e.preventDefault();
            openSearch();
        }

        if (e.key === 'Escape' && overlay.classList.contains('active')) {
            closeSearch();
        }
    });

    // Close on backdrop click
    overlay.addEventListener('click', (e) => {
        if (e.target === overlay) {
            closeSearch();
        }
    });

    if (navInput) {
        navInput.addEventListener('focus', () => {
            openSearch();
            navInput.blur();
        });
    }

    // Filter results on input
    input.addEventListener('input', () => {
        const query = input.value.trim().toLowerCase();
        if (query.length === 0) {
            renderResults(SEARCH_INDEX.slice(0, 8));
            return;
        }

        const queryWords = normalizeSearchText(query).split(/\s+/).filter(Boolean);
        filteredResults = SEARCH_INDEX.filter(item => {
            const title = item.title || '';
            const rpcAlias = title.startsWith('licn_')
                ? ` ${title.replace(/^lichen_/, '')} ${title.replace(/^lichen_/, 'lichen ')}`
                : '';
            const haystack = normalizeSearchText(`${title} ${item.desc || ''} ${item.category || ''}${rpcAlias}`);
            return queryWords.every(word => haystack.includes(word));
        });

        selectedIndex = filteredResults.length > 0 ? 0 : -1;
        renderResults(filteredResults);
    });

    // Keyboard navigation in results
    input.addEventListener('keydown', (e) => {
        const items = resultsContainer.querySelectorAll('.search-result-item');

        if (e.key === 'ArrowDown') {
            e.preventDefault();
            selectedIndex = Math.min(selectedIndex + 1, items.length - 1);
            updateSelection(items);
        } else if (e.key === 'ArrowUp') {
            e.preventDefault();
            selectedIndex = Math.max(selectedIndex - 1, 0);
            updateSelection(items);
        } else if (e.key === 'Enter') {
            e.preventDefault();
            if (items[selectedIndex]) {
                const url = items[selectedIndex].getAttribute('data-url');
                if (url) window.location.href = url;
            }
        }
    });

    function openSearch() {
        overlay.classList.add('active');
        input.value = '';
        selectedIndex = -1;
        renderResults(SEARCH_INDEX.slice(0, 8));

        // Focus input after display transition
        requestAnimationFrame(() => input.focus());
    }

    function closeSearch() {
        overlay.classList.remove('active');
        input.value = '';
        resultsContainer.innerHTML = '';
    }

    function renderResults(results) {
        if (results.length === 0) {
            resultsContainer.innerHTML = '<div class="search-results-empty">No results found</div>';
            return;
        }

        resultsContainer.innerHTML = results.map((item, i) => `
            <a class="search-result-item${i === selectedIndex ? ' selected' : ''}" 
               href="${item.url}" data-url="${item.url}" data-index="${i}">
                <span class="search-result-category">${item.category}</span>
                <div class="search-result-text">
                    <div class="search-result-title">${highlightMatch(item.title, input.value)}</div>
                    <div class="search-result-desc">${item.desc}</div>
                </div>
            </a>
        `).join('');

        // Hover selection
        resultsContainer.querySelectorAll('.search-result-item').forEach((el, i) => {
            el.addEventListener('mouseenter', () => {
                selectedIndex = i;
                updateSelection(resultsContainer.querySelectorAll('.search-result-item'));
            });
        });
    }

    function updateSelection(items) {
        items.forEach((el, i) => {
            el.classList.toggle('selected', i === selectedIndex);
        });

        // Scroll selected item into view
        if (items[selectedIndex]) {
            items[selectedIndex].scrollIntoView({ block: 'nearest' });
        }
    }

    function highlightMatch(text, query) {
        if (!query.trim()) return text;
        const words = query.trim().split(/\s+/).filter(Boolean);
        let result = text;
        words.forEach(word => {
            const regex = new RegExp(`(${escapeRegex(word)})`, 'gi');
            result = result.replace(regex, '<strong>$1</strong>');
        });
        return result;
    }

    function escapeRegex(str) {
        return str.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
    }

    // Expose open function globally for search trigger buttons
    window.openDevSearch = openSearch;
}

function normalizeSearchText(text) {
    return String(text || '').toLowerCase().replace(/[^a-z0-9]+/g, ' ').trim();
}


// ============================================================
// 6. NETWORK SELECTOR — devnet / testnet / mainnet
// ============================================================

const NETWORK_STORAGE_KEY = 'lichen_dev_network';

// Network endpoints — centralized in shared-config.js (LICHEN_CONFIG)
const NETWORK_ENDPOINTS = {};
const NETWORK_WS_ENDPOINTS = {};
for (const [k, v] of Object.entries(LICHEN_CONFIG.networks)) {
    NETWORK_ENDPOINTS[k] = v.rpc;
    NETWORK_WS_ENDPOINTS[k] = v.ws;
}

function initNetworkSelector() {
    LICHEN_CONFIG.initNetworkSelector(
        document.getElementById('devNetworkSelect') || document.querySelector('.network-select'),
        NETWORK_STORAGE_KEY,
        (network, cfg) => {
            setActiveNetwork(network);
        }
    );

    // Set initial active network
    const savedNetwork = localStorage.getItem(NETWORK_STORAGE_KEY) || LICHEN_CONFIG.defaultNetwork;
    setActiveNetwork(LICHEN_CONFIG.resolveNetwork(savedNetwork));
}

function setActiveNetwork(network) {
    const resolvedNetwork = NETWORK_ENDPOINTS[network] ? network : LICHEN_CONFIG.defaultNetwork;

    // Update endpoint displays on the page
    document.querySelectorAll('.endpoint-display[data-type="rpc"]').forEach(el => {
        el.textContent = NETWORK_ENDPOINTS[resolvedNetwork] || LICHEN_CONFIG.rpc(resolvedNetwork);
    });

    document.querySelectorAll('.endpoint-display[data-type="ws"]').forEach(el => {
        el.textContent = NETWORK_WS_ENDPOINTS[resolvedNetwork] || LICHEN_CONFIG.ws(resolvedNetwork);
    });

    // Update any generic endpoint display
    document.querySelectorAll('.endpoint-display:not([data-type])').forEach(el => {
        el.textContent = NETWORK_ENDPOINTS[resolvedNetwork] || LICHEN_CONFIG.rpc(resolvedNetwork);
    });

    // Dispatch custom event for other scripts to react
    document.dispatchEvent(new CustomEvent('lichen:networkChange', {
        detail: {
            network: resolvedNetwork,
            rpcEndpoint: NETWORK_ENDPOINTS[resolvedNetwork],
            wsEndpoint: NETWORK_WS_ENDPOINTS[resolvedNetwork]
        }
    }));
}

/**
 * Get the currently selected network.
 * @returns {{ network: string, rpc: string, ws: string }}
 */
function getActiveNetwork() {
    const network = localStorage.getItem(NETWORK_STORAGE_KEY) || LICHEN_CONFIG.defaultNetwork;
    const resolvedNetwork = NETWORK_ENDPOINTS[network] ? network : LICHEN_CONFIG.defaultNetwork;
    return {
        network: resolvedNetwork,
        rpc: NETWORK_ENDPOINTS[resolvedNetwork] || LICHEN_CONFIG.rpc(resolvedNetwork),
        ws: NETWORK_WS_ENDPOINTS[resolvedNetwork] || LICHEN_CONFIG.ws(resolvedNetwork)
    };
}

// Expose globally for other scripts
window.lichenNetwork = { getActiveNetwork, NETWORK_ENDPOINTS, NETWORK_WS_ENDPOINTS };


// ============================================================
// 7. NAV HIGHLIGHTING — Mark current page in navigation
// ============================================================

function initNavHighlight() {
    const currentPath = window.location.pathname;

    // Highlight top nav links
    document.querySelectorAll('.nav a, .nav-link').forEach(link => {
        const href = link.getAttribute('href');
        if (!href) return;

        // Resolve the link href relative to current location
        const linkPath = new URL(href, window.location.origin + currentPath).pathname;

        if (currentPath === linkPath || currentPath.endsWith(linkPath)) {
            link.classList.add('active');
        } else {
            link.classList.remove('active');
        }
    });

    // Highlight sidebar links that point to pages (not anchors)
    document.querySelectorAll('.sidebar-link').forEach(link => {
        const href = link.getAttribute('href');
        if (!href || href.startsWith('#')) return;

        const linkPath = new URL(href, window.location.origin + currentPath).pathname;

        if (currentPath === linkPath) {
            link.classList.add('active');

            // Expand parent section if collapsed
            const section = link.closest('.sidebar-links');
            if (section && section.classList.contains('collapsed')) {
                section.classList.remove('collapsed');
                section.style.maxHeight = section.scrollHeight + 'px';
                const toggle = section.previousElementSibling;
                if (toggle && toggle.classList.contains('sidebar-group-toggle')) {
                    toggle.classList.remove('collapsed');
                }
            }
        }
    });
}


// ============================================================
// 8. TABLE OF CONTENTS — Auto-generate from headings
// ============================================================

/**
 * Call this on pages that have a .toc container to auto-populate
 * the table of contents from h2/h3 headings inside .docs-main.
 */
function generateTOC() {
    const toc = document.querySelector('.toc');
    const main = document.querySelector('.docs-main');
    if (!toc || !main) return;

    const headings = main.querySelectorAll('h2[id], h3[id]');
    if (headings.length === 0) return;

    // Build TOC title
    let html = '<div class="toc-title">On this page</div>';

    headings.forEach(heading => {
        const depth = heading.tagName === 'H3' ? ' depth-3' : '';
        html += `<a class="toc-link${depth}" href="#${heading.id}">${heading.textContent}</a>`;
    });

    toc.innerHTML = html;

    // Add smooth scroll behavior to TOC links
    toc.querySelectorAll('.toc-link').forEach(link => {
        link.addEventListener('click', (e) => {
            e.preventDefault();
            const targetId = link.getAttribute('href').slice(1);
            const target = document.getElementById(targetId);
            if (target) {
                target.scrollIntoView({ behavior: 'smooth', block: 'start' });
                history.pushState(null, '', '#' + targetId);
            }
        });
    });

    // Re-init scroll spy to include new TOC links
    initScrollSpy();
}

// Auto-generate if a .toc element exists
if (document.querySelector('.toc')) {
    document.addEventListener('DOMContentLoaded', generateTOC);
}

// Expose for manual invocation
window.generateTOC = generateTOC;

// ============================================================
// 9. MOBILE NAV TOGGLE — Top navigation hamburger
// ============================================================

function initMobileNav() {
    const navToggle = document.getElementById('navToggle');
    const navMenu = document.querySelector('.nav-menu');
    const navActions = document.querySelector('.nav-actions');
    const navContainer = document.querySelector('.nav-container');
    if (!navToggle || !navMenu) return;
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
