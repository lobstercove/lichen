// LichenWallet Service Worker — Cache-first with auto-update
'use strict';

const CACHE_VERSION = 'lichen-wallet-v2';
const ASSETS = [
    './',
    './index.html',
    './shared-base-styles.css',
    './shared-theme.css',
    './shared-config.js',
    './wallet.css',
    './shared/env.js',
    './shared/utils.js',
    './shared/wallet-connect.js',
    './js/wallet.js',
    './js/identity.js',
    './js/crypto.js',
    './js/shielded.js',
    './manifest.json',
    './LichenWallet_Logo_256.png',
    './icon-192.png',
    './icon-256.png',
    './icon-512.png',
    './favicon.ico',
];

// Install: pre-cache core assets
self.addEventListener('install', (event) => {
    event.waitUntil(
        caches.open(CACHE_VERSION)
            .then((cache) => cache.addAll(ASSETS))
            .then(() => self.skipWaiting())
    );
});

// Activate: delete old caches, claim clients immediately
self.addEventListener('activate', (event) => {
    event.waitUntil(
        caches.keys()
            .then((keys) => Promise.all(
                keys.filter((k) => k !== CACHE_VERSION).map((k) => caches.delete(k))
            ))
            .then(() => self.clients.claim())
            .then(() => {
                // Notify all clients that a new version is active
                return self.clients.matchAll({ type: 'window' });
            })
            .then((clients) => {
                for (const client of clients) {
                    client.postMessage({ type: 'SW_UPDATED', version: CACHE_VERSION });
                }
            })
    );
});

// Fetch: cache-first for same-origin assets, network-first for API calls
self.addEventListener('fetch', (event) => {
    let url;

    try {
        url = new URL(event.request.url);
    } catch {
        return;
    }

    // Skip non-GET requests
    if (event.request.method !== 'GET') {
        return;
    }

    // Cache storage only supports HTTP(S) requests.
    if (url.protocol !== 'http:' && url.protocol !== 'https:') {
        return;
    }

    // Network-first for API / RPC calls
    if (url.pathname.includes('/api/') || url.pathname.includes('/solana-compat') || url.pathname.includes('/evm')) {
        return;
    }

    // CDN resources (fonts, icons): cache on first use for offline support
    if (url.origin !== self.location.origin) {
        event.respondWith(
            caches.match(event.request).then((cached) => {
                return cached || fetch(event.request).then((response) => {
                    if (response && response.status === 200) {
                        const clone = response.clone();
                        caches.open(CACHE_VERSION).then((cache) => cache.put(event.request, clone).catch(() => { }));
                    }
                    return response;
                }).catch(() => cached);
            })
        );
        return;
    }

    event.respondWith(
        caches.match(event.request).then((cached) => {
            // Return cached immediately, then update cache in background
            const fetchPromise = fetch(event.request).then((response) => {
                if (response && response.status === 200 && response.type === 'basic') {
                    const clone = response.clone();
                    caches.open(CACHE_VERSION).then((cache) => cache.put(event.request, clone).catch(() => { }));
                }
                return response;
            }).catch(() => cached);

            return cached || fetchPromise;
        })
    );
});

// Listen for skip waiting message from clients
self.addEventListener('message', (event) => {
    if (event.data && event.data.type === 'SKIP_WAITING') {
        self.skipWaiting();
    }
});
