#!/usr/bin/env node

const assert = require('assert');
const fs = require('fs');
const path = require('path');
const { requirePrivateDocs } = require('./private_docs_guard');

const ROOT = path.join(__dirname, '..', '..');
const DRILL_DOC = path.join(ROOT, 'docs', 'deployment', 'STOLEN_FUNDS_WALLET_FREEZE_UNFREEZE_DRILL.md');
const INCIDENT_DOC = path.join(ROOT, 'docs', 'deployment', 'INCIDENT_RESPONSE_MODE.md');
const TRACKER = path.join(ROOT, 'docs', 'strategy', 'RESTRICTION_GOVERNANCE_TRACKER.md');

requirePrivateDocs('RG-805 Stolen Funds Wallet Drill Docs QA', [DRILL_DOC, INCIDENT_DOC, TRACKER]);

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

console.log('\nRG-805 Stolen Funds Wallet Drill Docs QA');

test('drill doc preserves testnet drill and mainnet incident network scope', () => {
  assertAllIncluded(
    sources.drill,
    [
      'Restriction governance itself is network-agnostic.',
      'RG-805 must run on testnet',
      'Mainnet must not use `testnet_drill`.',
      'A real stolen-funds mainnet incident uses the same `restrict-account`, approval, execute, verify, lift, approve, execute flow',
      'The RG-804 schema activation script is testnet-only. It does not make restriction governance testnet-only.',
      "LICHEN_RPC_URL='https://rpc.lichen.network'",
      '--reason stolen_funds',
      '--evidence-hash',
    ],
    'STOLEN_FUNDS_WALLET_FREEZE_UNFREEZE_DRILL.md'
  );
});

test('drill doc uses shipped CLI and public RPC surfaces only', () => {
  assertAllIncluded(
    sources.drill,
    [
      'lichen --rpc-url "$LICHEN_RPC_URL" restriction status account "$DRILL_ACCOUNT"',
      'lichen --rpc-url "$LICHEN_RPC_URL" restriction can-transfer "$DRILL_ACCOUNT" "$DRILL_RECIPIENT" --asset native --amount 1',
      'restriction build restrict-account "$DRILL_ACCOUNT"',
      '--mode outgoing-only',
      '--reason testnet_drill',
      'getGovernanceEvents',
      'restriction build lift-restriction "$RESTRICTION_ID"',
      '--lift-reason testnet_drill_complete',
      'restriction get "$RESTRICTION_ID"',
    ],
    'STOLEN_FUNDS_WALLET_FREEZE_UNFREEZE_DRILL.md'
  );
});

test('drill doc requires real transaction IDs and post-condition evidence', () => {
  assertAllIncluded(
    sources.drill,
    [
      'create_proposal_tx',
      'create_approval_tx',
      'create_execute_tx',
      'create_execute_auto_executed_by_approval',
      'lift_proposal_tx',
      'lift_approval_tx',
      'lift_execute_tx',
      'lift_execute_auto_executed_by_approval',
      'restriction_id',
      'blocked_can_transfer',
      'restored_can_transfer',
      'wallet_preflight_blocked',
      'extension_preflight_blocked',
      'wallet_preflight_restored',
      'extension_preflight_restored',
      'watchtower_created_alert',
      'watchtower_lifted_alert',
      'all six transaction hash fields are populated with real transaction IDs',
      'the approval and execute fields may intentionally contain the same transaction ID',
    ],
    'STOLEN_FUNDS_WALLET_FREEZE_UNFREEZE_DRILL.md'
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
    'STOLEN_FUNDS_WALLET_FREEZE_UNFREEZE_DRILL.md'
  );
  assertNotIncludes(sources.drill, 'LICHEN_ADMIN_TOKEN', 'STOLEN_FUNDS_WALLET_FREEZE_UNFREEZE_DRILL.md');
  assertNotIncludes(sources.drill, 'rm -rf', 'STOLEN_FUNDS_WALLET_FREEZE_UNFREEZE_DRILL.md');
  assertNotIncludes(sources.drill, 'rsync', 'STOLEN_FUNDS_WALLET_FREEZE_UNFREEZE_DRILL.md');
});

test('incident response playbook links the RG-805 drill record', () => {
  assertAllIncluded(
    sources.incident,
    [
      'STOLEN_FUNDS_WALLET_FREEZE_UNFREEZE_DRILL.md',
      'testnet/mainnet scope is explicit',
      'RG-805',
    ],
    'INCIDENT_RESPONSE_MODE.md'
  );
});

test('tracker records completed RG-805 live drill and next task', () => {
  assertAllIncluded(
    sources.tracker,
    [
      '| RG-805 | Done | Drill stolen-funds wallet freeze/unfreeze |',
      'no remaining strict-order RG task',
      '3c695aaf0144b1f88bd72ef9e22021bf916234b3bc664df3d196e51d85606db8',
      'b1be0e836d9874750de5e5926bbf8211ae232400ae08c9eb14d2a577aaec2e92',
      '3991ab7fcf503b5df8f4c56cbbd7c89c724154308ccebb542f1e4679f21b8b0d',
      'ba43ff7a28d5fd18d5f8d0752e1802af9c008cd3a2d54a8671b411ce1bab4702',
    ],
    'RESTRICTION_GOVERNANCE_TRACKER.md'
  );
});

console.log(`\nRG-805 Stolen Funds Wallet Drill Docs QA: ${passed} passed, ${failed} failed (${passed + failed} total)`);

if (failed > 0) {
  process.exit(1);
}
