#!/usr/bin/env node

const assert = require('assert');
const fs = require('fs');
const path = require('path');
const { requirePrivateDocs } = require('./private_docs_guard');

const ROOT = path.join(__dirname, '..', '..');
const DRILL_DOC = path.join(ROOT, 'docs', 'deployment', 'GUARDIAN_EXPIRY_DRILL.md');
const INCIDENT_DOC = path.join(ROOT, 'docs', 'deployment', 'INCIDENT_RESPONSE_MODE.md');
const TRACKER = path.join(ROOT, 'docs', 'strategy', 'RESTRICTION_GOVERNANCE_TRACKER.md');

requirePrivateDocs('RG-809 Guardian Expiry Drill Docs QA', [DRILL_DOC, INCIDENT_DOC, TRACKER]);

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

console.log('\nRG-809 Guardian Expiry Drill Docs QA');

test('drill doc preserves testnet drill and mainnet incident scope', () => {
  assertAllIncluded(
    sources.drill,
    [
      'Restriction governance is network-agnostic.',
      'RG-809 must run on testnet',
      'Mainnet must not use `testnet_drill`',
      "LICHEN_RPC_URL='https://rpc.lichen.network'",
      '--evidence-hash',
      'Do not delete chain state, copy validator state, or copy consensus WAL files for this drill.',
    ],
    'GUARDIAN_EXPIRY_DRILL.md'
  );
});

test('drill doc verifies natural expiry without lift', () => {
  assertAllIncluded(
    sources.drill,
    [
      'Do not lift the restriction.',
      'effective_status=expired',
      'expired_transfer_allowed_without_lift',
      'no_lift_transaction_used',
      'No lift transaction was used.',
      '`can-transfer` is allowed',
      '`getGovernanceEvents` includes `created`, `approved`, `executed`, and `restriction_created`',
    ],
    'GUARDIAN_EXPIRY_DRILL.md'
  );
});

test('drill doc records shipped signer/control path and manual program-id guard', () => {
  assertAllIncluded(
    sources.drill,
    [
      'Lichen system program ID `11111111111111111111111111111111`',
      'instruction type `35` for approval',
      'or `36` for execute',
      "Do not use Solana's nonzero system-program bytes.",
      'create_execute_auto_executed_by_approval=true',
    ],
    'GUARDIAN_EXPIRY_DRILL.md'
  );
});

test('live record includes v0.5.36 expiry evidence', () => {
  assertAllIncluded(
    sources.drill,
    [
      'Live Testnet Record - v0.5.36',
      '/var/lib/lichen/rg809-v0.5.36-20260512T230448Z-live/evidence.json',
      'd3fc05a8ee22ed316689ee179bb63b7f1044f73f272a4ab19f309086ebd50968',
      'f7dfbdce997b0a0ae35362da105374a0420198ecadb6367adf01be2417d750a1',
      '`proposal_id`: `14`',
      '`restriction_id`: `3`',
      'expiry slot `274066`',
      'At slot `274067`, expiry was observed.',
      'allowed=false',
      'source_blocked=true',
      'allowed=true',
      'source_blocked=false',
      'proposal `13` was a preliminary created-only attempt that expired unexecuted',
      'No reset or state-copy operation occurred.',
    ],
    'GUARDIAN_EXPIRY_DRILL.md'
  );
});

test('incident response playbook links RG-809 guardian expiry drill', () => {
  assertAllIncluded(
    sources.incident,
    [
      'GUARDIAN_EXPIRY_DRILL.md',
      'RG-809',
      'natural `effective_status=expired`',
      'without a lift transaction',
      'For RG-805, RG-806, RG-807, RG-808, RG-809, and RG-810, testnet/mainnet scope is explicit.',
    ],
    'INCIDENT_RESPONSE_MODE.md'
  );
});

test('tracker marks RG-809 done with expiry evidence', () => {
  assertAllIncluded(
    sources.tracker,
    [
      '| RG-809 | Done | Drill guardian expiry |',
      'no remaining strict-order RG task',
      'd3fc05a8ee22ed316689ee179bb63b7f1044f73f272a4ab19f309086ebd50968',
      'f7dfbdce997b0a0ae35362da105374a0420198ecadb6367adf01be2417d750a1',
      'proposal `14`',
      'restriction `3`',
      '`effective_status=expired`',
      'without a lift transaction',
    ],
    'RESTRICTION_GOVERNANCE_TRACKER.md'
  );
});

test('drill doc preserves no privileged RPC, reset, state-copy, or lift command in expiry path', () => {
  assertNotIncludes(sources.drill, 'LICHEN_ADMIN_TOKEN', 'GUARDIAN_EXPIRY_DRILL.md');
  assertNotIncludes(sources.drill, 'rm -rf', 'GUARDIAN_EXPIRY_DRILL.md');
  assertNotIncludes(sources.drill, 'rsync', 'GUARDIAN_EXPIRY_DRILL.md');
  assertNotIncludes(sources.drill, 'testnet_drill_complete', 'GUARDIAN_EXPIRY_DRILL.md');
});

console.log(`\nResult: ${passed} passed, ${failed} failed`);
if (failed > 0) process.exit(1);
