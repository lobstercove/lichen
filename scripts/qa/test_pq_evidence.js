#!/usr/bin/env node
'use strict';

const http = require('http');
const pq = require('./helpers/pq-node');
const {
    createUnsignedEvidence,
    collectRouteHealthEvidence,
    hashCanonical,
    normalizeEvidenceDomain,
    signEvidenceEnvelope,
    stableStringify,
    verifyEvidenceEnvelope,
} = require('../pq-evidence');

let passed = 0;
let failed = 0;

function assert(condition, message) {
    if (condition) {
        passed += 1;
        process.stdout.write(`  ✓ ${message}\n`);
        return;
    }
    failed += 1;
    process.stderr.write(`  ✗ ${message}\n`);
}

function assertEqual(actual, expected, message) {
    const left = JSON.stringify(actual);
    const right = JSON.stringify(expected);
    assert(left === right, `${message}${left === right ? '' : `: expected ${right}, got ${left}`}`);
}

function assertThrows(fn, pattern, message) {
    try {
        fn();
        assert(false, `${message}: expected throw`);
    } catch (error) {
        assert(pattern.test(error.message), `${message}: ${error.message}`);
    }
}

function clone(value) {
    return JSON.parse(JSON.stringify(value));
}

function domain(overrides = {}) {
    return normalizeEvidenceDomain({
        lichenNetwork: 'local',
        neoNetwork: 't4',
        neoChainId: '12227332',
        route: 'neox',
        asset: 'gas',
        purpose: 'neo-x-pq-watchtower',
        ...overrides,
    });
}

function basePayload(overrides = {}) {
    return {
        source: 'governance-watchtower',
        evidence_type: 'route_health',
        health: 'critical',
        target: {
            label: 'Neo X GAS',
            chain: 'neox',
            asset: 'gas',
            symbol: 'WGAS',
            stats_method: 'getWgasStats',
        },
        route_status: {
            chain: 'neox',
            asset: 'gas',
            route_paused: false,
            active_restriction_ids: [],
        },
        wrapped_token_stats: {
            supply: '1000',
            reserve_attested: '900',
            attestation_count: '1',
            paused: false,
        },
        alerts: [
            {
                rule_id: 'wrapped-reserve-deficit',
                severity: 'critical',
                title: 'Wrapped token reserve deficit',
                event: {
                    event: 'WrappedTokenHealth',
                    asset: 'gas',
                    chain: 'neox',
                    deficit: '100',
                },
            },
        ],
        ...overrides,
    };
}

function unsignedEvidence(overrides = {}) {
    return createUnsignedEvidence({
        kind: 'route_health',
        domain: domain(),
        payload: basePayload(),
        slot: 100,
        issuedAtMs: 1_700_000_000_000,
        expiresAtSlot: 250,
        manifestHash: hashCanonical({ id: 'NX-850B-pq-attestation-watchtower', version: 1 }),
        requiredSignatures: 2,
        ...overrides,
    });
}

function signEvidence(evidence, signers) {
    return signEvidenceEnvelope(evidence, signers, {
        signMessage: (messageBytes, keypair) => pq.sign(messageBytes, keypair),
    });
}

function verifyEvidence(evidence, trustedSigners, options = {}) {
    return verifyEvidenceEnvelope(evidence, {
        expectedKind: 'route_health',
        expectedDomain: domain(),
        currentSlot: 120,
        trustedSigners,
        requiredThreshold: 2,
        verifySignature: (messageBytes, signature, publicKeyBytes) => pq.verify(messageBytes, signature, publicKeyBytes),
        ...options,
    });
}

async function testCanonicalization() {
    console.log('\n── PQ1: canonical evidence encoding ──');
    assertEqual(
        stableStringify({ b: 1, a: { d: 4, c: 3 }, z: ['x', { b: true, a: null }] }),
        '{"a":{"c":3,"d":4},"b":1,"z":["x",{"a":null,"b":true}]}',
        'canonical JSON sorts nested object keys deterministically',
    );
    assertThrows(
        () => stableStringify({ ok: true, bad: undefined }),
        /undefined/,
        'canonical JSON rejects undefined fields instead of silently dropping them',
    );
}

async function testQuorumVerification() {
    console.log('\n── PQ2: signed evidence quorum verification ──');
    const signers = [pq.generateKeypair(), pq.generateKeypair(), pq.generateKeypair()];
    const signed = signEvidence(unsignedEvidence(), signers.slice(0, 2));
    const result = verifyEvidence(signed, signers.map((signer) => signer.address));
    assertEqual(result.ok, true, '2-of-3 signed evidence verifies');
    assertEqual(result.requiredThreshold, 2, 'consumer policy threshold is enforced');
    assertEqual(result.validSigners.length, 2, 'verification reports the unique valid signer set');

    const tamperedPayload = clone(signed);
    tamperedPayload.payload.wrapped_token_stats.reserve_attested = '1000';
    assertThrows(
        () => verifyEvidence(tamperedPayload, signers.map((signer) => signer.address)),
        /payload_hash/,
        'payload mutation is rejected by payload hash verification',
    );

    const tamperedSignature = clone(signed);
    const sig = tamperedSignature.signatures[0].signature.sig;
    tamperedSignature.signatures[0].signature.sig = `${sig[0] === '0' ? '1' : '0'}${sig.slice(1)}`;
    assertThrows(
        () => verifyEvidence(tamperedSignature, signers.map((signer) => signer.address)),
        /signature mismatch/,
        'signature mutation is rejected',
    );

    const insufficient = signEvidence(unsignedEvidence({ requiredSignatures: 1 }), [signers[0]]);
    assertThrows(
        () => verifyEvidence(insufficient, signers.map((signer) => signer.address)),
        /quorum not met/,
        'insufficient signer quorum is rejected',
    );

    const duplicateSigner = clone(signed);
    duplicateSigner.signatures = [duplicateSigner.signatures[0], duplicateSigner.signatures[0]];
    assertThrows(
        () => verifyEvidence(duplicateSigner, signers.map((signer) => signer.address)),
        /duplicate signer/,
        'duplicate signer entries do not satisfy quorum',
    );

    assertThrows(
        () => verifyEvidence(signed, [signers[2].address]),
        /Trusted signer set/,
        'trusted signer policy rejects a signer set below the threshold',
    );
}

