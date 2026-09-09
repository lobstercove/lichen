#!/usr/bin/env node
'use strict';
// Full-page fixture test with a disposable profile and public synthetic balances.
const { chromium } = require('playwright');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const http = require('node:http');
const path = require('node:path');
const root = path.resolve(__dirname, '../../dex');
const address = '11111111111111111111111111111111';
const markets = [
    {pairId:12,baseSymbol:'wBTC',quoteSymbol:'lUSD',lastPrice:78393.62,tickSize:1,lotSize:1},
    {pairId:1,baseSymbol:'LICN',quoteSymbol:'lUSD',lastPrice:0.15,tickSize:1,lotSize:1},
];
const mime = {'.html':'text/html','.js':'text/javascript','.css':'text/css','.png':'image/png'};
const server = http.createServer((req,res) => {
    let file = path.resolve(root, '.' + new URL(req.url,'http://localhost').pathname);
    if (!file.startsWith(root+path.sep) && file !== root) return res.writeHead(403).end();
    if (file === root) file = path.join(root,'index.html');
    try {const bytes=fs.readFileSync(file);res.writeHead(200,{'Content-Type':mime[path.extname(file)] || 'application/octet-stream'});res.end(bytes);}
    catch {res.writeHead(404).end();}
});
async function run() {
    await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
    const origin = `http://127.0.0.1:${server.address().port}`;
    const browser = await chromium.launch({headless:true});
    try {
        for (const [label,response,width] of [
            ['desktop-spendable',{spendable:116670655400000,spores:233341310800000},1440],
            ['tablet-legacy',{spores:116670655400000},820],
            ['mobile-spendable',{spendable:116670655400000},390],
        ]) {
            const context = await browser.newContext({viewport:{width,height:1000}});
            await context.route('**/*',async route=>{
                const req=route.request(),url=new URL(req.url());
                if (url.origin===origin) return route.continue();
                if (req.resourceType()!=='fetch' && req.resourceType()!=='xhr') return route.abort();
                const payload=req.method()==='POST' ? req.postDataJSON() : {};
                assert(!/^(send|submit)/i.test(payload?.method || ''),'No transaction submission in browser fixtures');
                let result=null;
                if (payload?.method==='getBalance') result=response;
                if (payload?.method==='getTokenAccounts') result={accounts:[]};
                if (payload?.method==='getAllSymbolRegistry') result={entries:
                    ['DEX','DEXAMM','DEXROUTER','DEXMARGIN','DEXREWARDS','DEXGOV','ANALYTICS','PREDICT','SPOREPUMP','ORACLE','LUSD','WBTC']
                        .map(symbol=>({symbol,program:address}))};
                if (url.pathname.endsWith('/api/v1/pairs')) result=markets;
                if (/\/orders|\/trades|\/positions|\/candles|\/enabled-pairs/.test(url.pathname)) result=[];
                if (url.pathname.endsWith('/orderbook')) result={asks:[],bids:[]};
                const body=payload?.method ? {jsonrpc:'2.0',id:payload.id,result} : {success:true,data:result};
                return route.fulfill({contentType:'application/json',body:JSON.stringify(body)});
            });
            await context.routeWebSocket('**/*',socket=>socket.close());
            await context.addInitScript(({address})=>{
                localStorage.setItem('dexWallets',JSON.stringify([{address,provider:'extension'}]));
                localStorage.setItem('dexActiveWallet',address);
                localStorage.setItem('dexLastPair','12');
                window.licnwallet={isLichenWallet:true,on(){},
                    getProviderState:async()=>({connected:true,hasWallet:true,isLocked:false,accounts:[address]}),
                    signTransaction:async()=>{throw new Error('Synthetic wallet cannot sign');}};
            },{address});
            const page=await context.newPage(),errors=[],consoleErrors=[];
            page.on('pageerror',error=>errors.push(error.message));
            page.on('console',message=>{if(message.type()==='error') consoleErrors.push(message.text());});
            await page.goto(origin,{waitUntil:'domcontentloaded'});
            try {await page.locator('.balance-usd').filter({hasText:'17,500.6'}).waitFor({timeout:15000});}
            catch(error) {console.error({label,errors,consoleErrors,balances:await page.locator('.balance-list').textContent(),pair:await page.locator('.pair-active').textContent()});throw error;}
            assert.equal(await page.locator('.balance-available').first().textContent(),'116,670.66',label);
            assert.equal(Number((await page.locator('.portfolio-value').textContent()).replace(/[$,]/g,'')),17500.6,label);
            assert((await page.locator('.pair-active .pair-name').textContent()).includes('BTC'),label);
            assert.deepEqual(errors,[],label+': uncaught browser errors');
            assert(!consoleErrors.some(message=>message.includes('[DEX] Init error')),label+': initialization failed');
            console.log('PASS '+label+': BTC market retains LICN $17,500.60 balance and portfolio');
            await context.close();
        }
    } finally {await browser.close();server.close();}
}
run().catch(error=>{console.error(error);server.close();process.exitCode=1;});
