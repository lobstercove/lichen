#!/usr/bin/env node

const assert = require('assert');
const fs = require('fs');
const path = require('path');

const repoRoot = path.join(__dirname, '..', '..');
const monitoringJs = fs.readFileSync(path.join(repoRoot, 'monitoring', 'js', 'monitoring.js'), 'utf8');

let passed = 0;
let failed = 0;

function test(name, fn) {
    try {
        fn();
        passed++;
        console.log(`  ✅ ${name}`);
    } catch (error) {
        failed++;
        console.log(`  ❌ ${name}: ${error.message}`);
    }
}

function extractFunctionBody(source, functionName) {
    const signatures = [`function ${functionName}(`, `async function ${functionName}(`];
    const start = signatures
        .map((signature) => source.indexOf(signature))
        .filter((index) => index >= 0)
        .sort((a, b) => a - b)[0] ?? -1;
    if (start === -1) return '';
    const bodyStart = source.indexOf('{', start);
    let depth = 0;
    for (let index = bodyStart; index < source.length; index++) {
        if (source[index] === '{') depth++;
        if (source[index] === '}') {
            depth--;
            if (depth === 0) return source.slice(bodyStart + 1, index);
        }
    }
    return '';
}

console.log('\n── Monitoring Risk Lifecycle QA ──');

test('RG-704 lifecycle constants use consensus proposal instruction types', () => {
    assert(monitoringJs.includes('const RISK_PROPOSAL_APPROVE_IX = 35'));
    assert(monitoringJs.includes('const RISK_PROPOSAL_EXECUTE_IX = 36'));
    assert(!monitoringJs.includes('LICHEN_ADMIN_TOKEN'));
});

test('RG-704 approval and execution data encodes proposal id as u64 little-endian', () => {
    const u64Body = extractFunctionBody(monitoringJs, 'riskU64LeBytes');
    const controlBody = extractFunctionBody(monitoringJs, 'buildRiskGovernanceControlTransaction');
    assert(u64Body.includes('numeric & 0xffn'));
    assert(u64Body.includes('numeric >>= 8n'));
    assert(controlBody.includes('data: [instructionType, ...riskU64LeBytes(proposalId)]'));
    assert(controlBody.includes('accounts: [signer]'));
});

test('RG-704 proposal creation/lift/extend submission requires signed payloads', () => {
    const submitPreviewBody = extractFunctionBody(monitoringJs, 'submitRiskSignedPreview');
    const submitTxBody = extractFunctionBody(monitoringJs, 'submitRiskSignedTransaction');
    assert(submitPreviewBody.includes('lastRiskSignedPreview?.signedTransactionBase64'));
    assert(submitTxBody.includes("rpc('sendTransaction', [signedBase64])"));
    assert(!/admin_token|LICHEN_ADMIN_TOKEN/.test(submitPreviewBody + submitTxBody));
});

test('RG-704 approval and execute actions sign locally before public RPC submission', () => {
    const runBody = extractFunctionBody(monitoringJs, 'runRiskGovernanceControlAction');
    assert(runBody.includes("action === 'execute' ? RISK_PROPOSAL_EXECUTE_IX : RISK_PROPOSAL_APPROVE_IX"));
    assert(runBody.includes('provider.signTransaction(controlTx)'));
    assert(runBody.includes('submitRiskSignedTransaction'));
    assert(!/licn_sendTransaction|sendRawTransaction|admin_token|LICHEN_ADMIN_TOKEN/.test(runBody));
});

test('RG-704 lift and extend workflow routes through shipped unsigned builders', () => {
    const builderBody = extractFunctionBody(monitoringJs, 'riskBuilderPreviewRequest');
    [
        'buildUnrestrictAccountTx',
        'buildUnrestrictAccountAssetTx',
        'buildResumeContractTx',
        'buildUnbanCodeHashTx',
        'buildResumeBridgeRouteTx',
        'buildLiftRestrictionTx',
        'buildExtendRestrictionTx',
    ].forEach((method) => assert(builderBody.includes(method), `${method} missing`));
});

console.log(`\nMonitoring risk lifecycle QA: ${passed} passed, ${failed} failed`);
if (failed > 0) process.exit(1);
