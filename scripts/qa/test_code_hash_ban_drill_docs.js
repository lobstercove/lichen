#!/usr/bin/env node

const assert = require('assert');
const fs = require('fs');
const path = require('path');
const { requirePrivateDocs } = require('./private_docs_guard');

const ROOT = path.join(__dirname, '..', '..');
const DRILL_DOC = path.join(ROOT, 'docs', 'deployment', 'CODE_HASH_BAN_UNBAN_DRILL.md');
const INCIDENT_DOC = path.join(ROOT, 'docs', 'deployment', 'INCIDENT_RESPONSE_MODE.md');
const TRACKER = path.join(ROOT, 'docs', 'strategy', 'RESTRICTION_GOVERNANCE_TRACKER.md');

requirePrivateDocs('RG-807 Code-Hash Ban Drill Docs QA', [DRILL_DOC, INCIDENT_DOC, TRACKER]);

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

console.log('\nRG-807 Code-Hash Ban Drill Docs QA');

test('drill doc preserves testnet drill and mainnet incident scope', () => {
  assertAllIncluded(
    sources.drill,
    [
      'Restriction governance is network-agnostic.',
      'RG-807 must run on testnet',
      'Mainnet must not use `testnet_drill`.',
      '`malicious_code_hash`',
      "LICHEN_RPC_URL='https://rpc.lichen.network'",
      '--reason malicious_code_hash',
      '--evidence-hash',
      'Do not delete chain state, copy validator state, or copy consensus WAL files for this drill.',
    ],
    'CODE_HASH_BAN_UNBAN_DRILL.md'
  );
});

test('drill doc uses shipped CLI, RPC, simulation, and watchtower surfaces', () => {
  assertAllIncluded(
    sources.drill,
    [
      'lichen --rpc-url "$LICHEN_RPC_URL" restriction status code-hash "$CODE_HASH"',
      'restriction build ban-code-hash "$CODE_HASH"',
      '--reason testnet_drill',
      'getGovernanceEvents',
      '`restriction-code-hash-ban`',
      'deploy simulation or deploy preflight',
      '`simulateTransaction`',
      'DeployBlocked',
      'restriction build unban-code-hash "$CODE_HASH"',
      '--lift-reason testnet_drill_complete',
    ],
    'CODE_HASH_BAN_UNBAN_DRILL.md'
  );
});

test('drill doc requires real transaction IDs and restored deployment evidence', () => {
  assertAllIncluded(
    sources.drill,
    [
      'baseline_deploy_tx',
      'create_proposal_tx',
      'create_approval_tx',
      'create_execute_tx',
      'create_execute_auto_executed_by_approval',
      'ban_proposal_id',
      'restriction_id',
      'deploy_blocked_status_verified',
      'blocked_deploy_rejected',
      'watchtower_code_hash_ban_alert',
      'lift_proposal_tx',
      'lift_approval_tx',
      'lift_execute_tx',
      'lift_execute_auto_executed_by_approval',
      'lift_proposal_id',
      'restored_deploy_tx',
      'the matching boolean is true',
    ],
    'CODE_HASH_BAN_UNBAN_DRILL.md'
  );
});

test('live record includes v0.5.36 transaction evidence and simulation parity fix', () => {
  assertAllIncluded(
    sources.drill,
    [
      'Live Testnet Record - v0.5.36',
      '/var/lib/lichen/rg807-v0.5.36-20260512T-live',
      '7ce3a9085b38f9a622ec8ec7cd953bceea4a0a479705b736a68c45f1d61c76ee',
      '2d6610b9735592bde0593c72e84a4860ec4555516495e2a540b05e2b5fb1446a',
      '94314f0b836e8e2001b8423bd08f3f3ec4bc56a8539d3cc9c48640e27fb4ba91',
      'a46da65194ceda919e80a45784085677422fe283f42d2617dbdfe4b932b53a6d',
      '48f24fcb0e2094e389fb584423182a624a57c233cc92529dc166f4d9fdc38485',
      '91942ee27226c5d844831916face3942a6dcf349efeac99d41f042b978d291fc',
      'simulateTransaction` treated deploy as `would deploy`',
      'cargo test -p lobstercove-lichen-core simulate_code_hash_deploy_block -- --nocapture',
      'cargo test -p lobstercove-lichen-core code_hash_deploy_block -- --nocapture',
    ],
    'CODE_HASH_BAN_UNBAN_DRILL.md'
  );
});

test('incident response playbook links the RG-807 drill record', () => {
  assertAllIncluded(
    sources.incident,
    [
      'CODE_HASH_BAN_UNBAN_DRILL.md',
      'RG-807',
      'baseline deployment',
      'active `deploy_blocked` status',
      'restored deployment',
      'deploy-simulation parity regression test',
      'For RG-805, RG-806, RG-807, RG-808, RG-809, and RG-810, testnet/mainnet scope is explicit.',
    ],
    'INCIDENT_RESPONSE_MODE.md'
  );
});

test('tracker marks RG-807 done with transaction IDs recorded', () => {
  assertAllIncluded(
    sources.tracker,
    [
      '| RG-807 | Done | Drill code-hash ban |',
      '7ce3a9085b38f9a622ec8ec7cd953bceea4a0a479705b736a68c45f1d61c76ee',
      '91942ee27226c5d844831916face3942a6dcf349efeac99d41f042b978d291fc',
      'no remaining strict-order RG task',
    ],
    'RESTRICTION_GOVERNANCE_TRACKER.md'
  );
});

test('drill doc preserves no privileged RPC, reset, or state-copy commands', () => {
  assertNotIncludes(sources.drill, 'LICHEN_ADMIN_TOKEN', 'CODE_HASH_BAN_UNBAN_DRILL.md');
  assertNotIncludes(sources.drill, 'rm -rf', 'CODE_HASH_BAN_UNBAN_DRILL.md');
  assertNotIncludes(sources.drill, 'rsync', 'CODE_HASH_BAN_UNBAN_DRILL.md');
});

console.log(`\nResult: ${passed} passed, ${failed} failed`);
if (failed > 0) process.exit(1);
