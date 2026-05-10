#!/usr/bin/env node
'use strict';

const fs = require('fs');
const path = require('path');
const { requirePrivateDocs } = require('./private_docs_guard');

const ROOT = path.resolve(__dirname, '..', '..');

const files = {
    watchtower: path.join(ROOT, 'scripts', 'governance-watchtower.js'),
    watchtowerTest: path.join(ROOT, 'scripts', 'governance-watchtower.test.js'),
    ws: path.join(ROOT, 'rpc', 'src', 'ws.rs'),
    docs: path.join(ROOT, 'docs', 'deployment', 'GOVERNANCE_WATCHTOWER.md'),
};

let passed = 0;
let failed = 0;

function read(file) {
    return fs.readFileSync(file, 'utf8');
}

function test(name, fn) {
    try {
        fn();
        passed += 1;
        process.stdout.write(`  PASS ${name}\n`);
    } catch (error) {
        failed += 1;
        process.stderr.write(`  FAIL ${name}: ${error.message}\n`);
    }
}

function assert(condition, message) {
    if (!condition) {
        throw new Error(message);
    }
}

function assertIncludes(source, needle, label) {
    assert(source.includes(needle), `${label} missing '${needle}'`);
}

function main() {
    requirePrivateDocs('Governance Watchtower RG-705 Audit', [files.docs]);

    const watchtower = read(files.watchtower);
    const watchtowerTest = read(files.watchtowerTest);
    const ws = read(files.ws);
    const docs = read(files.docs);

    process.stdout.write('\nGovernance Watchtower RG-705 Audit\n\n');

    test('WS governance subscription carries restriction lifecycle fields', () => {
        [
            'approval_authority: Option<Pubkey>',
            'restriction_id: Option<u64>',
            'restriction_target_type: Option<String>',
            'restriction_target: Option<String>',
            'restriction_mode: Option<String>',
            'restriction_expires_at_slot: Option<u64>',
            'restriction_supersedes: Option<u64>',
            'restriction_lifted_by: Option<Pubkey>',
            '"restriction_lift_reason": event.restriction_lift_reason.as_ref()',
        ].forEach((needle) => assertIncludes(ws, needle, 'rpc/src/ws.rs'));
    });

    test('watchtower classifies required restriction proposal and lifecycle alerts', () => {
        [
            'restriction-proposal-created',
            'restriction-proposal-approved',
            'restriction-proposal-executed',
            'restriction-lifecycle-created',
            'restriction-lifecycle-extended',
            'restriction-lifecycle-lifted',
            'restriction-guardian-near-expiry',
            'restriction-contract-resumed',
            'restriction-code-hash-ban',
            'restricted-interaction-attempt',
        ].forEach((needle) => assertIncludes(watchtower, needle, 'scripts/governance-watchtower.js'));
    });

    test('watchtower uses structured restriction fields instead of log text inference', () => {
        [
            'function restrictionField(event, field)',
            'restriction_target_type',
            'restriction_expires_at_slot',
            'restrictionExpiryRemainingSlots',
            'LICHEN_WATCHTOWER_GUARDIAN_EXPIRY_WARNING_SLOTS',
        ].forEach((needle) => assertIncludes(watchtower, needle, 'scripts/governance-watchtower.js'));
    });

    test('watchtower tests cover restriction lifecycle WS consumption', () => {
        [
            'sampleRestrictionEvent',
            'testEndToEndRestrictionLifecycleWsConsumption',
            'restriction lifecycle lift is classified',
            'contract quarantine lift is classified as a resumed contract',
            'code-hash deploy bans are classified',
            'temporary split-authority restriction near expiry is classified',
        ].forEach((needle) => assertIncludes(watchtowerTest, needle, 'scripts/governance-watchtower.test.js'));
    });

    test('operator docs document RG-705 restriction alert coverage', () => {
        [
            'restriction proposal creation, approval, and execution',
            'restriction create, extend, and lift lifecycle events',
            'temporary split-authority restrictions that are near expiry',
            'contracts resumed after a quarantine restriction is lifted',
            'code-hash deploy bans',
            'LICHEN_WATCHTOWER_GUARDIAN_EXPIRY_WARNING_SLOTS',
        ].forEach((needle) => assertIncludes(docs, needle, 'docs/deployment/GOVERNANCE_WATCHTOWER.md'));
    });

    process.stdout.write(`\nGovernance Watchtower RG-705 Audit: ${passed} passed, ${failed} failed\n`);
    if (failed > 0) {
        process.exitCode = 1;
    }
}

main();
