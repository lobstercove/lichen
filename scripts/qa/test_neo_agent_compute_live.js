#!/usr/bin/env node
'use strict';

const http = require('http');
const https = require('https');

const RPCS = (process.env.LICHEN_RPCS || process.env.LICHEN_RPC || 'http://127.0.0.1:8899')
    .split(',')
    .map((value) => value.trim())
    .filter(Boolean);

let passed = 0;
let failed = 0;

function assert(condition, message) {
    if (condition) {
        passed += 1;
        console.log(`  PASS ${message}`);
        return;
    }
    failed += 1;
    console.error(`  FAIL ${message}`);
}

function rpc(url, method, params = []) {
    return new Promise((resolve, reject) => {
        const payload = JSON.stringify({ jsonrpc: '2.0', id: 1, method, params });
        const parsedUrl = new URL(url);
        const client = parsedUrl.protocol === 'https:' ? https : http;
        const req = client.request(parsedUrl, {
            method: 'POST',
            headers: { 'content-type': 'application/json' },
            timeout: 10_000,
        }, (res) => {
            let body = '';
            res.on('data', (chunk) => { body += chunk; });
            res.on('end', () => {
                try {
                    const parsed = JSON.parse(body);
                    if (parsed.error) {
                        reject(new Error(parsed.error.message || JSON.stringify(parsed.error)));
                        return;
                    }
                    resolve(parsed.result);
                } catch (error) {
                    reject(error);
                }
            });
        });
        req.on('error', reject);
        req.on('timeout', () => {
            req.destroy(new Error(`timeout calling ${method}`));
        });
        req.write(payload);
        req.end();
    });
}

async function checkEndpoint(url) {
    console.log(`\nNeo agent-compute live check: ${url}`);
    const health = await rpc(url, 'getHealth').catch((error) => ({ error: error.message }));
    assert(health && !health.error, `${url} health responds`);

    const stats = await rpc(url, 'getComputeMarketStats');
    assert(Object.prototype.hasOwnProperty.call(stats, 'agent_payments_enabled'), `${url} exposes agent_payments_enabled`);
    assert(Object.prototype.hasOwnProperty.call(stats, 'agent_route_paused'), `${url} exposes agent_route_paused`);
    assert(Object.prototype.hasOwnProperty.call(stats, 'agent_policy_count'), `${url} exposes agent_policy_count`);
    assert(Object.prototype.hasOwnProperty.call(stats, 'agent_payment_count'), `${url} exposes agent_payment_count`);
    assert(Object.prototype.hasOwnProperty.call(stats, 'agent_blocked_payment_count'), `${url} exposes agent_blocked_payment_count`);
    assert(stats.agent_payments_enabled === false, `${url} agent payments are fail-closed by default`);
    assert(stats.agent_payment_count === 0, `${url} has no agent payment mutations by default`);
}

async function main() {
    for (const endpoint of RPCS) {
        await checkEndpoint(endpoint);
    }
    console.log(`\nNeo agent-compute live: ${passed} passed, ${failed} failed`);
    if (failed > 0) {
        process.exitCode = 1;
    }
}

main().catch((error) => {
    console.error(`Neo agent-compute live check failed: ${error.stack || error.message}`);
    process.exitCode = 1;
});
