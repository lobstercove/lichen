// shared-config.js — Unified frontend configuration for Lichen
// Single source of truth for environment, network endpoints, and cross-app URLs.
//
// Load order in every HTML page:
//   1. shared/env.js        — sets window.LICHEN_ENV ('development' | 'production')
//   2. shared-config.js     — this file (reads env, builds LICHEN_CONFIG)
//   3. app-specific scripts — read LICHEN_CONFIG.networks, LICHEN_CONFIG.rpc(), etc.
//
// On VPS: set window.LICHEN_ENV = 'production' in env.js
// Locally: leave it as 'development' (default)

const LICHEN_CONFIG = (() => {
    // ── Environment Detection ───────────────────────────────────────────
    const hostname = window.location.hostname;
    const isLocalhost = hostname === 'localhost' || hostname === '127.0.0.1';
    const env = window.LICHEN_ENV || (isLocalhost ? 'development' : 'production');
    const isProduction = env === 'production';

    // ── Network Definitions ─────────────────────────────────────────────
    const networks = {
        mainnet: {
            label: 'Mainnet',
            rpc: 'https://rpc.lichen.network',
            ws: 'wss://ws.lichen.network',
            local: false,
        },
        testnet: {
            label: 'Testnet',
            rpc: 'https://testnet-rpc.lichen.network',
            ws: 'wss://testnet-ws.lichen.network',
            local: false,
        },
        'local-testnet': {
            label: 'Local Testnet',
            rpc: 'http://localhost:8899',
            ws: 'ws://localhost:8900',
            local: true,
        },
        'local-mainnet': {
            label: 'Local Mainnet',
            rpc: 'http://localhost:9899',
            ws: 'ws://localhost:9900',
            local: true,
        },
    };

    // ── Visible Networks (production hides local-*) ─────────────────────
    const visibleNetworks = {};
    for (const [key, net] of Object.entries(networks)) {
        if (!isProduction || !net.local) {
            visibleNetworks[key] = net;
        }
    }

    // ── Default Network ─────────────────────────────────────────────────
    const defaultNetwork = isProduction ? 'mainnet' : 'local-testnet';

    // ── Cross-App URLs ──────────────────────────────────────────────────
    let apps;
    if (isLocalhost) {
        apps = {
            explorer: 'http://localhost:3007',
            wallet: 'http://localhost:3008',
            marketplace: 'http://localhost:3009',
            dex: 'http://localhost:3011',
            website: 'http://localhost:9090',
            developers: 'http://localhost:3010',
            programs: 'http://localhost:3012',
            faucet: 'http://localhost:9100',
        };
    } else {
        apps = {
            explorer: 'https://explorer.lichen.network',
            wallet: 'https://wallet.lichen.network',
            marketplace: 'https://marketplace.lichen.network',
            dex: 'https://dex.lichen.network',
            website: 'https://lichen.network',
            developers: 'https://developers.lichen.network',
            programs: 'https://programs.lichen.network',
            faucet: 'https://faucet.lichen.network',
        };
    }

    // ── Helpers ─────────────────────────────────────────────────────────

    /** Resolve a network key, falling back to the default if invalid. */
    function resolveNetwork(name) {
        if (name === 'local') return networks['local-testnet'] ? 'local-testnet' : defaultNetwork;
        return networks[name] ? name : defaultNetwork;
    }

    /** Get RPC URL for a given network (or current). */
    function rpc(networkKey) {
        const key = resolveNetwork(networkKey || currentNetwork());
        return networks[key].rpc;
    }

    /** Get WS URL for a given network (or current). */
    function ws(networkKey) {
        const key = resolveNetwork(networkKey || currentNetwork());
        return networks[key].ws;
    }

    /** Read the current network from a given localStorage key, with fallback. */
    function currentNetwork(storageKey) {
        if (storageKey) {
            const saved = localStorage.getItem(storageKey);
            if (saved) return resolveNetwork(saved);
        }
        return defaultNetwork;
    }

    /**
     * Populate a <select> element with the visible networks.
     * @param {string|HTMLElement} selectOrId — select element or its ID
     * @param {string} storageKey — localStorage key for persisting selection
     * @param {function} [onChange] — callback(networkKey, config) on change
     */
    function initNetworkSelector(selectOrId, storageKey, onChange) {
        const select = typeof selectOrId === 'string'
            ? document.getElementById(selectOrId)
            : selectOrId;
        if (!select) return;

        // Clear hardcoded options and rebuild from config
        select.innerHTML = '';
        for (const [key, net] of Object.entries(visibleNetworks)) {
            const opt = document.createElement('option');
            opt.value = key;
            opt.textContent = net.label;
            select.appendChild(opt);
        }

        // Restore saved selection (or use default)
        const saved = storageKey ? localStorage.getItem(storageKey) : null;
        const initial = resolveNetwork(saved || defaultNetwork);
        // If saved network is local but we're in production, reset to default
        if (isProduction && networks[initial]?.local) {
            select.value = defaultNetwork;
            if (storageKey) localStorage.setItem(storageKey, defaultNetwork);
        } else {
            select.value = initial;
        }

        select.addEventListener('change', () => {
            const key = resolveNetwork(select.value);
            if (storageKey) localStorage.setItem(storageKey, key);
            if (typeof onChange === 'function') {
                onChange(key, networks[key]);
            }
        });

        return select;
    }

    // ── Public API ──────────────────────────────────────────────────────
    return {
        // Environment
        env,
        isProduction,
        isDev: !isProduction,

        // Networks
        networks,
        visibleNetworks,
        defaultNetwork,
        resolveNetwork,

        // Endpoints
        rpc,
        ws,

        // Network selection
        currentNetwork,
        initNetworkSelector,

        // Cross-app URLs
        ...apps,
    };
})();

// ── Auto-resolve cross-app nav links ────────────────────────────────────
document.addEventListener('DOMContentLoaded', () => {
    document.querySelectorAll('a[data-lichen-app]').forEach(link => {
        const app = link.dataset.lichenApp;
        const path = link.dataset.lichenPath || '';
        if (LICHEN_CONFIG[app]) {
            link.href = LICHEN_CONFIG[app] + path;
        }
    });
});
