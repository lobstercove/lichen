#!/usr/bin/env node
'use strict';
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const read = file => fs.readFileSync(path.join(__dirname, '../..', file), 'utf8');
const source = read('dex/shared/wallet-connect.js');
const origin = 'https://dex.lichen.network';
const walletOrigin = 'https://wallet.lichen.network';
const plain = value => JSON.parse(JSON.stringify(value));
function storage() {
    const data = new Map();
    return {data, getItem: k => data.get(k) || null, setItem: (k,v) => data.set(k,v), removeItem: k => data.delete(k)};
}
function element(tag) {
    return {tag, children: [], events: {}, removed: false, textContent: '',
        append(...children) { this.children.push(...children); },
        addEventListener(name, callback) { this.events[name] = callback; },
        remove() { this.removed = true; }, showModal() { this.visible = true; }};
}
function harness(sessionStorage = storage()) {
    let now = 100000, blocked = false, opened = 0;
    const timers = new Map(), listeners = {}, popups = [], body = element('body');
    let nextTimer = 0;
    const window = {location: {origin}, addEventListener: (name, fn) => { listeners[name] = fn; },
        open() {
            opened++;
            if (blocked) return null;
            const popup = {closed:false, posts:[], focus(){}, postMessage(data,target) { this.posts.push({data,target}); }};
            popups.push(popup); return popup;
        }};
    const context = vm.createContext({window, sessionStorage, localStorage: storage(), URL, Map, Set, Promise,
        document:{body, createElement:element}, Date:{now:()=>now},
        getWalletAppUrl:()=>new URL(walletOrigin), getWalletPopupUrl:()=>new URL(walletOrigin+'/?bridge=popup'),
        setInterval:fn=>{timers.set(++nextTimer,fn);return nextTimer;}, clearInterval:id=>timers.delete(id),
        setTimeout:fn=>{timers.set(++nextTimer,fn);return nextTimer;}, clearTimeout:id=>timers.delete(id)});
    vm.runInContext(source.slice(source.indexOf('var WALLET_POPUP_REQUEST_TARGET'), source.indexOf('function extensionOnlyWalletError')), context);
    const provider = new context.PopupLichenProvider();
    const response = (popup,result,request=popup.posts.at(-1).data, from=popup) => listeners.message({origin:walletOrigin,source:from,
        data:{target:'LICHEN_WEB_WALLET_RESPONSE',id:request.id,response:{ok:true,result}}});
    return {provider, context, sessionStorage, body, popups, response, timers,
        get opened(){return opened;}, setBlocked:v=>{blocked=v;}, advance:ms=>{now+=ms;},
        async connect() { const promise=provider.requestAccounts();response(provider.popup,['account-A']);await promise; },
        event(name,payload,from=provider.popup,eventOrigin=walletOrigin) {
            listeners.message({origin:eventOrigin,source:from,data:{target:'LICHEN_WEB_WALLET_EVENT',event:name,payload}});
        }};
}
function declaration(source, marker) {
    const start=source.indexOf(marker), open=source.indexOf('{',start);
    assert(start>=0,marker); let depth=0;
    for(let i=open;i<source.length;i++) {
        if(source[i]==='{')depth++;
        if(source[i]==='}' && --depth===0)return source.slice(start,i+1);
    }
    throw new Error(marker);
}
async function run() {
    const h=harness(), p=h.provider;
    await h.connect();
    const expiry=p._lastState.expiresAt;
    let disconnects=0;p.on('disconnect',()=>disconnects++);
    p.popup.closed=true;p._handlePopupClosed();
    assert.equal((await p.getProviderState()).connected,true);
    assert.deepEqual(plain(await p.accounts()),['account-A']);
    assert.equal(await p.isConnected(),true);
    assert.equal(h.opened,1,'polling closed popup must not reopen it');
    assert.equal(disconnects,0,'popup close must not emit disconnect');
    const restored=harness(h.sessionStorage);
    assert.equal((await restored.provider.getProviderState()).connected,true,'reload in same tab retains connection');
    assert.equal((await harness().provider.getProviderState()).connected,false,'new tab has no connection');
    const fields=Object.keys(JSON.parse([...h.sessionStorage.data.values()][0])).sort();
    assert.deepEqual(fields,['accounts','canRequestSignatures','chainId','connected','expiresAt','hasWallet','isLocked','network','origin','providerType'].sort());

    const signing=p.sendTransaction({message:{instructions:[]}});
    const popup=p.popup, request=popup.posts.at(-1).data;
    h.response(popup,{txHash:'forged'},request,{});
    assert.equal(p._pending.size,1,'another same-origin window cannot answer a request');
    h.event('disconnect',{},{});assert.equal(p._lastState.connected,true);
    h.event('disconnect',{},popup,'https://evil.example');assert.equal(p._lastState.connected,true);
    h.response(popup,{txHash:'approved'});
    assert.equal((await signing).txHash,'approved');
    assert.equal(h.opened,2,'signing reopens the wallet');

    h.advance(1000);
    const stateRequest=p.getProviderState();
    h.response(popup,{connected:true,accounts:['account-A'],hasWallet:true,isLocked:true,canRequestSignatures:true});
    const state=await stateRequest;
    assert.equal(state.expiresAt,expiry,'polls must not extend expiry');
    assert.equal(h.opened,2,'background state poll does not call window.open');
    const dexContext=vm.createContext({normalizeWalletAddress:v=>v, Boolean, Object, Array});
    vm.runInContext(declaration(read('dex/dex.js'),'async function readWalletProviderSnapshot('),dexContext);
    const snapshot=await dexContext.readWalletProviderSnapshot('web-wallet', {isWindowOpen:()=>false,getProviderState:async()=>state});
    assert.equal(snapshot.connected,true);assert.equal(snapshot.activeAddress,'account-A');assert.equal(snapshot.isLocked,false);
    const extension=await dexContext.readWalletProviderSnapshot('extension',{getProviderState:async()=>state});
    assert.equal(extension.isLocked,true,'extension lock semantics remain enforced');

    const interrupted=p.sendTransaction({message:{instructions:['old']}});
    const interruptedCheck=assert.rejects(interrupted,/window closed/);
    const oldPopup=p.popup;oldPopup.closed=true;
    // Reopen before the 300ms close monitor runs: old request must never replay.
    const next=p.sendTransaction({message:{instructions:['new']}});
    await interruptedCheck;
    assert.equal(p._pending.size,1);
    assert.equal(p.popup.posts.length,1);
    h.response(oldPopup,{txHash:'late'});
    assert.equal(p._pending.size,1,'late prior-window response cannot fulfill new request');
    h.response(p.popup,{txHash:'new'});await next;
    p.popup.closed=true;p._handlePopupClosed();
    h.advance(30*60*1000);
    assert.equal((await p.getProviderState()).connected,false,'session expires without popup');
    assert.equal(h.sessionStorage.data.size,0);
    assert.equal(disconnects,1);
    await h.connect();p.popup.closed=true;p._handlePopupClosed();await p.disconnect();
    assert.equal((await p.getProviderState()).connected,false,'explicit disconnect clears connection');

    const blocked=harness();await blocked.connect();
    blocked.provider.popup.closed=true;blocked.setBlocked(true);
    const deferred=blocked.provider.sendTransaction({message:{instructions:['once']}});
    const dialog=blocked.body.children.at(-1);
    assert(dialog.visible);assert.equal(blocked.provider._pending.size,1);
    blocked.setBlocked(false);dialog.children[1].events.click();
    assert(dialog.removed);assert.equal(blocked.provider.popup.posts.length,1);
    blocked.response(blocked.provider.popup,{txHash:'one'});await deferred;
    blocked.provider.popup.closed=true;blocked.setBlocked(true);
    const cancelled=blocked.provider.sendTransaction({message:{instructions:[]}});
    blocked.body.children.at(-1).children[2].events.click();
    await assert.rejects(cancelled,/User rejected/);assert.equal(blocked.provider._pending.size,0);
    assert.equal((await blocked.provider.getProviderState()).connected,true,'blocked popup cancellation preserves connection');

    // Execute actual wallet bridge: locked dashboard can sign only after explicit
    // password approval. No unlock flag, secret persistence or automatic signing.
    const walletState={wallets:[{}],isLocked:true,network:'testnet'};
    let approval={password:'correct'}, prompts=0, decrypts=0, signatures=0;
    const replies=[], approvedOrigins=storage();
    const bridgeContext=vm.createContext({URL, URLSearchParams, Map, Set, Date, Uint8Array, TextEncoder,
        window:{opener:{},location:{origin:walletOrigin,search:'?bridge=popup'},addEventListener(){}},
        localStorage:approvedOrigins,document:{getElementById:()=>null},setInterval(){},walletState,
        getActiveWallet:()=>({address:'account-A',encryptedKey:'encrypted',name:'Wallet'}),
        showToast(){},showPasswordModal:async()=>{prompts++;return approval;},
        LichenCrypto:{decryptPrivateKey:async(_key,password)=>{decrypts++;if(password!=='correct')throw new Error('Wrong password');return 'secret';},
            signTransaction:async()=>{signatures++;return {sig:'signature'};}}});
    const bridge=read('wallet/js/dapp-bridge.js').replace(/\}\)\(\);\s*$/, 'globalThis.test={processRequest,approveOrigin,buildProviderState};})();');
    vm.runInContext(bridge,bridgeContext);
    bridgeContext.test.approveOrigin(origin);
    assert.equal(bridgeContext.test.buildProviderState(origin).accounts[0],'account-A');
    async function sign() {
        const request={id:'request-'+prompts,origin,source:{postMessage:v=>replies.push(v)},createdAt:Date.now(),
            payload:{method:'licn_signMessage',params:[{message:'hello'}]}};
        await bridgeContext.test.processRequest(request);return replies.at(-1).response;
    }
    assert.equal((await sign()).ok,true);assert.equal(prompts,1);assert.equal(signatures,1);
    assert.equal(walletState.isLocked,true,'signature approval does not unlock the dashboard');
    approval=null;assert.equal((await sign()).ok,false);assert.equal(decrypts,1);
    approval={password:'wrong'};assert.equal((await sign()).ok,false);assert.equal(signatures,1);
    approval={};assert.equal((await sign()).ok,false);assert.equal(decrypts,2);
    assert(!JSON.stringify([...approvedOrigins.data]).includes('secret'));
    console.log('Web-wallet session lifecycle PASS: close/reopen, reload, expiry, disconnect, origin/source binding, cancellation, blocked popup, DEX gating and password approval.');
}
run().catch(error=>{console.error(error);process.exitCode=1;});
