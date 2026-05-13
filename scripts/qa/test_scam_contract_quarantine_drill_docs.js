#!/usr/bin/env node

const assert = require('assert');
const fs = require('fs');
const path = require('path');
const { requirePrivateDocs } = require('./private_docs_guard');

const ROOT = path.join(__dirname, '..', '..');
const DRILL_DOC = path.join(ROOT, 'docs', 'deployment', 'SCAM_CONTRACT_QUARANTINE_RESUME_DRILL.md');
const INCIDENT_DOC = path.join(ROOT, 'docs', 'deployment', 'INCIDENT_RESPONSE_MODE.md');
const TRACKER = path.join(ROOT, 'docs', 'strategy', 'RESTRICTION_GOVERNANCE_TRACKER.md');

requirePrivateDocs('RG-806 Scam Contract Quarantine Drill Docs QA', [DRILL_DOC, INCIDENT_DOC, TRACKER]);

const sources = {
  drill: fs.readFileSync(DRILL_DOC, 'utf8'),
  incident: fs.readFileSync(INCIDENT_DOC, 'utf8'),
  tracker: fs.readFileSync(TRACKER, 'utf8'),
};

let passed = 0;
let failed = 0;

function test(name, fn) {
  try {
    fn();
    passed += 1;
    console.log(`  PASS ${name}`);
  } catch (error) {
    failed += 1;
    console.log(`  FAIL ${name}: ${error.message}`);
  }
}

function assertIncludes(source, needle, label) {
  const compactSource = source.replace(/\s+/g, ' ');
  const compactNeedle = needle.replace(/\s+/g, ' ');
  assert.ok(
    source.includes(needle) || compactSource.includes(compactNeedle),
    `${label} missing: ${needle}`
  );
}

function assertAllIncluded(source, needles, label) {
  for (const needle of needles) {
    assertIncludes(source, needle, label);
  }
}

function assertNotIncludes(source, needle, label) {
  assert.ok(!source.includes(needle), `${label} must not include: ${needle}`);
}

console.log('\nRG-806 Scam Contract Quarantine Drill Docs QA');

test('drill doc preserves testnet drill and mainnet incident scope', () => {
  assertAllIncluded(
    sources.drill,
    [
      'Restriction governance is network-agnostic.',
      'RG-806 must run on testnet',
      'Mainnet must not use `testnet_drill`.',
      'A real scam-contract incident uses the same `quarantine-contract`, approval, execute, verify, `resume-contract`, approve, execute flow',
      "LICHEN_RPC_URL='https://rpc.lichen.network'",
      '--reason scam_contract',
      '--evidence-hash',
      'throwaway drill contract must be newly deployed for the drill',
    ],
    'SCAM_CONTRACT_QUARANTINE_RESUME_DRILL.md'
  );
});

test('drill doc uses shipped CLI, RPC, extension, and watchtower surfaces', () => {
  assertAllIncluded(
    sources.drill,
    [
      'lichen --rpc-url "$LICHEN_RPC_URL" restriction status contract "$CONTRACT"',
      'lichen --rpc-url "$LICHEN_RPC_URL" call "$CONTRACT" total_supply --args',
      'restriction build quarantine-contract "$CONTRACT"',
      '--reason testnet_drill',
      'getGovernanceEvents',
      'restriction build resume-contract "$CONTRACT"',
      '--restriction-id "$RESTRICTION_ID"',
      '--lift-reason testnet_drill_complete',
      'extension transaction preflight blocks the contract call before signing',
      'watchtower emits restriction lifted and contract-resumed alerts',
    ],
    'SCAM_CONTRACT_QUARANTINE_RESUME_DRILL.md'
  );
});

test('drill doc requires real transaction IDs and lifecycle evidence', () => {
  assertAllIncluded(
    sources.drill,
    [
      'fund_tx',
      'deploy_tx',
      'pre_call_tx',
      'create_proposal_tx',
      'create_approval_tx',
      'create_execute_tx',
      'create_execute_auto_executed_by_approval',
      'lift_proposal_tx',
      'lift_approval_tx',
      'lift_execute_tx',
      'lift_execute_auto_executed_by_approval',
      'quarantine_proposal_id',
      'lift_proposal_id',
      'restriction_id',
      'quarantined_signed_call_blocked',
      'extension_quarantined_preflight_blocked',
      'extension_resumed_preflight_allowed',
      'watchtower_contract_resumed_alert',
      'all transaction hash fields are populated with real transaction IDs',
      'the approval and execute fields may intentionally contain the same transaction ID',
    ],
    'SCAM_CONTRACT_QUARANTINE_RESUME_DRILL.md'
  );
});

