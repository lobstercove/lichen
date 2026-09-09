'use strict';
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const { test } = require('node:test');
const source = fs.readFileSync(path.join(__dirname, '../../wallet/sw.js'), 'utf8');

function dispatch(url, { cached, offline = true, mode = 'cors', method = 'GET' } = {}) {
    const listeners = {}, calls = [];
    const context = {
        URL, Response,
        self: { location: new URL('https://wallet.example/sw.js'), addEventListener: (name, fn) => { listeners[name] = fn; } },
        caches: {
            match: async () => cached,
            open: async () => ({ put: async () => {} }),
        },
        fetch: async request => {
            calls.push(request);
            if (offline) throw new TypeError('Network unavailable');
            return new Response('network response');
        },
    };
    vm.runInNewContext(source, context, { filename: 'wallet/sw.js' });
    let response;
    listeners.fetch({ request: { url, mode, method }, respondWith: value => { response = Promise.resolve(value); } });
    return { response, calls };
}

for (const url of ['https://wallet.example/uncached.css', 'https://cdn.example/uncached.woff2']) {
    test(`uncached failed fetch returns a Response: ${url}`, async () => {
        const { response } = dispatch(url);
        const value = await response;
        assert(value instanceof Response, 'respondWith must never resolve to undefined');
        assert.equal(value.type, 'error');
    });
    test(`cached assets remain available offline: ${url}`, async () => {
        const { response } = dispatch(url, { cached: new Response('cached asset') });
        assert.equal(await (await response).text(), 'cached asset');
    });
    test(`uncached successful fetch remains usable: ${url}`, async () => {
        const { response } = dispatch(url, { offline: false });
        assert.equal(await (await response).text(), 'network response');
    });
}

test('offline navigation with an empty cache returns a network-error Response', async () => {
    const { response } = dispatch('https://wallet.example/?connect=1', { mode: 'navigate' });
    assert.equal((await response).type, 'error');
});

for (const [url, method] of [
    ['https://wallet.example/api/testnet', 'GET'],
    ['https://testnet-api.example/', 'POST'],
    ['https://faucet.example/faucet/airdrops?address=public-fixture', 'GET'],
]) {
    test(`live account data bypasses the asset cache: ${method} ${url}`, () => {
        const result = dispatch(url, { method, cached: new Response('stale account data') });
        assert.equal(result.response, undefined, 'the browser must own this network request');
        assert.deepEqual(result.calls, []);
    });
}
