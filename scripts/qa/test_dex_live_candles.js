#!/usr/bin/env node
'use strict';
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const source = fs.readFileSync(path.join(__dirname, '../../dex/dex.js'), 'utf8');

function declaration(marker) {
    const start = source.indexOf(marker);
    assert(start >= 0, marker);
    const open = source.indexOf('{', start);
    let depth = 0;
    for (let i = open; i < source.length; i++) {
        if (source[i] === '{') depth++;
        if (source[i] === '}' && --depth === 0) return source.slice(start, i + 1);
    }
    throw new Error(`Unterminated ${marker}`);
}

function harness() {
    const context = vm.createContext({ Map, Number, Math, Date, setTimeout, clearTimeout });
    const names = ['createLiveCandleStream', 'chartStream', 'chartPair', 'createDatafeed',
        'streamBarUpdate', 'resolutionToMs', 'resolutionToSec', 'subscribeCandleWs',
        'unsubscribeCandleWs', 'drawChart'];
    vm.runInContext(`
        const pairs = [{id:'BTC/lUSD',pairId:12},{id:'ETH/lUSD',pairId:3},{id:'LICN/BTC',pairId:13,inverted:true}];
        const state = {activePair:pairs[0],activePairId:12};
        const chartStreams = new Map(), chartSubscribers = new Map();
        const channels = new Map(), unsubscribed = [];
        const dexWs = { subscribe: async (channel, callback) => { channels.set(channel, callback); return channel; },
            unsubscribe: channel => { channels.delete(channel); unsubscribed.push(channel); } };
        const localStorage = {setItem(){}};
        const isDisplayInvertedPair = p => !!p.inverted;
        const invertPrice = p => 1/p;
        let history = [], historyPair = null;
        async function loadCandles(from,to,res,pair) { historyPair=pair; return history; }
        ${names.map(n => declaration('function '+n+'(')).join('\n')}
        globalThis.test = { createLiveCandleStream, createDatafeed, streamBarUpdate, chartStream,
            pairs, state, channels, unsubscribed, chartStreams,
            setHistory: bars => { history=bars; }, getHistoryPair: () => historyPair };
    `, context);
    return context.test;
}

