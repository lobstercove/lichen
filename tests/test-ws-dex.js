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
        console.log('WS CLOSED OK — received ' + msgCount + ' messages');
        process.exit(0);
    }, 3000);
});

ws.on('message', (d) => {
    msgCount++;
    console.log('MSG:', d.toString().substring(0, 200));
});

ws.on('error', (e) => {
    console.log('WS ERROR:', e.message);
    process.exit(1);
});
