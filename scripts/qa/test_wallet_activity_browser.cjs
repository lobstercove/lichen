#!/usr/bin/env node
'use strict';
const { chromium } = require('playwright');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const http = require('node:http');
const path = require('node:path');
const root = path.resolve(__dirname, '../..');
const pending = new Set();
let partialBodies = 0;
const server = http.createServer((req, res) => {
    const url = new URL(req.url, 'http://localhost');
    if (url.pathname === '/activity-hang-fixture' || url.pathname === '/faucet/airdrops') {
        partialBodies++;
        pending.add(res);res.on('close', () => pending.delete(res));
        res.writeHead(200, {'Content-Type':'application/json','Access-Control-Allow-Origin':'*'});
        res.write('{"jsonrpc":"2.0",');
        return;
    }
    let file = path.resolve(root, '.' + decodeURIComponent(url.pathname));
    if (!file.startsWith(root + path.sep)) { res.writeHead(403).end();return; }
    try {
        if (fs.statSync(file).isDirectory()) file = path.join(file, 'index.html');
        res.writeHead(200, {'Content-Type':{'.js':'text/javascript','.css':'text/css','.html':'text/html','.json':'application/json','.png':'image/png','.svg':'image/svg+xml'}[path.extname(file)] || 'application/octet-stream'});
        res.end(fs.readFileSync(file));
    } catch { res.writeHead(404).end(); }
});
const transaction = signature => ({type:'Transfer',from:'public-observation',to:'public-recipient',amount:1000000000,signature,timestamp:1789190000,slot:123});
const history = signature => ({transactions:[transaction(signature)],has_more:false});
async function init(page, port) {
    await page.routeWebSocket('**/*', ws => ws.close());
    await page.route('**/*', async route => {
        const req = route.request(), url = new URL(req.url());
        if (url.hostname === '127.0.0.1') return route.continue();
        if (req.method() !== 'POST') return route.fulfill({contentType:'application/json',body:'[]'});
        const body = req.postDataJSON();
        assert(!/^(send|submit)/i.test(body.method || ''), 'activity test must never submit a transaction');
        const result = body.method === 'getTransactionsByAddress' ? history('immediate-activity') : null;
        return route.fulfill({contentType:'application/json',body:JSON.stringify({jsonrpc:'2.0',id:body.id,result})});
    });
    await page.goto(`http://127.0.0.1:${port}/wallet/`, {waitUntil:'networkidle'});
    await page.evaluate(() => {
        walletState.wallets=[{id:'activity-test',name:'Public fixture',address:'public-observation'}];
        walletState.activeWalletId='activity-test';walletState.isLocked=false;
    });
}
async function main() {
    await new Promise(resolve => server.listen(0,'127.0.0.1',resolve));
    const port = server.address().port;
    const browser = await chromium.launch({headless:true});
    const context = await browser.newContext({serviceWorkers:'block'});
    try {
        const page = await context.newPage();await init(page,port);
        await page.evaluate(() => {
            window.balanceStarted=false;
            fetchLivePrices=() => new Promise(resolve => { window.releasePrices=resolve; });
            refreshBalance=async () => { window.balanceStarted=true; };
            void showDashboard();
        });
        await page.waitForFunction(() => document.querySelector('#activityList .activity-item'));
        assert.equal(await page.evaluate(() => window.balanceStarted),false,'activity must render while prices and balance are still pending');
        await page.close();

        const stale = await context.newPage();await init(stale,port);
        await stale.evaluate(() => {
            window.firstActivity=null;
            rpc.call=async () => new Promise(resolve => { window.firstActivity=resolve; });
            void loadActivity();
        });
        await stale.waitForFunction(() => window.firstActivity);
        await stale.evaluate(result => {
            rpc.call=async () => result;
            void loadActivity();
        },history('newer-request'));
        await stale.waitForFunction(() => document.querySelector('#activityList a')?.href.includes('newer-request'));
        await stale.evaluate(result => window.firstActivity(result),history('superseded-request'));
        await stale.waitForTimeout(100);
        assert.equal(await stale.locator('#activityList a[href*="superseded-request"]').count(),0,'late same-wallet request must not replace a newer page');
        await stale.close();

        const timed = await context.newPage();await init(timed,port);await timed.clock.install();
        await timed.evaluate(endpoint => { rpc.url=endpoint;void loadActivity(); },`http://127.0.0.1:${port}/activity-hang-fixture`);
        await timed.waitForFunction(() => document.readyState === 'complete');
        const deadline=Date.now()+5000;
        while (!partialBodies) { assert(Date.now()<deadline,'history request did not reach partial-response fixture');await new Promise(resolve=>setTimeout(resolve,20)); }
        await timed.clock.fastForward(21000);
        await timed.getByText('Activity unavailable',{exact:true}).waitFor({state:'attached'});
        assert.equal(await timed.locator('#activityList button[data-wallet-action="loadActivity"]').count(),1);
        // Exercise the extension's actual RPC class against the same incomplete
        // response body; no extension keys or signing requests are involved.
        const before = partialBodies;
        await timed.evaluate(async endpoint => {
            const {LichenRPC:ExtensionRPC}=await import('/wallet/extension/src/core/rpc-service.js');
            window.extensionRead=null;
            void new ExtensionRPC(endpoint).getTransactionsByAddress('public-observation',{limit:20})
                .then(()=>{window.extensionRead='unexpected-success';},e=>{window.extensionRead=e.name;});
        },`http://127.0.0.1:${port}/activity-hang-fixture`);
        const extensionDeadline=Date.now()+5000;
        while (partialBodies===before) { assert(Date.now()<extensionDeadline,'extension request missing');await new Promise(resolve=>setTimeout(resolve,20)); }
        await timed.clock.fastForward(21000);
        await timed.waitForFunction(() => window.extensionRead);
        assert.equal(await timed.evaluate(() => window.extensionRead),'AbortError');
        await timed.close();

        const faucet = await context.newPage();await init(faucet,port);await faucet.clock.install();
        const faucetBefore = partialBodies;
        await faucet.evaluate(endpoint => { LICHEN_CONFIG.faucet=endpoint;void loadActivity(); },`http://127.0.0.1:${port}`);
        const faucetDeadline=Date.now()+5000;
        while (partialBodies===faucetBefore) { assert(Date.now()<faucetDeadline,'faucet request missing');await new Promise(resolve=>setTimeout(resolve,20)); }
        await faucet.clock.fastForward(2100);
        await faucet.waitForFunction(() => document.querySelector('#activityList .activity-item'));
        await faucet.close();
    } finally { await context.close();await browser.close(); }

    const extension=path.join(root,'wallet/extension');
    const extensionContext=await chromium.launchPersistentContext('',{channel:'chromium',headless:true,args:[`--disable-extensions-except=${extension}`,`--load-extension=${extension}`]});
    try {
        const worker=extensionContext.serviceWorkers()[0] || await extensionContext.waitForEvent('serviceworker');
        const id=new URL(worker.url()).hostname;
        await worker.evaluate(()=>chrome.storage.local.set({lichenWalletState:{schemaVersion:2,wallets:[{id:'fixture',name:'Public fixture',address:'public-observation'}],activeWalletId:'fixture',isLocked:false,settings:{currency:'USD',lockTimeout:300000},network:{selected:'testnet'}}}));
        const page=await extensionContext.newPage();let balancePending=false;
        await page.route('https://**/*',async route=>{
            const body=route.request().postDataJSON() || {};
            assert(!/^(send|submit)/i.test(body.method || ''),'extension activity fixture must not submit transactions');
            if(body.method==='getBalance') { balancePending=true;return; }
            const result=body.method==='getTransactionsByAddress' ? history('extension-immediate') : null;
            await route.fulfill({contentType:'application/json',body:JSON.stringify({jsonrpc:'2.0',id:body.id || 1,result})});
        });
        await page.goto(`chrome-extension://${id}/src/pages/full.html`,{waitUntil:'domcontentloaded'});
        await page.waitForFunction(()=>document.querySelector('#activityList .activity-item'));
        assert(balancePending,'extension history must render while balance is pending');
        await page.close();
    } finally { await extensionContext.close(); }
    console.log('Wallet activity browser PASS: web/extension immediate history, stale response rejection, history response-body timeouts, and bounded faucet supplement.');
}
main().catch(error=>{console.error(error);process.exitCode=1;}).finally(()=>{for(const res of pending)res.destroy();server.close();});
