#!/usr/bin/env node
'use strict';

const http = require('http');
const RPC = 'http://15.204.229.189:8899';

function rpc(method, params) {
    return new Promise((resolve, reject) => {
        const body = JSON.stringify({ jsonrpc: '2.0', id: 1, method, params });
        const url = new URL(RPC);
        const req = http.request({
            hostname: url.hostname,
            port: url.port,
            path: '/',
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
        }, (res) => {
            let d = '';
            res.on('data', c => d += c);
            res.on('end', () => {
                try { resolve(JSON.parse(d)); }
                catch (e) { reject(e); }
            });
        });
        req.on('error', reject);
        req.end(body);
    });
}

const ALICE = '6pSZMuzshpcAWSnUEYuoNbJ251xuCSUoAYiWENfk6Hi';
const BOB = '5h2cL2LCqmVgNwW39UVkwGrk83u9ypRtzkTSynTit4S';
const sleep = ms => new Promise(r => setTimeout(r, ms));

(async () => {
    // Check current balances
    const a0 = await rpc('getBalance', [ALICE]);
    const b0 = await rpc('getBalance', [BOB]);
    console.log('Start Alice:', a0.result ? a0.result.spendable_licn + ' LICN' : 'ERR');
    console.log('Start Bob:', b0.result ? b0.result.spendable_licn + ' LICN' : 'ERR');

    // Fund both wallets: 13 rounds of 10 LICN each
    for (let i = 0; i < 13; i++) {
        const ra = await rpc('requestAirdrop', [ALICE, 10]);
        const aOk = ra.result && ra.result.success;
        const rb = await rpc('requestAirdrop', [BOB, 10]);
        const bOk = rb.result && rb.result.success;

        const aMsg = aOk ? 'OK' : (ra.error ? ra.error.message : 'FAIL');
        const bMsg = bOk ? 'OK' : (rb.error ? rb.error.message : 'FAIL');
        console.log(`Round ${i + 1}/13: Alice=${aMsg} Bob=${bMsg}`);

        if (!aOk && !bOk) {
            console.log('Both failed, stopping');
            break;
        }
        if (i < 12) {
            console.log('  Waiting 61s for cooldown...');
            await sleep(61000);
        }
    }

    // Check final balances
    await sleep(3000);
    const aBal = await rpc('getBalance', [ALICE]);
    const bBal = await rpc('getBalance', [BOB]);
    console.log('Final Alice:', aBal.result ? aBal.result.spendable_licn + ' LICN' : 'ERR');
    console.log('Final Bob:', bBal.result ? bBal.result.spendable_licn + ' LICN' : 'ERR');
})().catch(console.error);
