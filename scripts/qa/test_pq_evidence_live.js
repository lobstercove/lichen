#!/usr/bin/env node
'use strict';

const pq = require('./helpers/pq-node');
const {
    collectRouteHealthEvidence,
    hashCanonical,
    verifyEvidenceEnvelope,
} = require('../pq-evidence');

const RPC_URL = process.env.LICHEN_RPC_URL || process.env.LICHEN_WATCHTOWER_RPC_URL || 'http://127.0.0.1:8899';
const MANIFEST_HASH = hashCanonical({
    id: 'NX-850B-pq-attestation-watchtower',
    gate: 'NX-850C-live-local-e2e',
    version: 1,
});

let passed = 0;
let failed = 0;

function assert(condition, message) {
    if (condition) {
        passed += 1;
        process.stdout.write(`  PASS ${message}\n`);
        return;
    }
    failed += 1;
    process.stderr.write(`  FAIL ${message}\n`);
}

async function rpc(method, params = []) {
    const response = await fetch(RPC_URL, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ jsonrpc: '2.0', id: 1, method, params }),
    });
    const body = await response.json();
    if (body.error) {
        throw new Error(`${method}: ${body.error.message || JSON.stringify(body.error)}`);
    }
    return body.result;
}

async function main() {
    process.stdout.write(`\nPQ Evidence Live E2E (${RPC_URL})\n\n`);
    await pq.init();

    const slot = Number(await rpc('getSlot', []));
    assert(Number.isSafeInteger(slot) && slot > 0, `local RPC getSlot returns a live slot (${slot})`);

    const signers = [pq.generateKeypair(), pq.generateKeypair()];
    const evidence = await collectRouteHealthEvidence({
        rpcUrl: RPC_URL,
        routeHealthTargets: [
            { label: 'Neo X GAS', chain: 'neox', asset: 'gas', symbol: 'WGAS', statsMethod: 'getWgasStats' },
            { label: 'Neo X NEO', chain: 'neox', asset: 'neo', symbol: 'WNEO', statsMethod: 'getWneoStats' },
        ],
        issuedAtMs: Date.now(),
        ttlSlots: 150,
        manifestHash: MANIFEST_HASH,
        requiredSignatures: 2,
        signers,
        signMessage: (messageBytes, keypair) => pq.sign(messageBytes, keypair),
    });

    assert(evidence.length === 2, 'watchtower emits signed evidence for wGAS and wNEO routes');

    const trustedSigners = signers.map((signer) => signer.address);
    for (const envelope of evidence) {
        assert(envelope.payload.route_status && typeof envelope.payload.route_status === 'object',
            `${envelope.domain.asset} evidence includes route status`);
        assert(envelope.payload.wrapped_token_stats && typeof envelope.payload.wrapped_token_stats === 'object',
            `${envelope.domain.asset} evidence includes wrapped-token reserve stats`);
        const result = verifyEvidenceEnvelope(envelope, {
            expectedKind: 'route_health',
            expectedDomain: envelope.domain,
            currentSlot: slot,
            trustedSigners,
            requiredThreshold: 2,
            verifySignature: (messageBytes, signature, publicKeyBytes) => pq.verify(messageBytes, signature, publicKeyBytes),
        });
        assert(result.ok, `${envelope.domain.asset} evidence verifies with a 2-of-2 PQ quorum`);
    }

    process.stdout.write(`\nPQ Evidence Live E2E: PASS=${passed} FAIL=${failed}\n`);
    if (failed > 0) {
        process.exitCode = 1;
    }
}

main().catch((error) => {
    process.stderr.write(`PQ Evidence Live E2E failed: ${error.message}\n`);
    process.exitCode = 1;
});
