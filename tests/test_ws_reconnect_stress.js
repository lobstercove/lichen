#!/usr/bin/env node
'use strict';

const fs = require('fs');
const path = require('path');
const assert = require('assert');

let passed = 0;
let failed = 0;

function test(name, fn) {
    try {
        fn();
        passed++;
        console.log(`  ✅ ${name}`);
    } catch (error) {
        failed++;
        console.log(`  ❌ ${name}: ${error.message}`);
    }
}

function testAsync(name, fn) {
    return Promise.resolve()
        .then(fn)
        .then(() => {
            passed++;
            console.log(`  ✅ ${name}`);
        })
        .catch((error) => {
            failed++;
            console.log(`  ❌ ${name}: ${error.message}`);
        });
}

function extractClass(source, className) {
    const start = source.indexOf(`class ${className}`);
    if (start < 0) throw new Error(`Class ${className} not found`);

    let i = source.indexOf('{', start);
    if (i < 0) throw new Error(`Class ${className} opening brace not found`);

    let depth = 0;
    let inSingle = false;
    let inDouble = false;
    let inTemplate = false;
    let inLineComment = false;
    let inBlockComment = false;
    let escaped = false;

    for (; i < source.length; i++) {
        const ch = source[i];
        const next = source[i + 1];

        if (inLineComment) {
            if (ch === '\n') inLineComment = false;
            continue;
        }
        if (inBlockComment) {
            if (ch === '*' && next === '/') {
                inBlockComment = false;
                i++;
            }
            continue;
        }

        if (!inSingle && !inDouble && !inTemplate) {
            if (ch === '/' && next === '/') {
                inLineComment = true;
                i++;
                continue;
            }
            if (ch === '/' && next === '*') {
                inBlockComment = true;
                i++;
                continue;
            }
        }

        if (!escaped) {
            if (!inDouble && !inTemplate && ch === '\'') inSingle = !inSingle;
            else if (!inSingle && !inTemplate && ch === '"') inDouble = !inDouble;
            else if (!inSingle && !inDouble && ch === '`') inTemplate = !inTemplate;
        }

        if (inSingle || inDouble || inTemplate) {
            escaped = !escaped && ch === '\\';
            continue;
        }
        escaped = false;

        if (ch === '{') depth++;
        if (ch === '}') {
            depth--;
            if (depth === 0) {
                return source.slice(start, i + 1);
            }
        }
    }

    throw new Error(`Class ${className} closing brace not found`);
}

function createTimerHarness() {
    let nextId = 1;
    const timers = new Map();

    function setTimeoutMock(fn, delay) {
        const id = nextId++;
        timers.set(id, { fn, delay });
        return id;
    }

    function clearTimeoutMock(id) {
        timers.delete(id);
    }

    function activeCount() {
        return timers.size;
    }

    function delays() {
        return Array.from(timers.values()).map((entry) => entry.delay);
    }

    function runOne() {
        const first = timers.entries().next();
        if (first.done) return false;
        const [id, entry] = first.value;
        timers.delete(id);
        entry.fn();
        return true;
    }

    function runAll(limit = 100) {
        let executed = 0;
        while (executed < limit && runOne()) executed++;
        return executed;
    }

    return {
        setTimeout: setTimeoutMock,
        clearTimeout: clearTimeoutMock,
        activeCount,
        delays,
        runAll,
    };
}

function createFakeWebSocketClass() {
    class FakeWebSocket {
        static CONNECTING = 0;
        static OPEN = 1;
        static CLOSING = 2;
        static CLOSED = 3;
        static instances = [];
        static failConstruct = false;

        constructor(url) {
            if (FakeWebSocket.failConstruct) throw new Error('construct failed');
            this.url = url;
            this.readyState = FakeWebSocket.CONNECTING;
            this.sent = [];
            this.onopen = null;
            this.onmessage = null;
            this.onerror = null;
            this.onclose = null;
            FakeWebSocket.instances.push(this);
        }

        send(msg) {
            this.sent.push(msg);
        }

        open() {
            this.readyState = FakeWebSocket.OPEN;
            if (typeof this.onopen === 'function') this.onopen();
        }

        close() {
            this.readyState = FakeWebSocket.CLOSED;
            if (typeof this.onclose === 'function') this.onclose({ code: 1000 });
        }
    }

    return FakeWebSocket;
}

