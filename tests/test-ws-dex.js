const WebSocket = require('ws');
const ws = new WebSocket('ws://localhost:8900/');
let done = false;
let msgCount = 0;

ws.on('open', () => {
    console.log('WS CONNECTED');
    ws.send(JSON.stringify({method:'subscribe',params:{channel:'dex_orderbook',pair_id:1}}));
    ws.send(JSON.stringify({method:'subscribe',params:{channel:'dex_ticker',pair_id:1}}));
    ws.send(JSON.stringify({method:'subscribe',params:{channel:'slots'}}));
    console.log('SUBSCRIBED to dex_orderbook + dex_ticker + slots');
    setTimeout(() => {
        if (done) return;
        done = true;
        ws.close();
        if (msgCount < 1) {
            console.error('WS FAIL — received 0 messages in 3 seconds');
            process.exit(1);
        }
        console.log('WS PASS — received ' + msgCount + ' messages');
        process.exit(0);
    }, 3000);
});

ws.on('message', (d) => {
    msgCount++;
    const text = d.toString().substring(0, 200);
    try {
        JSON.parse(d.toString());
    } catch (e) {
        console.error('WS FAIL — non-JSON message:', text);
        process.exit(1);
    }
    console.log('MSG:', text);
});

ws.on('error', (e) => {
    console.error('WS ERROR:', e.message);
    process.exit(1);
});
