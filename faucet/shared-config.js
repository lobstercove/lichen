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
            ws: 'wss://rpc.lichen.network/ws',
            local: false,
        },
        testnet: {
            label: 'Testnet',
            rpc: 'https://testnet-rpc.lichen.network',
            ws: 'wss://testnet-rpc.lichen.network/ws',
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
    const productionPrimaryNetwork = 'testnet';
    const visibleNetworks = {};
    for (const [key, net] of Object.entries(networks)) {
        if (!isProduction || (!net.local && key !== 'mainnet')) {
            visibleNetworks[key] = net;
        }
    }

    // ── Default Network ─────────────────────────────────────────────────
    const defaultNetwork = isProduction ? productionPrimaryNetwork : 'local-testnet';

    const INCIDENT_STATUS_RPC_METHOD = 'getIncidentStatus';
    const INCIDENT_BANNER_ID = 'lichen-incident-banner';
    const INCIDENT_NETWORK_STORAGE_KEYS = [
        'lichen_wallet_network',
        'lichen_mon_network',
        'lichen_website_network',
        'lichen_dex_network',
        'lichen_developers_network',
        'lichen_explorer_network',
        'lichen_marketplace_network',
        'lichen_programs_network',
        'lichen_faucet_network',
    ];
    let incidentBannerRequestId = 0;

    function hasSavedIncidentNetworkSelection() {
        if (window.LICHEN_INCIDENT_NETWORK_STORAGE_KEY) {
            const stored = localStorage.getItem(window.LICHEN_INCIDENT_NETWORK_STORAGE_KEY);
            if (stored) {
                return true;
            }
        }

        return INCIDENT_NETWORK_STORAGE_KEYS.some((key) => Boolean(localStorage.getItem(key)));
    }

    function shouldAutoRefreshIncidentStatusBanner() {
        return !isProduction || hasSavedIncidentNetworkSelection();
    }

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
        const resolved = networks[name] ? name : defaultNetwork;
        if (isProduction && resolved === 'mainnet') {
            return productionPrimaryNetwork;
        }
        return resolved;
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

    function incidentBannerNetwork(explicitNetwork) {
        if (explicitNetwork) {
            return resolveNetwork(explicitNetwork);
        }

        if (window.LICHEN_INCIDENT_NETWORK_STORAGE_KEY) {
            return currentNetwork(window.LICHEN_INCIDENT_NETWORK_STORAGE_KEY);
        }

        for (const key of INCIDENT_NETWORK_STORAGE_KEYS) {
            const saved = localStorage.getItem(key);
            if (saved) {
                return resolveNetwork(saved);
            }
        }

        return defaultNetwork;
    }

    async function incidentRpcCall(networkKey, method, params = []) {
        const response = await fetch(rpc(networkKey), {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({
                jsonrpc: '2.0',
                id: 'lichen-incident-status',
                method,
                params,
            }),
        });

        if (!response.ok) {
            throw new Error(`Incident status request failed with HTTP ${response.status}`);
        }

        const payload = await response.json();
        if (payload.error) {
            throw new Error(payload.error.message || 'Incident status request failed');
        }

        return payload.result;
    }

    function normalizeIncidentStatus(raw) {
        if (!raw || typeof raw !== 'object') {
            return null;
        }

        const components = [];
        if (raw.components && typeof raw.components === 'object') {
            for (const [name, component] of Object.entries(raw.components)) {
                if (!component || typeof component !== 'object') {
                    continue;
                }
                components.push({
                    name,
                    status: String(component.status || 'operational').trim().toLowerCase() || 'operational',
                    message: String(component.message || '').trim(),
                });
            }
        }

        return {
            mode: String(raw.mode || 'normal').trim().toLowerCase() || 'normal',
            severity: String(raw.severity || 'info').trim().toLowerCase() || 'info',
            bannerEnabled: Boolean(raw.banner_enabled),
            headline: String(raw.headline || '').trim(),
            summary: String(raw.summary || '').trim(),
            customerMessage: String(raw.customer_message || '').trim(),
            statusPageUrl: typeof raw.status_page_url === 'string' ? raw.status_page_url.trim() : '',
            actions: Array.isArray(raw.actions)
                ? raw.actions.map((value) => String(value || '').trim()).filter(Boolean)
                : [],
            components,
        };
    }

    function incidentBannerPalette(severity) {
        switch (severity) {
            case 'critical':
                return {
                    background: '#3a0f14',
                    backgroundAlt: '#54151d',
                    border: '#ff7b7b',
                    text: '#fff1f1',
                    accent: '#ffb3b3',
                };
            case 'high':
                return {
                    background: '#3b1908',
                    backgroundAlt: '#54220b',
                    border: '#ff9b54',
                    text: '#fff4eb',
                    accent: '#ffd2ae',
                };
            case 'warning':
                return {
                    background: '#332407',
                    backgroundAlt: '#46320a',
                    border: '#f2c14e',
                    text: '#fff8e6',
                    accent: '#ffe19a',
                };
            default:
                return {
                    background: '#11263d',
                    backgroundAlt: '#183452',
                    border: '#67c4ff',
                    text: '#eef8ff',
                    accent: '#b9e4ff',
                };
        }
    }

    function renderIncidentStatusBanner(status) {
        const existing = document.getElementById(INCIDENT_BANNER_ID);
        const shouldShow = Boolean(status)
            && (status.bannerEnabled || status.mode !== 'normal' || status.severity !== 'info');

        if (!shouldShow || !document.body || document.body.dataset.lichenIncidentBanner === 'off') {
            if (existing) {
                existing.remove();
            }
            return;
        }

        const palette = incidentBannerPalette(status.severity);
        const banner = existing || document.createElement('section');
        banner.id = INCIDENT_BANNER_ID;
        banner.setAttribute('role', 'status');
        banner.setAttribute('aria-live', 'polite');
        banner.style.background = `linear-gradient(135deg, ${palette.background} 0%, ${palette.backgroundAlt} 100%)`;
        banner.style.borderBottom = `1px solid ${palette.border}`;
        banner.style.color = palette.text;
        banner.style.padding = '14px 20px';
        banner.style.position = 'relative';
        banner.style.zIndex = '1000';
        banner.style.boxShadow = '0 10px 30px rgba(0, 0, 0, 0.18)';

        const container = document.createElement('div');
        container.style.maxWidth = '1200px';
        container.style.margin = '0 auto';
        container.style.display = 'flex';
        container.style.flexWrap = 'wrap';
        container.style.alignItems = 'flex-start';
        container.style.justifyContent = 'space-between';
        container.style.gap = '14px';

        const textBlock = document.createElement('div');
        textBlock.style.flex = '1 1 480px';
        textBlock.style.minWidth = '280px';

        const eyebrow = document.createElement('div');
        eyebrow.textContent = `Protocol status: ${status.mode.replace(/_/g, ' ')}`;
        eyebrow.style.fontSize = '0.76rem';
        eyebrow.style.textTransform = 'uppercase';
        eyebrow.style.letterSpacing = '0.08em';
        eyebrow.style.fontWeight = '700';
        eyebrow.style.color = palette.accent;

        const headline = document.createElement('div');
        headline.textContent = status.headline || 'Protocol status update';
        headline.style.fontSize = '1rem';
        headline.style.fontWeight = '700';
        headline.style.marginTop = '4px';

        const summary = document.createElement('div');
        summary.textContent = status.summary || status.customerMessage || 'Lichen is operating under an explicit incident-response mode.';
        summary.style.fontSize = '0.92rem';
        summary.style.lineHeight = '1.5';
        summary.style.marginTop = '6px';

        textBlock.appendChild(eyebrow);
        textBlock.appendChild(headline);
        textBlock.appendChild(summary);

        if (status.customerMessage && status.customerMessage !== summary.textContent) {
            const customerMessage = document.createElement('div');
            customerMessage.textContent = status.customerMessage;
            customerMessage.style.fontSize = '0.88rem';
            customerMessage.style.lineHeight = '1.5';
            customerMessage.style.marginTop = '6px';
            customerMessage.style.opacity = '0.94';
            textBlock.appendChild(customerMessage);
        }

        const detailBlock = document.createElement('div');
        detailBlock.style.flex = '0 1 360px';
        detailBlock.style.minWidth = '260px';
        detailBlock.style.display = 'grid';
        detailBlock.style.gap = '8px';

        const affectedComponents = status.components.filter((component) => component.status !== 'operational');
        if (affectedComponents.length > 0) {
            const componentRow = document.createElement('div');
            componentRow.style.display = 'flex';
            componentRow.style.flexWrap = 'wrap';
            componentRow.style.gap = '8px';

            for (const component of affectedComponents) {
                const badge = document.createElement('span');
                badge.textContent = `${component.name}: ${component.status}`;
                badge.style.padding = '5px 10px';
                badge.style.borderRadius = '999px';
                badge.style.border = `1px solid ${palette.border}`;
                badge.style.fontSize = '0.76rem';
                badge.style.fontWeight = '700';
                badge.style.textTransform = 'uppercase';
                badge.style.letterSpacing = '0.04em';
                badge.style.color = palette.accent;
                componentRow.appendChild(badge);
            }

            detailBlock.appendChild(componentRow);
        }

        if (status.actions.length > 0) {
            const actions = document.createElement('div');
            actions.textContent = `Actions: ${status.actions.join(' | ')}`;
            actions.style.fontSize = '0.84rem';
            actions.style.lineHeight = '1.5';
            actions.style.opacity = '0.94';
            detailBlock.appendChild(actions);
        }

        if (status.statusPageUrl) {
            const link = document.createElement('a');
            link.href = status.statusPageUrl;
            link.textContent = 'Open status details';
            link.style.color = palette.accent;
            link.style.fontSize = '0.84rem';
            link.style.fontWeight = '700';
            link.style.textDecoration = 'underline';
            detailBlock.appendChild(link);
        }

        banner.replaceChildren(container);
        container.appendChild(textBlock);
        container.appendChild(detailBlock);

        if (!existing) {
            document.body.insertBefore(banner, document.body.firstChild);
        }
    }

    async function refreshIncidentStatusBanner(networkKey) {
        if (!document.body || document.body.dataset.lichenIncidentBanner === 'off') {
            return null;
        }

        const requestId = ++incidentBannerRequestId;

        try {
            const status = normalizeIncidentStatus(
                await incidentRpcCall(incidentBannerNetwork(networkKey), INCIDENT_STATUS_RPC_METHOD),
            );
            if (requestId === incidentBannerRequestId) {
                renderIncidentStatusBanner(status);
            }
            return status;
        } catch (error) {
            console.warn('Could not load incident status:', error);
            return null;
        }
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
        refreshIncidentStatusBanner,
        shouldAutoRefreshIncidentStatusBanner,

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

    if (LICHEN_CONFIG.shouldAutoRefreshIncidentStatusBanner()) {
        void LICHEN_CONFIG.refreshIncidentStatusBanner();
    }
});
