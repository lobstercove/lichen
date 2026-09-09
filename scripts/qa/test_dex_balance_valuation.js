#!/usr/bin/env node
'use strict';
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const source = fs.readFileSync(path.join(__dirname, '../../dex/dex.js'), 'utf8');

function declaration(name, next) {
    const start = source.indexOf(name);
    const end = source.indexOf(next, start + name.length);
    assert(start >= 0 && end > start, `Missing production function ${name}`);
    return source.slice(start, end);
}

async function run() {
    const context = vm.createContext({});
    vm.runInContext(`
        let balances = {}, result = {}, pairs = [], renders = 0;
        const state = {lastPrice:78393.62};
        const api = {rpc:async method => method === 'getBalance' ? result : {accounts:[]}};
        const renderBalances = () => renders++;
        ${declaration('async function loadBalances(', 'async function loadUserOrders(')}
        ${declaration('function computeTokenUsd(', 'function computePortfolioSummary(')}
        ${declaration('function computePortfolioSummary(', 'function computeUnrealizedPnl(')}
        globalThis.test = {loadBalances,computePortfolioSummary,
            setup:(response,markets,selectedPrice) => {result=response;pairs=markets;state.lastPrice=selectedPrice;},
            balances:()=>balances,renders:()=>renders};
    `, context);
    const h = context.test;
    const spores = 116670655400000;
    const markets = [{base:'BTC',quote:'lUSD',price:78393.62}, {base:'LICN',quote:'lUSD',price:0.15}];
    for (const response of [{spendable:spores,spores:spores*2}, {spores}]) {
        for (const selected of [78393.62,3000,0.15]) {
            h.setup(response, markets, selected);
            await h.loadBalances('synthetic-public-address');
            assert.equal(h.balances().LICN.available,116670.6554);
            assert(Math.abs(h.balances().LICN.usd-17500.59831)<1e-8);
            assert.equal(h.computePortfolioSummary().totalValue,h.balances().LICN.usd);
        }
    }
    h.setup({spendable:0,spores},markets,78393.62);
    await h.loadBalances('synthetic-public-address');
    assert.equal(h.balances().LICN.available,0,'zero spendable must not expose locked funds');
    assert.equal(h.balances().LICN.usd,0);
    h.setup({spores},[{base:'lUSD',quote:'LICN',price:1/0.15}],78393.62);
    await h.loadBalances('synthetic-public-address');
    assert(Math.abs(h.balances().LICN.usd-17500.59831)<1e-8,'inverse native quote');
    h.setup({spores},markets.slice(0,1),78393.62);
    await h.loadBalances('synthetic-public-address');
    assert.equal(h.balances().LICN.usd,0,'missing LICN quote cannot borrow another asset price');
    assert.equal(h.renders(),9);
    console.log('PASS DEX native balance and portfolio valuation: spendable, legacy, zero, inverse, missing quote, market independence');
}
run().catch(error => {console.error(error);process.exitCode=1;});
