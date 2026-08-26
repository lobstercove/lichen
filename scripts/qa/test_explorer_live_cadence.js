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
    ['latestBlock', 'slotTimeLabel', 'slotCadenceSource', 'tpsValue', 'peakTps', 'tpsSource']
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
assert.strictEqual(elements.get('slotCadenceSource').textContent, 'Live');

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

const liveTps = vm.runInContext([
    'resetDashboardWsCadence();',
    'observeDashboardWsBlock({ slot: 300, transactions: 0 }, 1000);',
    'observeDashboardWsBlock({ slot: 301, transactions: 1 }, 2000);',
    'observeDashboardWsBlock({ slot: 302, transactions: 0 }, 3000);',
    'observeDashboardWsBlock({ slot: 303, transactions: 1 }, 4000);',
    'observeDashboardWsBlock({ slot: 304, transactions: 0 }, 5000);',
    'observeDashboardWsBlock({ slot: 305, transactions: 1 }, 6000);',
    'observeDashboardWsBlock({ slot: 306, transactions: 0 }, 7000);',
    '({ tps: dashboardWsTps, peakTps: dashboardWsPeakTps, samples: dashboardWsTpsSamples.length, ready: dashboardWsTpsReady });',
].join('\n'), context);
assert.deepStrictEqual(
    JSON.parse(JSON.stringify(liveTps)),
    { tps: 3 / 7, peakTps: 0.5, samples: 7, ready: true }
);
assert.strictEqual(elements.get('tpsValue').textContent, '0.43');
assert.strictEqual(elements.get('peakTps').textContent, '0.50');
assert.strictEqual(elements.get('tpsSource').textContent, 'Live 60s');

assert.strictEqual(
    vm.runInContext('formatDashboardTps(0.08)', context),
    '0.08',
    'fractional node TPS must not be floored to zero'
);
assert.strictEqual(
    vm.runInContext('calculateDashboardTotalStakeSpores([{ stake: 100 }, { stake: 200 }], { total_licn_staked: 400 })', context),
    700,
    'dashboard total stake must include validator and Moss stake'
);

console.log('Explorer live cadence/TPS: direct WS rendering, normalized windows, and reset gates passed');
