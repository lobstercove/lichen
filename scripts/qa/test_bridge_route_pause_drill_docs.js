#!/usr/bin/env node

const assert = require('assert');
const fs = require('fs');
const path = require('path');
const { requirePrivateDocs } = require('./private_docs_guard');

const ROOT = path.join(__dirname, '..', '..');
const DRILL_DOC = path.join(ROOT, 'docs', 'deployment', 'BRIDGE_ROUTE_PAUSE_RESUME_DRILL.md');
const INCIDENT_DOC = path.join(ROOT, 'docs', 'deployment', 'INCIDENT_RESPONSE_MODE.md');
const TRACKER = path.join(ROOT, 'docs', 'strategy', 'RESTRICTION_GOVERNANCE_TRACKER.md');

requirePrivateDocs('RG-808 Bridge Route Pause Drill Docs QA', [DRILL_DOC, INCIDENT_DOC, TRACKER]);

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

console.log('\nRG-808 Bridge Route Pause Drill Docs QA');

test('drill doc preserves testnet drill and mainnet incident scope', () => {
  assertAllIncluded(
    sources.drill,
    [
      'Restriction governance is network-agnostic.',
      'RG-808 must run on testnet',
      'Mainnet must not use `testnet_drill`.',
      '`bridge_compromise`',
      "LICHEN_RPC_URL='https://rpc.lichen.network'",
      '--reason bridge_compromise',
      '--evidence-hash',
      'Do not delete chain state, copy validator state, or copy consensus WAL files for this drill.',
    ],
    'BRIDGE_ROUTE_PAUSE_RESUME_DRILL.md'
  );
});

test('drill doc uses shipped CLI, RPC, custody, and watchtower surfaces', () => {
  assertAllIncluded(
    sources.drill,
    [
      'lichen --rpc-url "$LICHEN_RPC_URL" restriction status bridge-route "$CHAIN" "$ASSET"',
      'restriction build pause-bridge-route "$CHAIN" "$ASSET"',
      '--reason testnet_drill',
      'getLichenBridgeStats',
      'getGovernanceEvents',
      'createBridgeDeposit',
      'RoutePaused',
      'restriction build resume-bridge-route "$CHAIN" "$ASSET"',
      '--lift-reason testnet_drill_complete',
      'CUSTODY_URL=http://127.0.0.1:9105',
      'CUSTODY_API_AUTH_TOKEN',
    ],
    'BRIDGE_ROUTE_PAUSE_RESUME_DRILL.md'
  );
});

test('drill doc requires real transaction IDs and restored deposit evidence', () => {
  assertAllIncluded(
    sources.drill,
    [
      'pause_proposal_tx',
      'pause_approval_tx',
      'pause_execute_tx',
      'pause_execute_auto_executed_by_approval',
      'pause_proposal_id',
      'restriction_id',
      'paused_route_status_verified',
      'paused_deposit_rejected',
      'paused_deposit_error',
      'resume_proposal_tx',
      'resume_approval_tx',
      'resume_execute_tx',
      'resume_execute_auto_executed_by_approval',
      'resume_proposal_id',
      'final_route_paused',
      'restored_deposit_allowed_public_and_all_direct',
      'deposit ID/address on public RPC and each direct VPS RPC',
      'the matching boolean is true',
    ],
    'BRIDGE_ROUTE_PAUSE_RESUME_DRILL.md'
  );
});

test('live record includes v0.5.36 transaction evidence and deployment parity remediation', () => {
  assertAllIncluded(
    sources.drill,
    [
      'Live Testnet Record - v0.5.36',
      '/var/lib/lichen/rg808-v0.5.36-20260512T1123Z-live/evidence.json',
      '21eb03b04b61a14f85f48fed47045841bcd2ce9bf7caa7a297564a35c447e819',
      '18e2b6e0d7b9759f1bba09a72dd0155d6e83469c684febb4fda252a5c9bbf314',
      'fd424726aac22ee05a81586c9710107775c788c50cce906437ed4b270ab548b6',
      'afe7d2a89d64084efda94a8899229321d910a174c601d5d65e9819681fa827c1',
      '`pause_proposal_id`: `11`',
      '`resume_proposal_id`: `12`',
      '`restriction_id`: `2`',
      'createBridgeDeposit rejected: bridge route solana:sol is paused by active RoutePaused restriction 2',
      'validator service envs lacked `CUSTODY_URL`/`CUSTODY_API_AUTH_TOKEN`',
      'custody envs had stale or missing wrapped-token route mappings',
      'No reset or state-copy operation occurred.',
    ],
    'BRIDGE_ROUTE_PAUSE_RESUME_DRILL.md'
  );
});

test('incident response playbook links the RG-808 drill record', () => {
  assertAllIncluded(
    sources.incident,
    [
      'BRIDGE_ROUTE_PAUSE_RESUME_DRILL.md',
      'RG-808',
      'paused deposit rejection',
      'restored deposit creation',
      'custody service-env parity',
      'For RG-805, RG-806, RG-807, RG-808, RG-809, and RG-810, testnet/mainnet scope is explicit.',
    ],
    'INCIDENT_RESPONSE_MODE.md'
  );
});

test('tracker marks RG-808 done with transaction IDs recorded', () => {
  assertAllIncluded(
    sources.tracker,
    [
      '| RG-808 | Done | Drill bridge route pause/resume |',
      'no remaining strict-order RG task',
      '21eb03b04b61a14f85f48fed47045841bcd2ce9bf7caa7a297564a35c447e819',
      '18e2b6e0d7b9759f1bba09a72dd0155d6e83469c684febb4fda252a5c9bbf314',
      'fd424726aac22ee05a81586c9710107775c788c50cce906437ed4b270ab548b6',
      'afe7d2a89d64084efda94a8899229321d910a174c601d5d65e9819681fa827c1',
      'restored custody-backed deposit creation on public, US, EU, and SEA RPC',
    ],
    'RESTRICTION_GOVERNANCE_TRACKER.md'
  );
});

test('drill doc preserves no privileged RPC, reset, or state-copy commands', () => {
  assertNotIncludes(sources.drill, 'LICHEN_ADMIN_TOKEN', 'BRIDGE_ROUTE_PAUSE_RESUME_DRILL.md');
  assertNotIncludes(sources.drill, 'rm -rf', 'BRIDGE_ROUTE_PAUSE_RESUME_DRILL.md');
  assertNotIncludes(sources.drill, 'rsync', 'BRIDGE_ROUTE_PAUSE_RESUME_DRILL.md');
});

console.log(`\nResult: ${passed} passed, ${failed} failed`);
if (failed > 0) process.exit(1);