test('drill doc preserves no reset, no state-copy, no privileged RPC policy', () => {
  assertAllIncluded(
    sources.drill,
    [
      'This is not a reset runbook.',
      'Do not delete chain state, copy validator state, or copy consensus WAL files for this drill.',
      'no reset or state-copy operation occurred',
    ],
    'SCAM_CONTRACT_QUARANTINE_RESUME_DRILL.md'
  );
  assertNotIncludes(sources.drill, 'LICHEN_ADMIN_TOKEN', 'SCAM_CONTRACT_QUARANTINE_RESUME_DRILL.md');
  assertNotIncludes(sources.drill, 'rm -rf', 'SCAM_CONTRACT_QUARANTINE_RESUME_DRILL.md');
  assertNotIncludes(sources.drill, 'rsync', 'SCAM_CONTRACT_QUARANTINE_RESUME_DRILL.md');
});

test('live record contains the completed v0.5.25 testnet transaction evidence', () => {
  assertAllIncluded(
    sources.drill,
    [
      '/var/lib/lichen/rg806-v0.5.25-20260510T171027Z',
      '2a7vQwoCodhvdGYxiuo3vXeZ3cazFMdsqJK8A3qhBhe4',
      '2ea113681f4f721d6e7ca2bddcb8368604c3e03641f7ff3bad53b5bda0cd5a0d',
      '3f3f0d74c2b48b476fd28b9a10988c6a89a2d2f5575b9b1eded2e45a4aaf7c06',
      '47dc5753d46428d5b34efa691b8bf717daed7f3c2c9657bfa05d9e6e36877d17',
      '9f3780a22180466745c3172bc5e64e35885c81895a3368e3a646a6d68a918761',
      '496adf2da9807b3a6baf0fe8851f92f18025a3bcb7e31f4c9a99a98f7a14d50d',
      'a310ea552d597adf77748b7e90852e98c1ed7db802c16486235eba014fb61414',
      '`quarantine_proposal_id`: `3`',
      '`lift_proposal_id`: `4`',
      '`restriction_id`: `2`',
      'Contract lifecycle quarantined blocks execution',
      'restriction-contract-resumed',
    ],
    'SCAM_CONTRACT_QUARANTINE_RESUME_DRILL.md'
  );
});

test('incident response playbook links the RG-806 drill record', () => {
  assertAllIncluded(
    sources.incident,
    [
      'SCAM_CONTRACT_QUARANTINE_RESUME_DRILL.md',
      'RG-806',
      'throwaway contract deployment',
      'watchtower contract-resumed evidence',
      'testnet/mainnet scope is explicit',
    ],
    'INCIDENT_RESPONSE_MODE.md'
  );
});

test('tracker records completed RG-806 live drill and next task', () => {
  assertAllIncluded(
    sources.tracker,
    [
      '| RG-806 | Done | Drill scam contract quarantine/resume |',
      'no remaining strict-order RG task',
      '3f3f0d74c2b48b476fd28b9a10988c6a89a2d2f5575b9b1eded2e45a4aaf7c06',
      '47dc5753d46428d5b34efa691b8bf717daed7f3c2c9657bfa05d9e6e36877d17',
      '9f3780a22180466745c3172bc5e64e35885c81895a3368e3a646a6d68a918761',
      '496adf2da9807b3a6baf0fe8851f92f18025a3bcb7e31f4c9a99a98f7a14d50d',
    ],
    'RESTRICTION_GOVERNANCE_TRACKER.md'
  );
});

console.log(`\nRG-806 Scam Contract Quarantine Drill Docs QA: ${passed} passed, ${failed} failed (${passed + failed} total)`);

if (failed > 0) {
  process.exit(1);
}
