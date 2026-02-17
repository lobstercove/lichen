// ============================================================
// MoltChain Developer Portal — Core JavaScript
// Pure vanilla JS, no external dependencies
// ============================================================

document.addEventListener('DOMContentLoaded', () => {
    initSidebar();
    initScrollSpy();
    initCodeCopy();
    initLangTabs();
    initSearch();
    initNetworkSelector();
    initNavHighlight();
});


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

            navigator.clipboard.writeText(text).then(() => {
                const originalText = btn.textContent;
                btn.textContent = 'Copied!';
                btn.classList.add('copied');

                setTimeout(() => {
                    btn.textContent = originalText;
                    btn.classList.remove('copied');
                }, 2000);
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
                    btn.textContent = 'Copied!';
                    btn.classList.add('copied');
                    setTimeout(() => {
                        btn.textContent = 'Copy';
                        btn.classList.remove('copied');
                    }, 2000);
                } catch (_) { /* silently fail */ }
                document.body.removeChild(textarea);
            });
        });
    });
}


// ============================================================
// 4. LANGUAGE TABS — Switch between JS/Python/Rust/CLI
// ============================================================

const LANG_STORAGE_KEY = 'moltdev_lang';

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
    { title: 'Installation', desc: 'Install the MoltChain SDK and CLI tools', url: 'getting-started.html#installation', category: 'Guide' },
    { title: 'Hello World Tutorial', desc: 'Build your first MoltChain application', url: 'getting-started.html#hello-world', category: 'Tutorial' },
    { title: 'Network Configuration', desc: 'Connect to devnet, testnet, or mainnet', url: 'getting-started.html#network-config', category: 'Guide' },

    // Architecture
    { title: 'Architecture Overview', desc: 'Understanding MoltChain\'s design and components', url: 'architecture.html', category: 'Concepts' },
    { title: 'Consensus Mechanism', desc: 'Proof-of-Evolution consensus explained', url: 'architecture.html#consensus', category: 'Concepts' },
    { title: 'Transaction Lifecycle', desc: 'How transactions flow through the network', url: 'architecture.html#tx-lifecycle', category: 'Concepts' },
    { title: 'Block Structure', desc: 'Understanding block headers and body format', url: 'architecture.html#block-structure', category: 'Concepts' },
    { title: 'State Model', desc: 'Account-based state management', url: 'architecture.html#state-model', category: 'Concepts' },
    { title: 'Mempool', desc: 'Transaction pool and ordering mechanics', url: 'architecture.html#mempool', category: 'Concepts' },

    // JSON-RPC API
    { title: 'JSON-RPC API Reference', desc: 'Complete list of all RPC endpoints', url: 'rpc-reference.html', category: 'API' },
    { title: 'molt_getBalance', desc: 'Get account balance by address', url: 'rpc-reference.html#getBalance', category: 'API' },
    { title: 'molt_getBlock', desc: 'Retrieve a block by number or hash', url: 'rpc-reference.html#getBlock', category: 'API' },
    { title: 'molt_sendTransaction', desc: 'Submit a signed transaction', url: 'rpc-reference.html#sendTransaction', category: 'API' },
    { title: 'molt_getTransactionReceipt', desc: 'Get transaction receipt by hash', url: 'rpc-reference.html#getTransactionReceipt', category: 'API' },
    { title: 'molt_call', desc: 'Execute a read-only smart contract call', url: 'rpc-reference.html#call', category: 'API' },
    { title: 'molt_estimateGas', desc: 'Estimate gas for a transaction', url: 'rpc-reference.html#estimateGas', category: 'API' },
    { title: 'molt_subscribe', desc: 'Subscribe to real-time events via WebSocket', url: 'ws-reference.html#subscribe', category: 'API' },
    { title: 'molt_getCode', desc: 'Get contract bytecode at an address', url: 'rpc-reference.html#getCode', category: 'API' },
    { title: 'molt_getLogs', desc: 'Query event logs with filters', url: 'rpc-reference.html#getLogs', category: 'API' },

    // SDK
    { title: 'JavaScript SDK', desc: 'Client library for Node.js and browsers', url: 'sdk-js.html', category: 'SDK' },
    { title: 'Python SDK', desc: 'Client library for Python applications', url: 'sdk-python.html', category: 'SDK' },
    { title: 'Rust SDK', desc: 'Native Rust client with async support', url: 'sdk-rust.html', category: 'SDK' },
    { title: 'Creating a Wallet', desc: 'Generate and manage keypairs', url: 'sdk-js.html#wallets', category: 'SDK' },
    { title: 'Sending Transactions', desc: 'Build, sign, and send transactions', url: 'sdk-js.html#transactions', category: 'SDK' },
    { title: 'Querying Balances', desc: 'Check token and MOLT balances', url: 'sdk-js.html#balances', category: 'SDK' },

    // Smart Contracts
    { title: 'Smart Contracts Overview', desc: 'Writing and deploying contracts on MoltChain', url: 'contracts.html', category: 'Contracts' },
    { title: 'Contract Development', desc: 'Tools and patterns for contract development', url: 'contracts.html#development', category: 'Contracts' },
    { title: 'Deploying Contracts', desc: 'Deploy smart contracts to MoltChain networks', url: 'contracts.html#deploy', category: 'Contracts' },
    { title: 'Contract Events', desc: 'Emitting and listening for events', url: 'contracts.html#events', category: 'Contracts' },
    { title: 'Contract Reference', desc: 'Full reference for all 27 on-chain contracts', url: 'contract-reference.html', category: 'Contracts' },
    { title: 'MoltyID Identity', desc: 'On-chain identity, naming, and reputation', url: 'moltyid.html', category: 'Contracts' },

    // CLI
    { title: 'CLI Reference', desc: 'Complete command-line tool documentation', url: 'cli-reference.html', category: 'CLI' },
    { title: 'molt init', desc: 'Initialize a new MoltChain project', url: 'cli-reference.html#init', category: 'CLI' },
    { title: 'molt deploy', desc: 'Deploy a contract to the network', url: 'cli-reference.html#deploy', category: 'CLI' },
    { title: 'molt test', desc: 'Run contract tests locally', url: 'cli-reference.html#test', category: 'CLI' },
    { title: 'molt keygen', desc: 'Generate new keypair', url: 'cli-reference.html#keygen', category: 'CLI' },
    { title: 'molt balance', desc: 'Check address balance', url: 'cli-reference.html#balance', category: 'CLI' },

    // Tokens
    { title: 'Token Standard (MRC-20)', desc: 'Fungible token standard specification', url: 'contract-reference.html#musd_token', category: 'Tokens' },
    { title: 'NFT Standard (MRC-721)', desc: 'Non-fungible token standard', url: 'contract-reference.html#moltpunks', category: 'Tokens' },
    { title: 'Creating a Token', desc: 'Step-by-step token creation guide', url: 'contracts.html#tokens', category: 'Tutorial' },

    // Validators
    { title: 'Running a Validator', desc: 'Set up and operate a validator node', url: 'validator.html', category: 'Validators' },
    { title: 'Staking Guide', desc: 'Stake MOLT and earn rewards', url: 'validator.html#staking', category: 'Validators' },
    { title: 'Validator Requirements', desc: 'Hardware and network requirements', url: 'validator.html#requirements', category: 'Validators' },

    // Tools
    { title: 'Block Explorer', desc: 'Browse blocks, transactions, and accounts', url: '../explorer/index.html', category: 'Tools' },
    { title: 'Faucet', desc: 'Get testnet MOLT tokens', url: '../faucet/index.html', category: 'Tools' },
    { title: 'Marketplace', desc: 'MoltChain NFT and token marketplace', url: '../marketplace/index.html', category: 'Tools' },
];