console.log('\n── Task 18: WebSocket Reconnect Stress Tests ──\n');

const dexSource = fs.readFileSync(path.join(__dirname, '..', 'dex', 'dex.js'), 'utf8');
const explorerSource = fs.readFileSync(path.join(__dirname, '..', 'explorer', 'js', 'explorer.js'), 'utf8');
const walletSource = fs.readFileSync(path.join(__dirname, '..', 'wallet', 'js', 'wallet.js'), 'utf8');
const transactionsSource = fs.readFileSync(path.join(__dirname, '..', 'explorer', 'js', 'transactions.js'), 'utf8');

const dexClassSrc = extractClass(dexSource, 'DexWS');
const explorerClassSrc = extractClass(explorerSource, 'LichenWS');

async function main() {
await testAsync('T18.1 DEX reload loop dedupes repeated subscribe requests', async () => {
    const timers = createTimerHarness();
    const FakeWebSocket = createFakeWebSocketClass();

    const DexWS = new Function(
        'WebSocket',
        'setTimeout',
        'clearTimeout',
        'console',
        `${dexClassSrc}; return DexWS;`
    )(FakeWebSocket, timers.setTimeout, timers.clearTimeout, { log() {}, warn() {}, error() {} });

    const dexWs = new DexWS('ws://dex.test');
    assert.strictEqual(FakeWebSocket.instances.length, 1, 'One socket should be created');
    FakeWebSocket.instances[0].open();

    let subscribeCalls = 0;
    dexWs._sendSubscribe = async (_method, params) => {
        subscribeCalls++;
        return `${params.channel}-sub`;
    };

    const calls = [];
    for (let i = 0; i < 50; i++) {
        calls.push(dexWs.subscribe('trades.1', () => {}));
    }
    await Promise.all(calls);

    assert.strictEqual(dexWs.subs.size, 1, 'Only one channel entry should exist after reload loop');
    assert.strictEqual(subscribeCalls, 1, 'Only one subscribe RPC should be emitted for duplicate channel');
});

test('T18.2 DEX network flap keeps at most one reconnect timer', () => {
    const timers = createTimerHarness();
    const FakeWebSocket = createFakeWebSocketClass();
    FakeWebSocket.failConstruct = true;

    const DexWS = new Function(
        'WebSocket',
        'setTimeout',
        'clearTimeout',
        'console',
        `${dexClassSrc}; return DexWS;`
    )(FakeWebSocket, timers.setTimeout, timers.clearTimeout, { log() {}, warn() {}, error() {} });

    const dexWs = new DexWS('ws://dex.test');
    dexWs.connect();
    dexWs.connect();

    assert.strictEqual(timers.activeCount(), 1, 'Only one reconnect timer should remain during repeated failures');
});

test('T18.3 DEX intentional close clears timers and subscriptions', () => {
    const timers = createTimerHarness();
    const FakeWebSocket = createFakeWebSocketClass();

    const DexWS = new Function(
        'WebSocket',
        'setTimeout',
        'clearTimeout',
        'console',
        `${dexClassSrc}; return DexWS;`
    )(FakeWebSocket, timers.setTimeout, timers.clearTimeout, { log() {}, warn() {}, error() {} });

    const dexWs = new DexWS('ws://dex.test');
    FakeWebSocket.instances[0].open();
    dexWs.subs.set('book.1', { channel: 'book.1' });
    dexWs.pendingReqs.set(99, { reject() {} });
    dexWs.reconnectTimer = timers.setTimeout(() => {}, 1000);

    dexWs.close();

    assert.strictEqual(dexWs._closing, true, 'Close should mark websocket as intentional close');
    assert.strictEqual(timers.activeCount(), 0, 'Close should clear reconnect timer');
    assert.strictEqual(dexWs.subs.size, 0, 'Close should clear subscriptions');
    assert.strictEqual(dexWs.pendingReqs.size, 0, 'Close should clear pending RPC requests');
});

test('T18.4 Explorer reload loop connect is idempotent while connecting', () => {
    const timers = createTimerHarness();
    const FakeWebSocket = createFakeWebSocketClass();

    const LichenWS = new Function(
        'WebSocket',
        'setTimeout',
        'clearTimeout',
        'setInterval',
        'clearInterval',
        'console',
        `${explorerClassSrc}; return LichenWS;`
    )(
        FakeWebSocket,
        timers.setTimeout,
        timers.clearTimeout,
        () => 0,
        () => {},
        { log() {}, warn() {}, error() {} }
    );

    const wsClient = new LichenWS('ws://explorer.test');
    wsClient.connect();
    wsClient.connect();

    assert.strictEqual(FakeWebSocket.instances.length, 1, 'Explorer should not spawn duplicate sockets during connect storms');
});

test('T18.5 Explorer network flap schedules bounded reconnect backoff', () => {
    const timers = createTimerHarness();
    const FakeWebSocket = createFakeWebSocketClass();

    const LichenWS = new Function(
        'WebSocket',
        'setTimeout',
        'clearTimeout',
        'setInterval',
        'clearInterval',
        'console',
        `${explorerClassSrc}; return LichenWS;`
    )(
        FakeWebSocket,
        timers.setTimeout,
        timers.clearTimeout,
        () => 0,
        () => {},
        { log() {}, warn() {}, error() {} }
    );

    const wsClient = new LichenWS('ws://explorer.test');
    wsClient.connect();
    FakeWebSocket.instances[0].open();
    FakeWebSocket.instances[0].close();

    assert.strictEqual(timers.activeCount(), 1, 'Explorer should schedule exactly one reconnect timer after close');
    assert.strictEqual(timers.delays()[0], 1000, 'First reconnect delay should start at 1s');
    assert.strictEqual(wsClient.reconnectDelay, 2000, 'Reconnect delay should back off after close');
});

test('T18.6 Explorer intentional close cancels reconnect and clears desired subscriptions', () => {
    const timers = createTimerHarness();
    const FakeWebSocket = createFakeWebSocketClass();

    const LichenWS = new Function(
        'WebSocket',
        'setTimeout',
        'clearTimeout',
        'setInterval',
        'clearInterval',
        'console',
        `${explorerClassSrc}; return LichenWS;`
    )(
        FakeWebSocket,
        timers.setTimeout,
        timers.clearTimeout,
        () => 0,
        () => {},
        { log() {}, warn() {}, error() {} }
    );

    const wsClient = new LichenWS('ws://explorer.test');
    wsClient.connect();
    wsClient.desired.push({ method: 'subscribeBlocks' });
    wsClient.reconnectTimer = timers.setTimeout(() => {}, 1000);

    wsClient.close();

    assert.strictEqual(wsClient._closing, true, 'Close should mark explorer websocket as intentional');
    assert.strictEqual(timers.activeCount(), 0, 'Close should clear reconnect timer');
    assert.strictEqual(wsClient.desired.length, 0, 'Close should clear desired subscription set');
    assert.strictEqual(wsClient.pending.size, 0, 'Close should clear pending RPC map');
});

test('T18.7 Wallet reconnect guards cover manual close + offline + hidden states', () => {
    assert(walletSource.includes('if (balanceWsReconnectTimer) return;'), 'Wallet should dedupe reconnect timer scheduling');
    assert(walletSource.includes('if (_wsManualClose) return;'), 'Wallet should skip reconnect on intentional close');
    assert(walletSource.includes("if (typeof navigator !== 'undefined' && !navigator.onLine) return;"), 'Wallet should skip reconnect when offline');
    assert(walletSource.includes("if (typeof document !== 'undefined' && document.visibilityState === 'hidden') return;"), 'Wallet should skip reconnect while tab hidden');
    assert(walletSource.includes("window.addEventListener('online'"), 'Wallet should reconnect on online event');
    assert(walletSource.includes("document.addEventListener('visibilitychange'"), 'Wallet should reconnect on visibility change');
});

test('T18.8 Explorer transactions page blocks duplicate subscribeBlocks registration', () => {
    assert(transactionsSource.includes('let blockSubRegistered = false;'), 'Transactions page should track subscription registration state');
    assert(transactionsSource.includes('if (blockSubRegistered) return;'), 'Transactions page should short-circuit duplicate registrations');
    assert(transactionsSource.includes('blockSubRegistered = false;'), 'Transactions page should reset guard on subscribe failure');
});

console.log(`\n${'─'.repeat(58)}`);
console.log(`Task 18 Reconnect Stress Tests: ${passed} passed, ${failed} failed (${passed + failed} total)`);

if (failed > 0) {
    process.exit(1);
}
console.log('All reconnect stress tests passed ✅');

}

main().catch((error) => {
    console.error(error);
    process.exit(1);
});