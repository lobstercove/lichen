#!/usr/bin/env node
'use strict';

const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');

const repoRoot = path.resolve(__dirname, '..', '..');
const explorerSource = fs.readFileSync(
    path.join(repoRoot, 'explorer', 'js', 'explorer.js'),
    'utf8'
);
const elements = new Map(
    ['latestBlock', 'slotTimeLabel', 'slotTargetLabel', 'slotCadenceSource']
        .map((id) => [id, { id, textContent: '' }])
);
const context = vm.createContext({
    console,
    Date,
    URL,
    URLSearchParams,
    setTimeout,
    clearTimeout,
    setInterval,
    clearInterval,
    LICHEN_CONFIG: {
        currentNetwork: () => 'testnet',
        resolveNetwork: () => 'testnet',
        networks: {
            testnet: {
                rpc: '',
                ws: null,
                slotDurationMs: 400,
            },
        },
    },
    document: {
        addEventListener: () => {},
        getElementById: (id) => elements.get(id) || null,
    },
    window: {
        location: {
            href: '',
            hostname: 'explorer.lichen.network',
            host: 'explorer.lichen.network',
            origin: 'https://explorer.lichen.network',
        },
    },
    formatSlot: (slot) => String(slot),
});

vm.runInContext(explorerSource, context, { filename: 'explorer/js/explorer.js' });

const steady = vm.runInContext([
    'resetDashboardWsCadence();',
    'observeDashboardWsBlock({ slot: 100 }, 1000);',
    'observeDashboardWsBlock({ slot: 101 }, 1320);',
    'observeDashboardWsBlock({ slot: 102 }, 1640);',
    'observeDashboardWsBlock({ slot: 103 }, 1960);',
    'observeDashboardWsBlock({ slot: 104 }, 2280);',
    'observeDashboardWsBlock({ slot: 105 }, 2600);',
].join('\n'), context);
assert.deepStrictEqual(
    JSON.parse(JSON.stringify(steady)),
    { slot: 105, samples: 5, observedMs: 320, ready: true }
);
assert.strictEqual(elements.get('latestBlock').textContent, '105');
assert.strictEqual(elements.get('slotTimeLabel').textContent, 320);
assert.strictEqual(elements.get('slotTargetLabel').textContent, 400);
assert.strictEqual(elements.get('slotCadenceSource').textContent, 'Live WS');

const normalized = vm.runInContext([
    'resetDashboardWsCadence();',
    'observeDashboardWsBlock({ slot: 200 }, 1000);',
    'observeDashboardWsBlock({ slot: 202 }, 1640);',
    'observeDashboardWsBlock({ slot: 204 }, 2280);',
    'observeDashboardWsBlock({ slot: 206 }, 2920);',
    'observeDashboardWsBlock({ slot: 208 }, 3560);',
    'observeDashboardWsBlock({ slot: 210 }, 4200);',
].join('\n'), context);
assert.strictEqual(normalized.observedMs, 320);
assert.strictEqual(normalized.ready, true);

const discontinuity = vm.runInContext(
    'observeDashboardWsBlock({ slot: 243 }, 5000);',
    context
);
assert.deepStrictEqual(
    JSON.parse(JSON.stringify(discontinuity)),
    { slot: 243, samples: 0, observedMs: 0, ready: false }
);
vm.runInContext('observeDashboardWsBlock({ slot: 242 }, 5100);', context);
assert.strictEqual(elements.get('latestBlock').textContent, '243');

console.log('Explorer live cadence: direct slot rendering, normalized median, and reset gates passed');