function initSearch() {
    const overlay = document.querySelector('.search-overlay');
    const input = document.querySelector('.search-modal-input');
    const resultsContainer = document.querySelector('.search-results');

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

    // Filter results on input
    input.addEventListener('input', () => {
        const query = input.value.trim().toLowerCase();
        if (query.length === 0) {
            renderResults(SEARCH_INDEX.slice(0, 8));
            return;
        }

        filteredResults = SEARCH_INDEX.filter(item => {
            const haystack = (item.title + ' ' + item.desc + ' ' + item.category).toLowerCase();
            return query.split(/\s+/).every(word => haystack.includes(word));
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


// ============================================================
// 6. NETWORK SELECTOR — devnet / testnet / mainnet
// ============================================================

const NETWORK_STORAGE_KEY = 'moltchain_dev_network';

const NETWORK_ENDPOINTS = {
    devnet:  'http://localhost:8899',
    testnet: 'https://testnet-rpc.moltchain.io',
    mainnet: 'https://rpc.moltchain.io'
};

const NETWORK_WS_ENDPOINTS = {
    devnet:  'ws://localhost:8900',
    testnet: 'wss://testnet-ws.moltchain.io',
    mainnet: 'wss://ws.moltchain.io'
};

function initNetworkSelector() {
    const selector = document.querySelector('.network-selector-dev');
    if (!selector) return;

    const options = selector.querySelectorAll('.network-option');
    if (options.length === 0) return;

    // Restore saved network or default to devnet
    const savedNetwork = localStorage.getItem(NETWORK_STORAGE_KEY) || 'devnet';
    setActiveNetwork(savedNetwork, options);

    options.forEach(option => {
        option.addEventListener('click', () => {
            const network = option.getAttribute('data-network');
            if (!network) return;

            localStorage.setItem(NETWORK_STORAGE_KEY, network);
            setActiveNetwork(network, options);
        });
    });
}

function setActiveNetwork(network, options) {
    // Update button states
    options.forEach(opt => {
        opt.classList.toggle('active', opt.getAttribute('data-network') === network);
    });

    // Update endpoint displays on the page
    document.querySelectorAll('.endpoint-display[data-type="rpc"]').forEach(el => {
        el.textContent = NETWORK_ENDPOINTS[network] || NETWORK_ENDPOINTS.devnet;
    });

    document.querySelectorAll('.endpoint-display[data-type="ws"]').forEach(el => {
        el.textContent = NETWORK_WS_ENDPOINTS[network] || NETWORK_WS_ENDPOINTS.devnet;
    });

    // Update any generic endpoint display
    document.querySelectorAll('.endpoint-display:not([data-type])').forEach(el => {
        el.textContent = NETWORK_ENDPOINTS[network] || NETWORK_ENDPOINTS.devnet;
    });

    // Dispatch custom event for other scripts to react
    document.dispatchEvent(new CustomEvent('moltchain:networkChange', {
        detail: {
            network: network,
            rpcEndpoint: NETWORK_ENDPOINTS[network],
            wsEndpoint: NETWORK_WS_ENDPOINTS[network]
        }
    }));
}

/**
 * Get the currently selected network.
 * @returns {{ network: string, rpc: string, ws: string }}
 */
function getActiveNetwork() {
    const network = localStorage.getItem(NETWORK_STORAGE_KEY) || 'devnet';
    return {
        network,
        rpc: NETWORK_ENDPOINTS[network] || NETWORK_ENDPOINTS.devnet,
        ws: NETWORK_WS_ENDPOINTS[network] || NETWORK_WS_ENDPOINTS.devnet
    };
}

// Expose globally for other scripts
window.moltchainNetwork = { getActiveNetwork, NETWORK_ENDPOINTS, NETWORK_WS_ENDPOINTS };


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

        if (currentPath === linkPath || currentPath.startsWith(linkPath.replace(/index\.html$/, ''))) {
            link.classList.add('active');
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
