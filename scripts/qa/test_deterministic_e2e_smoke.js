#!/usr/bin/env node
'use strict';

const http = require('http');
const https = require('https');

const RPC_URL = process.env.RPC_URL || 'http://127.0.0.1:8899';
const EXPECT_CHAIN_ID = process.env.E2E_EXPECT_CHAIN_ID || '';
const HEX_32 = /^[0-9a-f]{64}$/i;

let rpcId = 1;
let passed = 0;
let failed = 0;

function sleep(ms) {
    return new Promise((resolve) => setTimeout(resolve, ms));
}

function pass(label) {
    passed += 1;
    console.log(`  PASS ${label}`);
}

function fail(label) {
    failed += 1;
    console.log(`  FAIL ${label}`);
}

function check(condition, label) {
    if (condition) {
        pass(label);
    } else {
        fail(label);
    }
}

function rpc(method, params = []) {
    const body = JSON.stringify({ jsonrpc: '2.0', id: rpcId, method, params });
    rpcId += 1;

    return new Promise((resolve, reject) => {
        const url = new URL(RPC_URL);
        const transport = url.protocol === 'https:' ? https : http;
        const request = transport.request(
            url,
            {
                method: 'POST',
                headers: {
                    'Content-Type': 'application/json',
                    'Content-Length': Buffer.byteLength(body),
                },
            },
            (response) => {
                let raw = '';
                response.on('data', (chunk) => {
                    raw += chunk;
                });
                response.on('end', () => {
                    let payload;
                    try {
                        payload = JSON.parse(raw);
                    } catch (error) {
                        reject(new Error(`${method} returned invalid JSON: ${raw.slice(0, 120)}`));
                        return;
                    }

                    if (payload.error) {
                        reject(new Error(`${method} RPC error ${payload.error.code}: ${payload.error.message}`));
                        return;
                    }

                    resolve(payload.result);
                });
            },
        );

        request.on('error', reject);
        request.setTimeout(10_000, () => {
            request.destroy(new Error(`${method} timed out`));
        });
        request.write(body);
        request.end();
    });
}

function getValidatorList(result) {
    if (Array.isArray(result)) {
        return result;
    }
    if (result && Array.isArray(result.validators)) {
        return result.validators;
    }
    return [];
}

function getRegistryEntries(result) {
    if (Array.isArray(result)) {
        return result;
    }
    if (result && Array.isArray(result.entries)) {
        return result.entries;
    }
    return [];
}

async function main() {
    console.log(`Deterministic local E2E smoke (${RPC_URL})`);

    const health = await rpc('getHealth');
    check(health && health.status === 'ok', 'getHealth reports ok');
    check(Number.isInteger(health.slot) && health.slot >= 0, 'getHealth returns a numeric slot');

    const network = await rpc('getNetworkInfo');
    check(typeof network.chain_id === 'string' && network.chain_id.length > 0, 'getNetworkInfo returns chain_id');
    check(network.chain_id === network.network_id, 'chain_id and network_id match');
    if (EXPECT_CHAIN_ID) {
        check(network.chain_id === EXPECT_CHAIN_ID, `chain_id matches ${EXPECT_CHAIN_ID}`);
    }
    check(Number.isInteger(network.validator_count) && network.validator_count >= 1, 'validator_count is at least one');
    check(Number.isInteger(network.peer_count) && network.peer_count >= 0, 'peer_count is non-negative');

    const slotA = await rpc('getSlot');
    await sleep(500);
    const slotB = await rpc('getSlot');
    check(Number.isInteger(slotA) && slotA >= 0, 'getSlot returns a numeric slot');
    check(Number.isInteger(slotB) && slotB >= slotA, 'slot is monotonic across reads');

    const recent = await rpc('getRecentBlockhash');
    check(recent && HEX_32.test(recent.blockhash), 'recent blockhash is canonical 32-byte hex');
    check(Number.isInteger(recent.slot) && recent.slot >= slotA, 'recent blockhash slot is current');

    const latest = await rpc('getLatestBlock');
    check(latest && Number.isInteger(latest.slot) && latest.slot > 0, 'latest block has a positive slot');
    check(latest && HEX_32.test(latest.hash), 'latest block hash is canonical 32-byte hex');
    check(latest && HEX_32.test(latest.state_root), 'latest block state root is canonical 32-byte hex');
    check(latest && !/^0+$/.test(latest.state_root), 'latest block state root is nonzero');

    const block = await rpc('getBlock', [latest.slot]);
    check(block && block.slot === latest.slot, 'getBlock(latest.slot) returns the requested slot');
    check(block && block.hash === latest.hash, 'getBlock(latest.slot) matches latest block hash');
    check(block && block.state_root === latest.state_root, 'getBlock(latest.slot) matches latest state root');

    const validators = getValidatorList(await rpc('getValidators'));
    check(validators.length >= 1, 'getValidators returns at least one validator');
    check(validators.length === network.validator_count, 'validator list length matches network validator_count');
    check(validators.every((validator) => typeof validator.pubkey === 'string' && validator.pubkey.length >= 32), 'validators have public keys');

    const entries = getRegistryEntries(await rpc('getAllSymbolRegistry'));
    const symbols = new Set(entries.map((entry) => entry.symbol));
    for (const symbol of ['DEX', 'LUSD', 'ORACLE', 'YID']) {
        check(symbols.has(symbol), `symbol registry includes ${symbol}`);
    }
    check(entries.every((entry) => typeof entry.program === 'string' && entry.program.length >= 32), 'symbol registry entries have program ids');

    console.log(`\nDeterministic local E2E smoke: ${passed} passed, ${failed} failed`);
    if (failed > 0) {
        process.exit(1);
    }
}

main().catch((error) => {
    console.error(`Deterministic local E2E smoke failed: ${error.stack || error.message}`);
    process.exit(1);
});