async function testReplayBoundaries() {
    console.log('\n── PQ3: replay and domain boundaries ──');
    const signers = [pq.generateKeypair(), pq.generateKeypair()];
    const signed = signEvidence(unsignedEvidence(), signers);
    const trusted = signers.map((signer) => signer.address);

    assertThrows(
        () => verifyEvidence(signed, trusted, { currentSlot: 251 }),
        /stale/,
        'stale evidence is rejected after expires_at_slot',
    );
    assertThrows(
        () => verifyEvidence(signed, trusted, {
            expectedDomain: domain({ asset: 'neo' }),
        }),
        /domain/,
        'cross-asset or cross-domain replay is rejected',
    );
    assertThrows(
        () => verifyEvidence(signed, trusted, {
            expectedDomain: domain({ neoChainId: '47763' }),
        }),
        /domain/,
        'cross-chain replay is rejected',
    );

    const seenEvidenceIds = new Set([signed.evidence_id]);
    assertThrows(
        () => verifyEvidence(signed, trusted, { seenEvidenceIds }),
        /already been consumed/,
        'duplicate evidence IDs are rejected by consumer replay memory',
    );
}

async function testWatchtowerRouteEvidence() {
    console.log('\n── PQ4: watchtower route evidence generation ──');
    const observedMethods = [];
    const rpcServer = http.createServer((req, res) => {
        let body = '';
        req.on('data', (chunk) => {
            body += chunk;
        });
        req.on('end', () => {
            const payload = JSON.parse(body || '{}');
            observedMethods.push(payload.method);
            res.setHeader('Content-Type', 'application/json');
            if (payload.method === 'getSlot') {
                res.end(JSON.stringify({ jsonrpc: '2.0', id: payload.id || 1, result: 777 }));
                return;
            }
            if (payload.method === 'getBridgeRouteRestrictionStatus') {
                res.end(JSON.stringify({
                    jsonrpc: '2.0',
                    id: payload.id || 1,
                    result: {
                        chain: payload.params[0],
                        asset: payload.params[1],
                        route_paused: false,
                        active_restriction_ids: [],
                    },
                }));
                return;
            }
            if (payload.method === 'getWgasStats') {
                res.end(JSON.stringify({
                    jsonrpc: '2.0',
                    id: payload.id || 1,
                    result: {
                        supply: '1000',
                        reserve_attested: '900',
                        attestation_count: '1',
                        paused: false,
                    },
                }));
                return;
            }
            res.end(JSON.stringify({ jsonrpc: '2.0', id: payload.id || 1, result: null }));
        });
    });
    await new Promise((resolve) => rpcServer.listen(0, '127.0.0.1', resolve));

    const signers = [pq.generateKeypair(), pq.generateKeypair()];
    try {
        const evidence = await collectRouteHealthEvidence({
            rpcUrl: `http://127.0.0.1:${rpcServer.address().port}`,
            routeHealthTargets: [
                { label: 'Neo X GAS', chain: 'neox', asset: 'gas', symbol: 'WGAS', statsMethod: 'getWgasStats' },
            ],
            issuedAtMs: 1_700_000_000_000,
            ttlSlots: 20,
            manifestHash: hashCanonical({ id: 'NX-850B-pq-attestation-watchtower', version: 1 }),
            requiredSignatures: 2,
            signers,
            signMessage: (messageBytes, keypair) => pq.sign(messageBytes, keypair),
        });

        assertEqual(evidence.length, 1, 'watchtower emits one route evidence envelope for the configured Neo X route');
        assert(
            observedMethods.includes('getSlot')
            && observedMethods.includes('getBridgeRouteRestrictionStatus')
            && observedMethods.includes('getWgasStats'),
            'watchtower evidence reads only shipped slot, route, and wrapped-token RPC surfaces',
        );
        assert(
            evidence[0].payload.alerts.some((alert) => alert.rule_id === 'wrapped-reserve-deficit'),
            'watchtower evidence preserves route-health alert context',
        );
        const result = verifyEvidence(evidence[0], signers.map((signer) => signer.address), {
            currentSlot: 778,
        });
        assertEqual(result.ok, true, 'signed watchtower route evidence verifies through the consumer verifier');
    } finally {
        await new Promise((resolve) => rpcServer.close(resolve));
    }
}

async function main() {
    await pq.init();
    await testCanonicalization();
    await testQuorumVerification();
    await testReplayBoundaries();
    await testWatchtowerRouteEvidence();

    process.stdout.write(`\n═══ PQ Evidence: ${passed} passed, ${failed} failed ═══\n`);
    if (failed > 0) {
        process.exitCode = 1;
    }
}

main().catch((error) => {
    process.stderr.write(`PQ evidence tests failed: ${error.message}\n`);
    process.exitCode = 1;
});
