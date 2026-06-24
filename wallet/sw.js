// LichenWallet Service Worker — Cache-first assets with safe navigation fallback
'use strict';

const CACHE_VERSION = 'lichen-wallet-v4-20260624b';
const INDEX_URL = './index.html';
const ASSETS = [
    INDEX_URL,
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

function isCacheableResponse(response, type) {
    if (!response || !response.ok || response.redirected) return false;
    if (type === 'same-origin') return response.type === 'basic';
    return response.status === 200 && response.type !== 'opaqueredirect';
}

function fetchIndexFallback(requestUrl) {
    const fallbackUrl = new URL(INDEX_URL, self.location.href);
    fallbackUrl.search = requestUrl.search;

    return fetch(fallbackUrl.toString(), { cache: 'reload' }).then((response) => {
        if (isCacheableResponse(response, 'same-origin')) {
            const clone = response.clone();
            caches.open(CACHE_VERSION).then((cache) => {
                cache.put(INDEX_URL, clone).catch(() => { });
            });
            return response;
        }

        return caches.match(INDEX_URL).then((cached) => cached || response);
    }).catch(() => caches.match(INDEX_URL).then((cached) => cached || Response.error()));
}

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

    // Safari PWA rejects navigation responses served by a service worker when the
    // response carries redirect metadata. Keep navigations network-first, and use
    // the non-redirected app shell only when the network path redirects or fails.
    if (event.request.mode === 'navigate') {
        event.respondWith(
            fetch(event.request).then((response) => {
                if (isCacheableResponse(response, 'same-origin')) {
                    return response;
                }
                return fetchIndexFallback(url);
            }).catch(() => fetchIndexFallback(url))
        );
        return;
    }

    // CDN resources (fonts, icons): cache on first use for offline support
    if (url.origin !== self.location.origin) {
        event.respondWith(
            caches.match(event.request).then((cached) => {
                return cached || fetch(event.request).then((response) => {
                    if (isCacheableResponse(response, 'cross-origin')) {
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
                if (isCacheableResponse(response, 'same-origin')) {
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
