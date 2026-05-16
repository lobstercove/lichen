#!/usr/bin/env node
'use strict';

const assert = require('assert');
const { validateManifest } = require('./check_neo_zk_proof_services_gate');

const HEX_A = 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa';
const HEX_B = 'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb';

function clone(value) {
    return JSON.parse(JSON.stringify(value));
}

function baseManifest() {
    return {
        id: 'NX-960-neo-zk-proof-services',
        version: 1,
        network: 'testnet',
        scope: {
            zk_stark_services: true,
            neo_route_required: true,
            reserve_liability_only: true,
            prediction_collateral: false,
            agent_compute: false,
        },
        approvals: {
            product: { approved: true, approver: 'product', approved_at: '2026-05-16T00:00:00Z', evidence: 'signed product approval hash' },
            governance: { approved: true, approver: 'governance', approved_at: '2026-05-16T00:00:00Z', evidence: 'governance approval hash' },
            security: { approved: true, approver: 'security', approved_at: '2026-05-16T00:00:00Z', evidence: 'security review hash' },
            custody: { approved: true, approver: 'custody', approved_at: '2026-05-16T00:00:00Z', evidence: 'custody reserve source approval hash' },
            legal_compliance: { approved: true, approver: 'legal', approved_at: '2026-05-16T00:00:00Z', evidence: 'legal disclosure approval hash' },
            privacy: { approved: true, approver: 'privacy', approved_at: '2026-05-16T00:00:00Z', evidence: 'privacy leakage review hash' },
            deployment: { approved: true, approver: 'deployment', approved_at: '2026-05-16T00:00:00Z', evidence: 'deployment approval hash' },
        },
        assets: ['wNEO', 'wGAS', 'NEOGASRWD'],
        proof_statement: {
            proof_type: 'reserve_liability',
            zk_scheme: 'plonky3-fri-poseidon2',
            privacy_model: 'transparent_aggregate_totals_no_address_list_v1',
            verifier_version: 1,
            public_inputs: [
                'domain_hash',
                'statement_hash',
                'witness_commitment',
                'reserve_amount',
                'liability_amount',
                'epoch',
                'verifier_version',
            ],
            domain_fields: [
                'lichen_network',
                'neo_network',
                'neo_chain_id',
                'route',
                'asset',
                'product',
                'verifier_version',
            ],
            reserve_amount_public: true,
            liability_amount_public: true,
            no_address_list: true,
            undercollateralized_statements_rejected: true,
            domain_separator_hash: HEX_A,
            statement_schema_hash: HEX_B,
        },
        claims: {
            direct_neox_onchain_verification: false,
            public_disclosure_url: 'https://lichen.network/disclosures/neo-zk-proof-services-v1',
            no_hidden_witness_claim: 'v1 discloses transparent aggregate totals and no address list',
        },
        benchmarks: {
            max_proof_ms: 5000,
            max_verify_ms: 1000,
            evidence: 'local benchmark summary hash',
        },
        activation: {
            mode: 'local_rehearsal',
            rollback_runbook: 'docs/deployment/PRODUCTION_DEPLOYMENT.md#neo-zk-proof-services-gate',
            deployment_scope: 'local rehearsal only',
        },
        evidence: {
            local_3_validator: { passed: true, evidence: 'local 3-validator proof verification passed' },
            cli_proof_generation: { passed: true, evidence: 'zk-prove reserve-liability generated proof' },
            native_verification: { passed: true, evidence: 'core verifier accepted proof' },
            rpc_verification: { passed: true, evidence: 'verifyNeoReserveLiabilityProof accepted proof' },
            sdk_consumer: { passed: true, evidence: 'SDK consumer example verified proof' },
            public_input_binding: { passed: true, evidence: 'mutated domain public input rejected' },
            replay_rejection: { passed: true, evidence: 'neox/gas proof rejected for neox/neo domain replay' },
            privacy_leakage_review: { passed: true, evidence: 'no address list or user-level liabilities disclosed' },
            watchtower: { passed: true, evidence: 'watchtower status includes proof-service lane' },
        },
    };
}

function expectPass(label, manifest) {
    const result = validateManifest(manifest);
    assert.strictEqual(result.ok, true, `${label}: ${result.errors.join('; ')}`);
    console.log(`PASS ${label}`);
}

function expectFail(label, manifest, expectedSubstring) {
    const result = validateManifest(manifest);
    assert.strictEqual(result.ok, false, `${label}: expected failure`);
    assert(
        result.errors.some((error) => error.includes(expectedSubstring)),
        `${label}: expected error containing "${expectedSubstring}", got ${result.errors.join('; ')}`
    );
    console.log(`PASS ${label}`);
}

expectPass('local Neo ZK proof services manifest is accepted', baseManifest());

{
    const manifest = clone(baseManifest());
    manifest.claims.direct_neox_onchain_verification = true;
    expectFail('direct Neo X on-chain verification claim is blocked in v1', manifest, 'direct_neox_onchain_verification');
}

{
    const manifest = clone(baseManifest());
    manifest.proof_statement.privacy_model = 'hidden_totals_v1';
    expectFail('privacy model must disclose transparent aggregate totals', manifest, 'transparent_aggregate_totals_no_address_list_v1');
}

{
    const manifest = clone(baseManifest());
    manifest.proof_statement.public_inputs = manifest.proof_statement.public_inputs.filter((value) => value !== 'liability_amount');
    expectFail('liability amount must be a public input', manifest, 'liability_amount');
}

{
    const manifest = clone(baseManifest());
    manifest.scope.agent_compute = true;
    expectFail('future lanes stay blocked by NX-960 manifest', manifest, 'scope.agent_compute must remain false');
}

{
    const manifest = clone(baseManifest());
    manifest.evidence.replay_rejection.passed = false;
    expectFail('replay rejection evidence is required', manifest, 'evidence.replay_rejection.passed must be true');
}

console.log('Neo ZK proof services gate tests passed');
