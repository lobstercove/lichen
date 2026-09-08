#!/usr/bin/env node
'use strict';
// Disposable browser profiles and synthetic public balances only. No real signing.
const { chromium } = require('playwright');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const http = require('node:http');
const path = require('node:path');
const root = path.resolve(__dirname, '../..');
const output = process.env.WALLET_BROWSER_OUTPUT || path.join(root, 'dist/wallet-browser');
const expectedTabs = ['assets', 'activity', 'nfts', 'staking', 'identity', 'shield'];
const balance = { licn:'12480.52', spendable_licn:'12480.52', spores:12480520000000, spendable:12480520000000 };
const identityFixture = {identity:{name:'Main account',is_active:true,reputation:1500},achievements:[1,9,12,32,33,34,35,36,37,38,39,40,41].map(id=>({id})),skills:[],vouches:{received:[],given:[]}};
const mime = {'.html':'text/html','.js':'text/javascript','.mjs':'text/javascript','.css':'text/css','.json':'application/json','.png':'image/png','.ico':'image/x-icon'};
fs.mkdirSync(output, {recursive:true});
const server = http.createServer((request,response) => {
    const url = new URL(request.url, 'http://localhost');
    let file = path.resolve(root, '.' + decodeURIComponent(url.pathname));
    if (!file.startsWith(root + path.sep)) { response.writeHead(403).end(); return; }
    try {
        if (fs.statSync(file).isDirectory()) file = path.join(file, 'index.html');
        response.writeHead(200, {'Content-Type':mime[path.extname(file)] || 'application/octet-stream'});
        response.end(fs.readFileSync(file));
    } catch { response.writeHead(404).end(); }
});
async function routeFixtures(route) {
    const request = route.request(), url = new URL(request.url());
    if (['127.0.0.1','localhost','fonts.googleapis.com','fonts.gstatic.com','cdnjs.cloudflare.com'].includes(url.hostname)) return route.continue();
    if (url.pathname.endsWith('/coins/128x128/licn.png')) return route.fulfill({contentType:'image/png',body:fs.readFileSync(path.join(root,'website/assets/img/coins/128x128/licn.png'))});
    const payload = request.postDataJSON() || {};
    assert(!/^(send|submit)/i.test(payload.method || ''), 'browser layout test must never submit a transaction');
    let result = null;
    if (payload.method === 'getBalance') result = balance;
    if (payload.method === 'getNFTsByOwner') result = {nfts:[]};
    if (payload.method === 'getRecentTransactions') result = [];
    if (payload.method === 'getLichenIdProfile') result = identityFixture;
    return route.fulfill({contentType:'application/json',body:JSON.stringify({jsonrpc:'2.0',id:payload.id || 1,result})});
}
async function checkLayout(page, label, width, selectors) {
    const data = await page.evaluate(selectors => ({
        width:innerWidth, scroll:document.documentElement.scrollWidth,
        tabs:[...document.querySelectorAll(selectors.tabs)].map(e=>e.dataset.tab),
    }), selectors);
    assert(data.scroll <= width, `${label}: horizontal overflow ${data.scroll}/${width}`);
    assert.deepEqual(data.tabs, expectedTabs, `${label}: wallet sections must stay aligned`);
    const buttons = await page.locator('.balance-actions .action-btn').evaluateAll(elements=>elements.map(e=>{const b=e.getBoundingClientRect();return {width:b.width,height:b.height,top:b.top};}));
    if(buttons.length) {
        assert.equal(buttons.length,3);
        assert(buttons.every(b=>b.width===64 && b.height===64 && Math.abs(b.top-buttons[0].top)<1),label+': balance actions must be aligned squares');
    }
    await page.screenshot({path:path.join(output, label+'.png'),fullPage:true});
    await page.locator(selectors.tabs+'[data-tab="activity"]').click();
    assert(await page.locator(selectors.content+'[data-tab="activity"]').isVisible());
    await page.locator(selectors.tabs+'[data-tab="assets"]').click();
    assert(await page.locator('#assetsList').isVisible());
    return data;
}
async function checkAchievements(page,label,tabs) {
    await page.locator(tabs+'[data-tab="identity"]').click();
    const section=page.locator('.wallet-achievements');
    await section.waitFor({state:'visible'});
    assert.equal(await section.locator('.achievement-preview .achievement-badge').count(),4);
    assert((await section.boundingBox()).height<230,label+': collapsed achievements must stay compact');
    const summary=section.locator('summary');await summary.focus();await page.keyboard.press('Enter');
    assert.equal(await section.locator('details').getAttribute('open'),'');
    assert.equal(await section.locator('.achievement-grid .achievement-badge').count(),92);
    assert.equal(await section.locator('.achievement-preview').isVisible(),false);
    assert((await section.locator('.achievement-grid').boundingBox()).height<=300);
    await page.screenshot({path:path.join(output,label+'-achievements-expanded.png'),fullPage:true});
    await summary.click();assert.equal(await section.locator('details').getAttribute('open'),null);
    await page.screenshot({path:path.join(output,label+'-achievements.png'),fullPage:true});
    await page.locator(tabs+'[data-tab="assets"]').click();
}
async function popupLifecycle(context, port) {
    const origin = `http://127.0.0.1:${port}`, walletOrigin = `http://localhost:${port}`;
    const source = fs.readFileSync(path.join(root,'dex/shared/wallet-connect.js'),'utf8');
    const provider = source.slice(source.indexOf('var WALLET_POPUP_REQUEST_TARGET'),source.indexOf('function extensionOnlyWalletError'));
    await context.route(origin+'/session-fixture', route=>route.fulfill({contentType:'text/html',body:`<button id="connect">Connect</button><button id="sign">Sign</button><script>function getWalletAppUrl(){return new URL('${walletOrigin}/approval-fixture')}function getWalletPopupUrl(){return getWalletAppUrl()}${provider};window.p=new PopupLichenProvider();connect.onclick=()=>p.requestAccounts().then(x=>window.result=x);sign.onclick=()=>p.sendTransaction({message:{instructions:[]}}).then(x=>window.signed=x);</script>`}));
    await context.route(walletOrigin+'/approval-fixture', route=>route.fulfill({contentType:'text/html',body:`<button id="approve">Approve fixture</button><script>let request;addEventListener('message',e=>{if(e.source!==opener||e.origin!=='${origin}')return;request=e.data});approve.onclick=()=>opener.postMessage({target:'LICHEN_WEB_WALLET_RESPONSE',id:request.id,response:{ok:true,result:request.payload.method==='licn_requestAccounts'?['fixture-account']:{txHash:'fixture-signature'}}},'${origin}')</script>`}));
    const page = await context.newPage(); await page.goto(origin+'/session-fixture');
    for (const action of ['connect','sign']) {
        const opened = page.waitForEvent('popup'); await page.locator('#'+action).click(); const popup = await opened;
        await popup.waitForFunction(()=>typeof request !== 'undefined' && request);
        await popup.locator('#approve').click();
        await page.waitForFunction(action=>action==='connect' ? window.result?.[0]==='fixture-account' : window.signed?.txHash==='fixture-signature',action);
        await popup.close();await page.waitForFunction(()=>p.popup===null);
        assert.equal(await page.evaluate(()=>p.isConnected()),true);
        if (action==='connect') { await page.reload();assert.equal(await page.evaluate(()=>p.isConnected()),true); }
    }
    await page.evaluate(()=>p.disconnect());assert.equal(await page.evaluate(()=>p.isConnected()),false);await page.close();
}
async function main() {
    await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
    const port = server.address().port, results = [];
    const browser = await chromium.launch({headless:true});
    try {
        for (const [label,width,height] of [['desktop',1440,1000],['tablet',834,1112],['mobile',390,844],['small',320,740]]) {
            const context = await browser.newContext({viewport:{width,height},serviceWorkers:'block',reducedMotion:'reduce'});
            const page = await context.newPage(), errors=[];page.on('pageerror',e=>errors.push(e.message));
            await page.route('**/*',routeFixtures);await page.routeWebSocket('**/*',ws=>ws.close());
            await page.goto(`http://127.0.0.1:${port}/wallet/`,{waitUntil:'networkidle'});
            await page.evaluate(async ({balance,identityFixture})=>{
                walletState.wallets=[{id:'fixture',name:'Main account',address:'11111111111111111111111111111111'}];
                walletState.activeWalletId='fixture';walletState.isLocked=false;
                rpc.getBalance=async()=>balance;rpc.call=async method=>method==='getLichenIdProfile'?identityFixture:null;
                getAllTokenBalances=async()=>({});fetchWrappedReserveStats=async()=>({});fetchNeoGasRewardsSnapshot=async()=>null;
                showScreen('walletDashboard');setupDashboardTabs();setupWalletSelector();
                await refreshBalance();await loadAssets();
                document.getElementById('chainBlockHeight').textContent='Testnet · local fixture';
            }, {balance,identityFixture});
            results.push({label,...await checkLayout(page,label,width,{tabs:'.dashboard-tab',content:'.tab-content'})});
            await page.locator('[data-wallet-action="showReceive"]').first().click();assert(await page.locator('#receiveModal').isVisible());
            await page.screenshot({path:path.join(output,label+'-receive.png'),fullPage:true});
            await page.locator('#receiveModal [data-wallet-action="closeModal"]').click();
            await page.locator('[data-wallet-action="showSettings"]').click();assert(await page.locator('#settingsModal').isVisible());
            await page.locator('#settingsModal .modal-close').click();
            await checkAchievements(page,label,'.dashboard-tab');
            assert.deepEqual(errors,[],label+': browser exceptions');await context.close();
        }
        const context=await browser.newContext();await popupLifecycle(context,port);await context.close();
        const pwaContext=await browser.newContext();const pwa=await pwaContext.newPage();
        await pwa.route(`http://127.0.0.1:${port}/wallet/pwa-fixture`,route=>route.fulfill({contentType:'text/html',body:'<title>Disposable PWA update fixture</title>'}));
        await pwa.goto(`http://127.0.0.1:${port}/wallet/pwa-fixture`);
        await pwa.evaluate(async()=>{
            await caches.open('lichen-wallet-v5-20260830a');
            await navigator.serviceWorker.register('./sw.js');
            await navigator.serviceWorker.ready;
        });
        await pwa.waitForFunction(async()=>!(await caches.keys()).includes('lichen-wallet-v5-20260830a'));
        assert(await pwa.evaluate(async()=>!!(await caches.match('./wallet-layout.css'))),'PWA update must cache the new layout');
        await pwaContext.close();
    } finally {await browser.close();}
    const extension=path.join(root,'wallet/extension');
    const context=await chromium.launchPersistentContext('',{channel:'chromium',headless:true,args:[`--disable-extensions-except=${extension}`,`--load-extension=${extension}`]});
    try {
        const worker=context.serviceWorkers()[0] || await context.waitForEvent('serviceworker');
        const id=new URL(worker.url()).hostname;
        await worker.evaluate(()=>chrome.storage.local.set({lichenWalletState:{schemaVersion:2,wallets:[{id:'fixture',name:'Main account',address:'11111111111111111111111111111111'}],activeWalletId:'fixture',isLocked:false,settings:{currency:'USD',lockTimeout:300000},network:{selected:'testnet'}}}));
        for(const [label,view,width,height,popup] of [['extension-desktop','pages/full.html',1440,1000,false],['extension-mobile','pages/full.html',390,844,false],['extension-popup','popup/popup.html',400,600,true]]) {
            const page=await context.newPage();await page.setViewportSize({width,height});const errors=[];page.on('pageerror',e=>errors.push(e.message));
            await page.route('https://**/*',routeFixtures);
            await page.goto(`chrome-extension://${id}/src/${view}`,{waitUntil:'networkidle'});
            assert(await page.locator(popup?'#dashboardScreen':'#walletDashboard').isVisible());
            results.push({label,...await checkLayout(page,label,width,{tabs:popup?'.popup-dash-tab':'.dashboard-tab',content:popup?'.popup-tab-content':'.tab-content'})});
            if(!popup) await checkAchievements(page,label,'.dashboard-tab');
            assert.deepEqual(errors,[],label+': browser exceptions');await page.close();
        }
    } finally {await context.close();}
    fs.writeFileSync(path.join(output,'results.json'),JSON.stringify({success:true,popupLifecycle:true,pwaCacheMigration:true,results},null,2));
    console.log('Wallet browser PASS: four web viewports, three extension views, existing navigation/Receive/Settings, and cross-origin popup session lifecycle.');
}
main().catch(error=>{console.error(error);process.exitCode=1;}).finally(()=>server.close());
