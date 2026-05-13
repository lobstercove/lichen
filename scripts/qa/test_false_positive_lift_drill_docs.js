#!/usr/bin/env node

const assert = require('assert');
const fs = require('fs');
const path = require('path');
const { requirePrivateDocs } = require('./private_docs_guard');

const ROOT = path.join(__dirname, '..', '..');
const DRILL_DOC = path.join(ROOT, 'docs', 'deployment', 'FALSE_POSITIVE_LIFT_DRILL.md');
const INCIDENT_DOC = path.join(ROOT, 'docs', 'deployment', 'INCIDENT_RESPONSE_MODE.md');
const TRACKER = path.join(ROOT, 'docs', 'strategy', 'RESTRICTION_GOVERNANCE_TRACKER.md');

requirePrivateDocs('RG-810 False-Positive Lift Drill Docs QA', [DRILL_DOC, INCIDENT_DOC, TRACKER]);

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

console.log('\nRG-810 False-Positive Lift Drill Docs QA');

test('drill doc preserves testnet create and mainnet recovery scope', () => {
  assertAllIncluded(
    sources.drill,
    [
      'Restriction governance is network-agnostic.',
      'RG-810 must run on testnet',
      'temporary `testnet_drill` restriction',
      '`false_positive` to exercise the production false-positive recovery reason',
      'Mainnet must not use `testnet_drill` for restriction creation',
      "LICHEN_RPC_URL='https://rpc.lichen.network'",
      '--lift-reason false_positive',
      'Do not delete chain state, copy validator state, or copy consensus WAL files for this drill.',
    ],
    'FALSE_POSITIVE_LIFT_DRILL.md'
  );
});

test('drill doc verifies active block and restored operation after lift', () => {
  assertAllIncluded(
    sources.drill,
    [
      '`can-transfer` is blocked for outgoing movement',
      'restriction build unrestrict-account',
      'lift_reason=false_positive',
      'effective_status=lifted',
      '`can-transfer` is allowed again',
      '`restriction_lifted`',
      'restored_transfer_allowed',
    ],
    'FALSE_POSITIVE_LIFT_DRILL.md'
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
      'lift_execute_auto_executed_by_approval=true',
    ],
    'FALSE_POSITIVE_LIFT_DRILL.md'
  );
});

test('live record includes v0.5.36 transaction evidence and restored operation', () => {
  assertAllIncluded(
    sources.drill,
    [
      'Live Testnet Record - v0.5.36',
      '/var/lib/lichen/rg810-v0.5.36-20260513T084440Z-live/evidence.json',
      '2b1c40a68bd9900801a164247e8c442ef4d8d078999638b45ffac844c3623efd',
      '77b917010612924e17fe9436e4a1190b0afd9fe25418c297f96bcb3f5bda09df',
      '9c21c4dd93a27ebe855f6736ce05a3748007a9a6b5bae837571ea61eaf4c7e54',
      '4be8dd9f996ee6b27d5b52b8571cca3fb1c3efead970785398e404cc250cef06',
      '`create_proposal_id`: `15`',
      '`lift_proposal_id`: `16`',
      '`restriction_id`: `4`',
      'allowed=false',
      'source_blocked=true',
      'lift_reason=false_positive',
      'allowed=true',
      'source_blocked=false',
      'No reset or state-copy operation occurred.',
    ],
    'FALSE_POSITIVE_LIFT_DRILL.md'
  );
});

test('incident response playbook links RG-810 false-positive lift drill', () => {
  assertAllIncluded(
    sources.incident,
    [
      'FALSE_POSITIVE_LIFT_DRILL.md',
      'RG-810',
      '`lift_reason=false_positive`',
      'restored account status and `can-transfer` preflight',
      'For RG-805, RG-806, RG-807, RG-808, RG-809, and RG-810, testnet/mainnet scope is explicit.',
    ],
    'INCIDENT_RESPONSE_MODE.md'
  );
});

test('tracker marks RG-810 done and closes the RG queue', () => {
  assertAllIncluded(
    sources.tracker,
    [
      '| RG-810 | Done | Drill false-positive lift |',
      'no remaining strict-order RG task',
      '2b1c40a68bd9900801a164247e8c442ef4d8d078999638b45ffac844c3623efd',
      '77b917010612924e17fe9436e4a1190b0afd9fe25418c297f96bcb3f5bda09df',
      '9c21c4dd93a27ebe855f6736ce05a3748007a9a6b5bae837571ea61eaf4c7e54',
      '4be8dd9f996ee6b27d5b52b8571cca3fb1c3efead970785398e404cc250cef06',
      'proposal `15`',
      'proposal `16`',
      'restriction `4`',
      '`lift_reason=false_positive`',
      'restored transfer preflight',
    ],
    'RESTRICTION_GOVERNANCE_TRACKER.md'
  );
});

test('drill doc preserves no privileged RPC, reset, or state-copy commands', () => {
  assertNotIncludes(sources.drill, 'LICHEN_ADMIN_TOKEN', 'FALSE_POSITIVE_LIFT_DRILL.md');
  assertNotIncludes(sources.drill, 'rm -rf', 'FALSE_POSITIVE_LIFT_DRILL.md');
  assertNotIncludes(sources.drill, 'rsync', 'FALSE_POSITIVE_LIFT_DRILL.md');
});

console.log(`\nResult: ${passed} passed, ${failed} failed`);
if (failed > 0) process.exit(1);
