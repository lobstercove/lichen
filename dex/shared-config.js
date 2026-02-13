// shared-config.js — Frontend URL configuration for MoltChain
// Detects dev vs production and provides cross-app URL resolution.
// Include this script before other app scripts in each frontend HTML file.

const MOLT_CONFIG = (() => {
    const isDev = window.location.hostname === 'localhost' || window.location.hostname === '127.0.0.1';

    if (isDev) {
        return {
            explorer:    'http://localhost:3007',
            wallet:      'http://localhost:3008',
            marketplace: 'http://localhost:3009',
            website:     'http://localhost:9090',
            developers:  'http://localhost:3010',
            faucet:      'http://localhost:8901',
        };
    }

    // Production: all frontends served under subdirectories on the same origin
    const base = window.location.origin;
    return {
        explorer:    `${base}/explorer`,
        wallet:      `${base}/wallet`,
        marketplace: `${base}/marketplace`,
        website:     base,
        developers:  `${base}/developers`,
        faucet:      `${base}/faucet`,
    };
})();

// On DOMContentLoaded, resolve all cross-app navigation links tagged with data-molt-app
document.addEventListener('DOMContentLoaded', () => {
    document.querySelectorAll('a[data-molt-app]').forEach(link => {
        const app = link.dataset.moltApp;
        const path = link.dataset.moltPath || '';
        if (MOLT_CONFIG[app]) {
            link.href = MOLT_CONFIG[app] + path;
        }
    });
});