async function run() {
    const h = harness();
    let now = 900000, updates = [];
    const stream = h.createLiveCandleStream(900000, bar => updates.push(bar), () => now);
    stream.seed({time:900000,open:78447.79,high:78447.79,low:78283.36,close:78283.36,volume:7});
    // Actual public BTC/lUSD ticker/candle mismatch observed September 8.
    for (const price of [78228.74,78228.75,78241.88,78242,78245.99]) stream.price(price);
    assert.equal(updates.length,5);
    assert.equal(stream.current().close,78245.99);
    assert.equal(stream.current().open,78447.79);
    assert.equal(stream.current().low,78228.74);
    assert.equal(stream.current().volume,7,'quotes must not invent or erase volume');
    stream.snapshot({open:78447.79,high:78447.79,low:78249.97,close:78249.97,volume:0});
    assert.equal(updates.length,5,'delayed consensus candle must not pull live close backward');
    stream.price(78201.28,2);
    assert.equal(stream.current().volume,9);
    stream.seed({time:900000,open:78447.79,high:78500,low:78220,close:78249.97,volume:9},true);
    assert.equal(stream.current().close,78201.28,'history refresh preserves live close');
    assert.equal(stream.current().high,78500);
    stream.snapshot({open:78447.79,high:78500,low:78220,close:78249.97,volume:10});
    assert.equal(stream.current().volume,10,'current-period canonical volume can advance without replacing live close');
    assert.equal(stream.current().close,78201.28);
    const before=JSON.stringify(stream.current());
    stream.seed({time:0,open:1,high:1,low:1,close:1,volume:0});
    for(const price of [0,-1,NaN,Infinity]) stream.price(price);
    assert.equal(JSON.stringify(stream.current()),before,'pagination/invalid quotes cannot alter current bar');
    now=1800000;stream.price(78199);
    assert.deepEqual(JSON.parse(JSON.stringify(stream.current())),{time:1800000,open:78199,high:78199,low:78199,close:78199,volume:0});
    stream.snapshot({open:78447.79,high:78447.79,low:78249.97,close:78249.97,volume:99});
    assert.equal(stream.current().open,78199,'undated snapshot cannot populate the next interval');
    updates.at(-1).close=1;
    assert.equal(stream.current().close,78199,'chart callback mutation cannot corrupt stored bar');
    now=900000;stream.price(77777);
    assert.equal(stream.current().close,78199,'clock rollback cannot rewrite a future bar');

    const feed=h.createDatafeed(), btc=[], eth=[], minute=[];
    feed.subscribeBars({ticker:'BTC/lUSD'},'15',bar=>btc.push(bar),'btc15');
    feed.subscribeBars({ticker:'ETH/lUSD'},'15',bar=>eth.push(bar),'eth15');
    feed.subscribeBars({ticker:'BTC/lUSD'},'1',bar=>minute.push(bar),'btc1');
    h.streamBarUpdate(100,0,12);
    assert.equal(btc.at(-1).close,100);assert.equal(minute.at(-1).close,100);assert.equal(eth.length,0);
    h.streamBarUpdate(20,0,3);assert.equal(eth.at(-1).close,20);assert.equal(btc.at(-1).close,100);
    const late=h.channels.get('candles:12:900');
    feed.unsubscribeBars('btc15');
    late({pairId:12,interval:900,open:1,high:1,low:1,close:1,slot:123});
    h.streamBarUpdate(101,0,12);
    assert.equal(btc.length,1);assert.equal(minute.at(-1).close,101,'unsubscribe preserves other timeframe');
    const replacement=[];
    feed.subscribeBars({ticker:'BTC/lUSD'},'15',bar=>replacement.push(bar),'btc15-new');
    late({pairId:12,interval:900,open:1,high:1,low:1,close:1,slot:124});
    assert.equal(replacement.length,0,'late prior-generation events cannot affect new subscription');
    h.streamBarUpdate(102,0,12);assert.equal(replacement.at(-1).close,102);
    const same=[];
    feed.subscribeBars({ticker:'BTC/lUSD'},'15',bar=>same.push(bar),'btc15-second');
    feed.unsubscribeBars('btc15-new');h.streamBarUpdate(103,0,12);
    assert.equal(same.at(-1).close,103,'same-dataset subscriber survives peer unsubscribe');
    h.setHistory([{time:0,open:2,high:2,low:2,close:2,volume:0}]);
    await feed.getBars({ticker:'ETH/lUSD'},'15',{from:0,to:1},()=>{});
    assert.equal(h.getHistoryPair().pairId,3,'history uses requested symbol rather than active UI pair');
    assert.equal(eth.at(-1).close,20);

    // Test real ticker wiring, not just the accumulator in isolation.
    const tickers=new Map();
    const inverted=[];
    feed.subscribeBars({ticker:'LICN/BTC'},'15',bar=>inverted.push(bar),'inverse');
    const wired=vm.createContext({Map,streamBarUpdate:h.streamBarUpdate});
    vm.runInContext(`const pairs=[{pairId:12},{pairId:3},{pairId:13,inverted:true}];const state={activePairId:12};
        let _tickerSubs=[];const dexWs={unsubscribe(){},subscribe:async(c,cb)=>{ globalThis.handlers.set(c,cb);return c; }};
        const isDisplayInvertedPair=p=>!!p.inverted, invertPrice=p=>1/p, updateTickerDisplay=()=>{}, throttledRenderPairList=()=>{};
        ${declaration('function subscribeAllTickers(')}
        globalThis.start=subscribeAllTickers;`,wired);
    wired.handlers=tickers;wired.start();
    tickers.get('ticker:12')({lastPrice:78201.28,change24h:0});
    assert.equal(same.at(-1).close,78201.28,'ticker callback must reach chart at event cadence');
    tickers.get('ticker:13')({lastPrice:500000,change24h:0});
    assert.equal(inverted.at(-1).close,0.000002,'inverted pair uses display price exactly once');

    const race = vm.createContext({Map,WebSocket:{OPEN:1}});
    vm.runInContext(`${declaration('class DexWS {')}\nglobalThis.client=Object.create(DexWS.prototype);`,race);
    const client=race.client, sent=[], resolvers=[];
    client.subs=new Map();client.nextReqId=1;
    client.ws={readyState:1,send:message=>sent.push(JSON.parse(message))};
    client._sendSubscribe=()=>new Promise(resolve=>resolvers.push(resolve));
    const first=client.subscribe('candles:12:900',()=>{});
    client.unsubscribe('candles:12:900');
    const freshCallback=()=>{};
    const second=client.subscribe('candles:12:900',freshCallback);
    resolvers[0](7);await first;
    assert.equal(client.subs.get('candles:12:900').callback,freshCallback,'late acknowledgement must not resurrect removed subscriber');
    assert.equal(sent.at(-1).method,'unsubscribeDex');assert.equal(sent.at(-1).params.subscription,7);
    resolvers[1](8);await second;
    assert.equal(client.subs.get('candles:12:900').subId,8);
    class Socket {
        static OPEN=1;
        constructor(){this.readyState=1;}
        send(message){sent.push(JSON.parse(message));}
    }
    race.WebSocket=Socket;
    client._closing=false;client.reconnectTimer=null;client.pending=[];
    client.connect();client.ws.onopen();
    client.unsubscribe('candles:12:900');
    resolvers[2](9);await Promise.resolve();
    assert.equal(client.subs.has('candles:12:900'),false,'reconnect acknowledgement cannot resurrect removed subscription');
    assert.equal(sent.at(-1).params.subscription,9);
    console.log('PASS DEX live candles: live ticks, late snapshots, rollover, volume, history, symbol/timeframe isolation and ticker wiring');
}

run().catch(error=>{console.error(error);process.exitCode=1;});
