'use strict';
// Real service-worker lifecycle, offline failures and live faucet data. No keys or signing.
const { chromium } = require('playwright');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const http = require('node:http');
const path = require('node:path');
const root = path.resolve(__dirname, '../..');
const mime = {'.html':'text/html','.js':'text/javascript','.mjs':'text/javascript','.css':'text/css','.json':'application/json','.png':'image/png','.ico':'image/x-icon'};
let faucetRequests = 0;
const server = http.createServer((request, response) => {
    const url = new URL(request.url, 'http://localhost');
    if (url.pathname === '/wallet/pwa-fixture') {
        response.writeHead(200, {'Content-Type':'text/html'}).end('<title>Wallet PWA regression fixture</title>');
        return;
    }
    if (url.pathname === '/wallet/faucet/airdrops') {
        response.writeHead(200, {'Content-Type':'application/json','Cache-Control':'no-store'}).end(JSON.stringify({request:++faucetRequests}));
        return;
    }
    let file = path.resolve(root, '.' + decodeURIComponent(url.pathname));
    if (!file.startsWith(root + path.sep)) { response.writeHead(403).end(); return; }
    try {
        if (fs.statSync(file).isDirectory()) file = path.join(file,'index.html');
        response.writeHead(200, {'Content-Type':mime[path.extname(file)] || 'application/octet-stream'}).end(fs.readFileSync(file));
    } catch { response.writeHead(404).end(); }
});

async function main() {
    await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
    const origin = `http://127.0.0.1:${server.address().port}`;
    const browser = await chromium.launch({headless:true});
    const context = await browser.newContext();
    const errors = [];
    context.on('console', message=>{
        if (message.text().includes("Failed to convert value to 'Response'")) errors.push(message.text());
    });
    try {
        const page = await context.newPage();
        await page.goto(origin+'/wallet/pwa-fixture');
        await page.evaluate(async()=>{
            await caches.open('lichen-wallet-v7-20260908-polish');
            await navigator.serviceWorker.register('./sw.js');
            await navigator.serviceWorker.ready;
        });
        await page.waitForFunction(()=>navigator.serviceWorker.controller);
        assert.equal(await page.evaluate(async()=>(await caches.keys()).includes('lichen-wallet-v7-20260908-polish')),false);
        const faucet = origin+'/wallet/faucet/airdrops?address=public-fixture';
        await page.evaluate(async url=>{
            const [name] = await caches.keys();
            await (await caches.open(name)).put(url,new Response(JSON.stringify({request:'stale'}),{headers:{'Content-Type':'application/json'}}));
        },faucet);
        const live = await page.evaluate(async url=>[
            await (await fetch(url)).json(),await (await fetch(url)).json()
        ],faucet);
        assert.deepEqual(live,[{request:1},{request:2}],'faucet activity must bypass a stale asset-cache entry');
        await context.setOffline(true);
        assert(await page.evaluate(async()=>(await fetch('./wallet-layout.css')).ok),'cached wallet assets must remain available offline');
        for (const url of [origin+'/wallet/uncached.css','https://uncached.invalid/font.woff2']) {
            assert.equal(await page.evaluate(async url=>{
                try { await fetch(url); return 'unexpected success'; }
                catch(error) { return error.name; }
            },url),'TypeError','an uncached offline asset must produce a normal fetch failure');
        }
        await page.goto(origin+'/wallet/',{waitUntil:'domcontentloaded'});
        assert.equal(await page.title(),'Lichen Wallet — Your Gateway to Lichen');
        assert.deepEqual(errors,[],'service worker must not emit an invalid Response conversion');
        const output=path.join(root,'dist/wallet-browser');fs.mkdirSync(output,{recursive:true});
        fs.writeFileSync(path.join(output,'service-worker.json'),JSON.stringify({success:true,faucetRequests,offlineCachedAsset:true,offlineCacheMisses:2,offlineNavigation:true,invalidResponseErrors:errors},null,2)+'\n');
        console.log('Wallet service-worker browser regression checks passed');
    } finally { await context.close();await browser.close(); }
}
main().catch(error=>{console.error(error);process.exitCode=1;}).finally(()=>server.close());
