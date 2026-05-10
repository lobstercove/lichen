#!/usr/bin/env node
'use strict';

const assert = require('assert');
const fs = require('fs');
const path = require('path');
const { requirePrivateDocs } = require('./private_docs_guard');

const repoRoot = path.join(__dirname, '..', '..');
const keyHierarchyDocPath = path.join(repoRoot, 'docs', 'deployment', 'KEY_HIERARCHY_TEMPLATE.md');
requirePrivateDocs('Key Hierarchy Docs QA', [keyHierarchyDocPath]);

const keyHierarchyDoc = fs.readFileSync(keyHierarchyDocPath, 'utf8');
const multisigSource = fs.readFileSync(path.join(repoRoot, 'core', 'src', 'multisig.rs'), 'utf8');
const authoritiesSource = fs.readFileSync(
  path.join(repoRoot, 'core', 'src', 'processor', 'governance_authorities.rs'),
  'utf8'
);
const policiesSource = fs.readFileSync(
  path.join(repoRoot, 'core', 'src', 'processor', 'governance_policies.rs'),
  'utf8'
);
const restrictionsSource = fs.readFileSync(path.join(repoRoot, 'core', 'src', 'restrictions.rs'), 'utf8');

let passed = 0;
let failed = 0;

function test(name, fn) {
  try {
    fn();
    passed++;
    console.log(`  PASS ${name}`);
  } catch (error) {
    failed++;
    console.log(`  FAIL ${name}: ${error.message}`);
  }
}

function assertIncludes(source, needle, label) {
  assert.ok(source.includes(needle), `${label} missing '${needle}'`);
}

function assertAllIncluded(source, needles, label) {
  needles.forEach((needle) => assertIncludes(source, needle, label));
}

function governedLabelsFromSource() {
  return Array.from(
    multisigSource.matchAll(/pub const [A-Z_]+_LABEL: &str = "([^"]+)";/g),
    (match) => match[1]
  );
}

console.log('\nKey Hierarchy Docs QA');

test('key hierarchy doc covers every governed split-role authority label in source', () => {
  const labels = governedLabelsFromSource();
  assertAllIncluded(
    multisigSource,
    [
      'derive_incident_guardian_authority',
      'derive_bridge_committee_admin_authority',
      'derive_oracle_committee_admin_authority',
      'derive_upgrade_proposer_authority',
      'derive_upgrade_veto_guardian_authority',
      'derive_treasury_executor_authority',
    ],
    'core/src/multisig.rs'
  );
  assertAllIncluded(
    labels,
    [
      'incident_guardian',
      'bridge_committee_admin',
      'oracle_committee_admin',
      'upgrade_proposer',
      'upgrade_veto_guardian',
      'treasury_executor',
    ],
    'governed source labels'
  );
  labels.forEach((label) => {
    assertIncludes(keyHierarchyDoc, `\`${label}\``, 'KEY_HIERARCHY_TEMPLATE.md');
  });
});

test('key hierarchy doc documents main governance and stored restriction authority routing', () => {
  assertAllIncluded(
    authoritiesSource,
    [
      'resolve_restriction_governance_proposal_authority',
      'restriction_create_split_authority',
      'resolve_stored_restriction_approval_authority',
      'approval_authority',
    ],
    'core/src/processor/governance_authorities.rs'
  );
  assertAllIncluded(
    keyHierarchyDoc,
    [
      '`main_governance`',
      '`stored_restriction_authority`',
      '`approval_authority`',
      'main governance remains higher authority',
      'main governance can lift or extend records',
      'not a standalone key',
    ],
    'KEY_HIERARCHY_TEMPLATE.md'
  );
});

test('key hierarchy doc documents bridge and oracle split-role restriction scope', () => {
  assertAllIncluded(
    keyHierarchyDoc,
    [
      '`BridgeRoute`',
      '`ProtocolModule(Bridge)`',
      '`ProtocolModule(Oracle)`',
      'bridge committee membership keys are separate roots',
      'oracle feeder/attester keys are separate roots',
    ],
    'KEY_HIERARCHY_TEMPLATE.md'
  );
});

test('key hierarchy doc documents incident guardian TTL and target/mode policy', () => {
  assertIncludes(restrictionsSource, 'pub const GUARDIAN_RESTRICTION_MAX_SLOTS: u64 = 648_000;', 'core/src/restrictions.rs');
  assertAllIncluded(
    policiesSource,
    [
      'guardian_restrict_target_mode_allowed',
      'RestrictionMode::OutgoingOnly',
      'RestrictionMode::StateChangingBlocked',
      'RestrictionMode::Quarantined',
      'RestrictionMode::DeployBlocked',
      'RestrictionMode::RoutePaused',
      'RestrictionMode::ProtocolPaused',
      'validate_guardian_expiry',
    ],
    'core/src/processor/governance_policies.rs'
  );
  assertAllIncluded(
    keyHierarchyDoc,
    [
      '`GUARDIAN_RESTRICTION_MAX_SLOTS`',
      '`648,000`',
      '`expires_at_slot`',
      'one guardian extension',
      '`OutgoingOnly`',
      '`StateChangingBlocked`',
      '`Quarantined`',
      '`DeployBlocked`',
      '`RoutePaused`',
      '`ProtocolPaused`',
    ],
    'KEY_HIERARCHY_TEMPLATE.md'
  );
});

test('key hierarchy doc documents prohibited guardian and raw-admin paths', () => {
  assertAllIncluded(
    keyHierarchyDoc,
    [
      '`FrozenAmount`',
      '`IncomingOnly`',
      '`Bidirectional`',
      '`Terminated`',
      '`LICHEN_ADMIN_TOKEN`',
      'not a production governance or restriction mutation key',
      'must not bypass wallet signing',
      'must not bypass wallet signing, governed wallet thresholds, proposal lifecycle checks',
      'direct database edits',
    ],
    'KEY_HIERARCHY_TEMPLATE.md'
  );
});

test('key hierarchy doc preserves launch inventory and no-state-copy controls', () => {
  assertAllIncluded(
    keyHierarchyDoc,
    [
      'Before genesis or a clean deployment',
      'threshold policy',
      'signer public keys',
      'last verification date',
      'Do not recover a validator by copying another validator',
      'Run `npm run test-key-hierarchy-docs`',
      'Run `npm run test-key-hierarchy-docs` and `npm run test-deployment-docs`',
    ],
    'KEY_HIERARCHY_TEMPLATE.md'
  );
});

console.log(`\nKey Hierarchy Docs QA: ${passed} passed, ${failed} failed (${passed + failed} total)`);

if (failed > 0) {
  process.exit(1);
}
