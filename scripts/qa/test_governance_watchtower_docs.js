#!/usr/bin/env node
'use strict';

const fs = require('fs');
const path = require('path');
const { requirePrivateDocs } = require('./private_docs_guard');

const ROOT = path.resolve(__dirname, '..', '..');
const DOC_PATH = path.join(ROOT, 'docs', 'deployment', 'GOVERNANCE_WATCHTOWER.md');
const WATCHTOWER_PATH = path.join(ROOT, 'scripts', 'governance-watchtower.js');
const WS_PATH = path.join(ROOT, 'rpc', 'src', 'ws.rs');

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

function extractAlertRuleIds(watchtower) {
    return Array.from(watchtower.matchAll(/\bid:\s*'([^']+)'/g), (match) => match[1]);
}

function main() {
    requirePrivateDocs('Governance Watchtower Docs QA', [DOC_PATH]);

    const docs = read(DOC_PATH);
    const watchtower = read(WATCHTOWER_PATH);
    const ws = read(WS_PATH);

    process.stdout.write('\nGovernance Watchtower Docs QA\n\n');

    test('operator docs preserve the non-mutating detection boundary', () => {
        [
            'The watchtower is detection and escalation only.',
            'must never auto-freeze',
            'auto-submit governance transactions',
            'Every restriction action still requires the signed governed transaction flow',
        ].forEach((needle) => assertIncludes(docs, needle, 'GOVERNANCE_WATCHTOWER.md'));
    });

    test('operator docs document shipped WS and RPC signal sources', () => {
        [
            'subscribeGovernance',
            'subscribeAccount',
            'subscribeTokenBalance',
            'getBalance',
            'getRewardAdjustmentInfo',
            'getTokenAccountsByOwner',
        ].forEach((needle) => assertIncludes(docs, needle, 'GOVERNANCE_WATCHTOWER.md'));
    });

    test('operator docs list every shipped governance alert rule id', () => {
        const ruleIds = extractAlertRuleIds(watchtower);
        assert(ruleIds.length >= 18, 'watchtower rule ID extraction returned too few rules');
        ruleIds.forEach((ruleId) => {
            assertIncludes(docs, `\`${ruleId}\``, 'GOVERNANCE_WATCHTOWER.md');
        });
    });

    test('operator docs list balance and canary alert rule ids', () => {
        [
            'native-account-outflow',
            'token-balance-outflow',
            'native-account-canary-touch',
            'token-balance-canary-touch',
        ].forEach((ruleId) => assertIncludes(docs, `\`${ruleId}\``, 'GOVERNANCE_WATCHTOWER.md'));
    });

    test('operator docs document restriction WS payload fields emitted by RPC', () => {
        [
            'approval_authority',
            'restriction_id',
            'restriction_status',
            'restriction_target_type',
            'restriction_target',
            'restriction_mode',
            'restriction_amount',
            'restriction_reason',
            'restriction_created_slot',
            'restriction_created_epoch',
            'restriction_expires_at_slot',
            'restriction_evidence_hash',
            'restriction_evidence_uri_hash',
            'restriction_supersedes',
            'restriction_lifted_by',
            'restriction_lifted_slot',
            'restriction_lift_reason',
        ].forEach((field) => {
            assertIncludes(ws, field, 'rpc/src/ws.rs');
            assertIncludes(docs, `\`${field}\``, 'GOVERNANCE_WATCHTOWER.md');
        });
    });

    test('operator docs document severity policy knobs and escalation behavior', () => {
        [
            'LICHEN_WATCHTOWER_LARGE_TRANSFER_SPORES',
            'LICHEN_WATCHTOWER_GUARDIAN_EXPIRY_WARNING_SLOTS',
            'at zero remaining slots',
            'executed transfers at or above',
            'canary',
            'critical',
        ].forEach((needle) => assertIncludes(docs, needle, 'GOVERNANCE_WATCHTOWER.md'));
    });

    test('operator docs tie restriction alerts to read-only verification commands', () => {
        [
            'lichen restriction get "$RESTRICTION_ID"',
            'lichen restriction list-active',
            'lichen restriction status account "$ACCOUNT"',
            'lichen restriction status contract "$CONTRACT"',
            'lichen restriction status code-hash "$CODE_HASH"',
            'lichen restriction status bridge-route "$CHAIN" "$ASSET"',
            'Do not treat an alert as evidence by itself.',
        ].forEach((needle) => assertIncludes(docs, needle, 'GOVERNANCE_WATCHTOWER.md'));
    });

    process.stdout.write(`\nGovernance Watchtower Docs QA: ${passed} passed, ${failed} failed\n`);
    if (failed > 0) {
        process.exitCode = 1;
    }
}

main();
