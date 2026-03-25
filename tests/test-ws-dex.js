const WebSocket = require('ws');

const WS_URL = process.env.LICHEN_WS || 'ws://localhost:8900/';
const ws = new WebSocket(WS_URL);

let finished = false;
let messageCount = 0;
let ackCount = 0;
let dexNotifications = 0;
let genericNotifications = 0;

const requiredAcks = new Set([1, 2, 3, 4, 5]);
const expectedErrorIds = new Set([6]);

function fail(msg) {
    if (finished) return;
    finished = true;
    console.error('WS FAIL — ' + msg);
    try { ws.close(); } catch (_) {}
    process.exit(1);
}

function pass() {
    if (finished) return;
    finished = true;
    console.log(
        `WS PASS — messages=${messageCount}, acks=${ackCount}, notifications=${genericNotifications}, dex_typed_notifications=${dexNotifications}`
    );
    try { ws.close(); } catch (_) {}
    process.exit(0);
}

ws.on('open', () => {
    console.log('WS CONNECTED:', WS_URL);

    ws.send(JSON.stringify({
        jsonrpc: '2.0',
        id: 1,
        method: 'subscribeDex',
        params: { channel: 'trades:1' },
    }));

    ws.send(JSON.stringify({
        jsonrpc: '2.0',
        id: 2,
        method: 'subscribeDex',
        params: { channel: 'ticker:1' },
    }));

    ws.send(JSON.stringify({
        jsonrpc: '2.0',
        id: 3,
        method: 'subscribeSlots',
        params: {},
    }));

    ws.send(JSON.stringify({
        jsonrpc: '2.0',
        id: 4,
        method: 'subscribeDex',
        params: { channel: 'orders:testaddr' },
    }));

    ws.send(JSON.stringify({
        jsonrpc: '2.0',
        id: 5,
        method: 'subscribeDex',
        params: { channel: 'positions:testaddr' },
    }));

    ws.send(JSON.stringify({
        jsonrpc: '2.0',
        id: 6,
        method: 'subscribeDex',
        params: { channel: 'rewards:testaddr' },
    }));

    console.log('SUBSCRIBED to trades:1 + ticker:1 + slots + orders:testaddr + positions:testaddr (and invalid rewards:testaddr)');
});

ws.on('message', (raw) => {
    messageCount += 1;
    let msg;
    try {
        msg = JSON.parse(raw.toString());
    } catch (_e) {
        fail('received non-JSON message');
        return;
    }

    if (msg && msg.error) {
        const errId = msg.id;
        if (expectedErrorIds.has(errId)) {
            expectedErrorIds.delete(errId);
            return;
        }
        fail(`rpc error received: ${JSON.stringify(msg.error)}`);
        return;
    }

    if (msg && Object.prototype.hasOwnProperty.call(msg, 'id') && requiredAcks.has(msg.id)) {
        if (typeof msg.result !== 'number' || msg.result <= 0) {
            fail(`invalid subscription ack for id=${msg.id}: ${JSON.stringify(msg)}`);
            return;
        }
        requiredAcks.delete(msg.id);
        ackCount += 1;
        return;
    }

    const method = msg?.method;
    if (method === 'notification') {
        genericNotifications += 1;
        const payloadType = msg?.params?.result?.type;
        if (typeof payloadType === 'string' && payloadType.length > 0) {
            dexNotifications += 1;
        }
    }

    if (requiredAcks.size === 0 && genericNotifications >= 1) {
        pass();
    }
});

ws.on('error', (e) => {
    fail(e.message || 'websocket error');
});

setTimeout(() => {
    if (requiredAcks.size > 0) {
        fail(`missing subscription ACKs for ids: ${Array.from(requiredAcks).join(', ')}`);
        return;
    }
    if (expectedErrorIds.size > 0) {
        fail(`missing expected invalid-channel error for ids: ${Array.from(expectedErrorIds).join(', ')}`);
        return;
    }
    pass();
}, 10000);
