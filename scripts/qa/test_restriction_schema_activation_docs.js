#!/usr/bin/env node
'use strict';

const assert = require('assert');
const fs = require('fs');
const path = require('path');
const { requirePrivateDocs } = require('./private_docs_guard');

const ROOT = path.join(__dirname, '..', '..');
const RUNBOOK = path.join(ROOT, 'docs', 'deployment', 'RESTRICTION_SCHEMA_ACTIVATION.md');
const PRODUCTION_RUNBOOK = path.join(ROOT, 'docs', 'deployment', 'PRODUCTION_DEPLOYMENT.md');
const SCRIPT = path.join(ROOT, 'scripts', 'activate-restriction-schema-testnet.sh');
const VALIDATOR = path.join(ROOT, 'validator', 'src', 'main.rs');
const PACKAGE = path.join(ROOT, 'package.json');

requirePrivateDocs('Restriction Schema Activation Docs QA', [RUNBOOK, PRODUCTION_RUNBOOK]);

const sources = {
  runbook: fs.readFileSync(RUNBOOK, 'utf8'),
  productionRunbook: fs.readFileSync(PRODUCTION_RUNBOOK, 'utf8'),
  script: fs.readFileSync(SCRIPT, 'utf8'),
  validator: fs.readFileSync(VALIDATOR, 'utf8'),
  packageJson: JSON.parse(fs.readFileSync(PACKAGE, 'utf8')),
};

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

function assertNotIncludes(source, needle, label) {
  assert.ok(!source.includes(needle), `${label} must not include '${needle}'`);
}

function assertAllIncluded(source, needles, label) {
  needles.forEach((needle) => assertIncludes(source, needle, label));
}

console.log('\nRestriction Schema Activation Docs QA');

test('runbook documents testnet-only owner-approved in-place activation', () => {
  assertAllIncluded(
    sources.runbook,
    [
      'RG-804 testnet-only activation',
      'not a reset',
      'Mainnet activation is not covered by RG-804',
      'LICHEN_OWNER_APPROVED_RESTRICTION_SCHEMA_ACTIVATION',
      'LICHEN_RESTRICTION_SCHEMA_ACTIVATION_CONFIRM',
      'scripts/activate-restriction-schema-testnet.sh',
      'Stop all testnet validators before setting the schema',
    ],
    'RESTRICTION_SCHEMA_ACTIVATION.md'
  );
});

test('runbook documents exact schema commands and evidence requirements', () => {
  assertAllIncluded(
    sources.runbook,
    [
      '--show-restriction-schema',
      '--activate-restriction-schema',
      'after_schema=active',
      'has_genesis_block=true',
      'sync-evidence-summary.json',
      'validator-hash.txt',
      'synced_within_threshold=true',
      'post-latest-block-<host>.json',
      'post-journal-scan-<host>.txt',
      'cargo test -p lichen-validator restriction_schema --bin lichen-validator -- --nocapture',
      'bash -n scripts/activate-restriction-schema-testnet.sh',
    ],
    'RESTRICTION_SCHEMA_ACTIVATION.md'
  );
});

test('activation script has approval gates and no reset/state-copy operations', () => {
  assertAllIncluded(
    sources.script,
    [
      'LICHEN_OWNER_APPROVED_RESTRICTION_SCHEMA_ACTIVATION',
      'LICHEN_RESTRICTION_SCHEMA_ACTIVATION_CONFIRM',
      'owner-approved:restriction-schema',
      'activate-restriction-schema',
      'sudo systemctl stop ${SERVICE}',
      'sudo systemctl start ${SERVICE}',
      '--show-restriction-schema',
      '--activate-restriction-schema',
      'verify_matching_validator_hashes',
      'LICHEN_ACTIVATION_MAX_SLOT_SPREAD',
      'synced_within_threshold',
      'sync-evidence-summary.json',
    ],
    'activate-restriction-schema-testnet.sh'
  );
  assertNotIncludes(sources.script, 'rm -rf', 'activate-restriction-schema-testnet.sh');
  assertNotIncludes(sources.script, 'clean-slate-redeploy.sh', 'activate-restriction-schema-testnet.sh');
  assertNotIncludes(sources.script, 'rsync', 'activate-restriction-schema-testnet.sh');
});

test('validator one-shot command is testnet-only and requires stored genesis', () => {
  assertAllIncluded(
    sources.validator,
    [
      'ACTIVATE_RESTRICTION_SCHEMA_FLAG',
      'SHOW_RESTRICTION_SCHEMA_FLAG',
      'RG-804 allows testnet only',
      'get_block_by_slot(0)',
      'set_state_root_schema(true)',
      'print_restriction_schema_report',
      'rg804_restriction_schema_activation_is_testnet_only_and_requires_genesis',
      'rg804_restriction_schema_activation_sets_prefixed_root_without_state_copy',
    ],
    'validator/src/main.rs'
  );
});

test('production deployment runbook links the activation path', () => {
  assertAllIncluded(
    sources.productionRunbook,
    [
      'RESTRICTION_SCHEMA_ACTIVATION.md',
      'scripts/activate-restriction-schema-testnet.sh',
      'Testnet restriction schema activation',
      'It is not a reset path and must not copy chain state',
    ],
    'PRODUCTION_DEPLOYMENT.md'
  );
});

test('package deployment-doc QA includes activation audit', () => {
  assertIncludes(
    sources.packageJson.scripts['test-restriction-schema-activation-docs'],
    'test_restriction_schema_activation_docs.js',
    'package.json'
  );
  assertIncludes(
    sources.packageJson.scripts['test-deployment-docs'],
    'test_restriction_schema_activation_docs.js',
    'package.json'
  );
});

console.log(`\nRestriction Schema Activation Docs QA: ${passed} passed, ${failed} failed (${passed + failed} total)`);

if (failed > 0) {
  process.exit(1);
}
