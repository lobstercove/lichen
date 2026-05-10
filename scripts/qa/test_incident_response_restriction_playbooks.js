#!/usr/bin/env node

const assert = require('assert');
const fs = require('fs');
const path = require('path');
const { requirePrivateDocs } = require('./private_docs_guard');

const repoRoot = path.join(__dirname, '..', '..');
const incidentDocPath = path.join(repoRoot, 'docs', 'deployment', 'INCIDENT_RESPONSE_MODE.md');
requirePrivateDocs('Restriction Incident Playbook QA', [incidentDocPath]);

const incidentDoc = fs.readFileSync(incidentDocPath, 'utf8');

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

function section(title) {
  const heading = `### ${title}\n`;
  const start = incidentDoc.indexOf(heading);
  assert.ok(start >= 0, `missing section: ${title}`);
  const bodyStart = start + heading.length;
  const nextHeading = /^##{1,2} /gm;
  nextHeading.lastIndex = bodyStart;
  const match = nextHeading.exec(incidentDoc);
  const bodyEnd = match ? match.index : incidentDoc.length;
  return incidentDoc.slice(bodyStart, bodyEnd);
}

console.log('\nRestriction Incident Playbook QA');

test('incident response doc separates manifest communication from consensus restrictions', () => {
  assert.ok(incidentDoc.includes('The incident manifest is the public communication and intake-gating layer.'), 'missing manifest vs restriction distinction');
  assert.ok(incidentDoc.includes('Restriction governance is the consensus-enforced containment layer.'), 'missing consensus containment statement');
  assert.ok(incidentDoc.includes('They are not final actions by themselves.'), 'missing unsigned builder warning');
});

test('incident response doc preserves shared-network safety rules', () => {
  assert.ok(incidentDoc.includes('Never reset a shared testnet'), 'missing no-reset rule');
  assert.ok(incidentDoc.includes("Never recover a validator by copying another validator's live state directory"), 'missing no-state-copy rule');
  assert.ok(incidentDoc.includes('72h maximum'), 'missing guardian TTL limit');
});

test('stolen funds wallet freeze playbook uses real account and account-asset builders', () => {
  const body = section('Stolen Funds Wallet Freeze');
  assert.ok(body.includes('lichen restriction build restrict-account'), 'missing restrict-account builder');
  assert.ok(body.includes('--mode outgoing-only'), 'missing outgoing-only guidance');
  assert.ok(body.includes('--reason stolen_funds'), 'missing stolen_funds reason');
  assert.ok(body.includes('set-frozen-asset-amount'), 'missing partial frozen amount builder');
  assert.ok(body.includes('lichen restriction can-transfer'), 'missing transfer verification');
});

test('scam contract quarantine playbook uses real contract builders and lifecycle checks', () => {
  const body = section('Scam Contract Quarantine');
  assert.ok(body.includes('lichen restriction status contract'), 'missing contract status check');
  assert.ok(body.includes('lichen restriction build quarantine-contract'), 'missing quarantine builder');
  assert.ok(body.includes('--reason scam_contract'), 'missing scam_contract reason');
  assert.ok(body.includes('terminate-contract'), 'missing termination caution');
});

test('bridge route pause playbook pairs manifest pause with consensus route restriction', () => {
  const body = section('Bridge Route Pause');
  assert.ok(body.includes('scripts/incident-status.js bridge-pause'), 'missing bridge manifest helper');
  assert.ok(body.includes('lichen restriction status bridge-route'), 'missing bridge-route status check');
  assert.ok(body.includes('lichen restriction build pause-bridge-route'), 'missing pause bridge-route builder');
  assert.ok(body.includes('--reason bridge_compromise'), 'missing bridge_compromise reason');
});

test('malicious code-hash ban playbook uses deploy-ban status and builder', () => {
  const body = section('Malicious Code-Hash Ban');
  assert.ok(body.includes('lichen restriction status code-hash'), 'missing code-hash status check');
  assert.ok(body.includes('lichen restriction build ban-code-hash'), 'missing ban-code-hash builder');
  assert.ok(body.includes('--reason malicious_code_hash'), 'missing malicious_code_hash reason');
  assert.ok(body.includes('deploy_blocked'), 'missing expected deploy_blocked outcome');
});

test('false-positive unfreeze playbook uses lift and target-specific recovery builders', () => {
  const body = section('False-Positive Unfreeze Or Resume');
  assert.ok(body.includes('lichen restriction build lift-restriction'), 'missing generic lift builder');
  assert.ok(body.includes('--lift-reason false_positive'), 'missing false_positive lift reason');
  assert.ok(body.includes('unrestrict-account'), 'missing account unrestrict builder');
  assert.ok(body.includes('resume-contract'), 'missing contract resume builder');
  assert.ok(body.includes('resume-bridge-route'), 'missing bridge route resume builder');
});

test('testnet drill playbook exercises create, verify, lift, and evidence retention', () => {
  const body = section('Testnet Drill');
  assert.ok(body.includes('--reason testnet_drill'), 'missing testnet_drill reason');
  assert.ok(body.includes('--lift-reason testnet_drill_complete'), 'missing testnet drill lift reason');
  assert.ok(body.includes('wallet and extension screenshots'), 'missing wallet/extension evidence requirement');
  assert.ok(body.includes('final `lichen restriction get <id>` output'), 'missing final restriction output requirement');
});

test('every non-drill playbook requires evidence hash placeholder', () => {
  for (const title of [
    'Stolen Funds Wallet Freeze',
    'Scam Contract Quarantine',
    'Bridge Route Pause',
    'Malicious Code-Hash Ban'
  ]) {
    const body = section(title);
    assert.ok(body.includes('EVIDENCE_HASH'), `${title} missing evidence hash variable`);
    assert.ok(body.includes('--evidence-hash "$EVIDENCE_HASH"'), `${title} missing evidence hash flag`);
  }
});

console.log(`\nRestriction Incident Playbook QA: ${passed} passed, ${failed} failed (${passed + failed} total)`);

if (failed > 0) {
  process.exit(1);
}
