#!/usr/bin/env node
'use strict';

const assert = require('assert');
const { validateManifest } = require('./check_neo_agent_compute_gate');

const HEX_A = 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa';
const HEX_B = 'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb';

function clone(value) {
    return JSON.parse(JSON.stringify(value));
}

function approval(name) {
    return {
        approved: true,
        approver: name,
        approved_at: '2026-05-17T00:00:00Z',
        evidence: `${name} approval hash`,
    };
}

function evidence(label) {
    return { passed: true, evidence: `${label} evidence hash` };
}

function baseManifest() {
    return {
        id: 'NX-980-neo-agent-compute',
        version: 1,
        network: 'testnet',
        scope: {
            agent_compute: true,
            neo_route_required: true,
            local_3_validator_required: true,
            no_state_copy_required: true,
            public_deployment_allowed: false,
            prediction_collateral: false,
        },
        approvals: {
            product: approval('product'),
            governance: approval('governance'),
            security: approval('security'),
            custody: approval('custody'),
            legal_compliance: approval('legal'),
            deployment: approval('deployment'),
        },
        assets: {
            payment_assets: ['wGAS', 'wNEO'],
            primary_fee_asset: 'wGAS',
            whole_lot_wneo_policy: 'wNEO remains whole-lot; agent policies cannot fractionalize it',
            no_unbacked_minting: true,
        },
        spending_policy: {
            policy_hash: HEX_A,
            policy_version: 1,
            max_per_agent_daily: '6000',
            max_per_task: '4000',
            opt_in_required: true,
            agent_disable_available: true,
            pq_action_hash_required: true,
            route_pause_blocks_new_payments: true,
            escrow_exit_unaffected_by_new_payment_pause: true,
        },
        pq_attestation: {
            evidence_kind: 'agent_action',
            purpose: 'neo-x-agent-compute',
            required_signatures: 2,
            stale_evidence_rejected: true,
            signature_mismatch_rejected: true,
            replay_domains: [
                'lichen_network',
                'neo_network',
                'neo_chain_id',
                'route',
                'asset',
                'purpose',
                'agent',
                'policy_hash',
            ],
        },
        compute_market: {
            contract_symbol: 'COMPUTE',
            required_functions: [
                'set_agent_compute_controls',
                'set_agent_spending_policy',
                'disable_agent_spending_policy',
                'submit_agent_job',
                'get_agent_compute_controls',
                'get_agent_spending_policy',
                'get_agent_spend_window',
                'get_agent_job_action',
            ],
            per_agent_spend_accounting: true,
            task_action_hash_accounting: true,
            normal_compute_flow_unchanged: true,
        },
        disclosure: {
            risk_disclosure_url: 'https://lichen.network/disclosures/neo-agent-compute-v1',
            risk_disclosure_hash: HEX_B,
        },
        evidence: {
            unit_contract_policy: evidence('contract policy'),
            pq_agent_action: evidence('pq agent action'),
            manifest_gate: evidence('manifest gate'),
            rpc_stats: evidence('rpc stats'),
            local_3_validator: evidence('local 3-validator'),
            route_pause_blocks_payment: evidence('route pause'),
            no_regression_existing_compute: evidence('existing compute regression'),
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
        `${label}: expected "${expectedSubstring}", got ${result.errors.join('; ')}`,
    );
    console.log(`PASS ${label}`);
}

expectPass('local Neo agent-compute manifest is accepted', baseManifest());

{
    const manifest = clone(baseManifest());
    manifest.scope.public_deployment_allowed = true;
    expectFail('public deployment stays separately gated', manifest, 'scope.public_deployment_allowed must be false');
}

{
    const manifest = clone(baseManifest());
    manifest.spending_policy.max_per_task = '7000';
    expectFail('per-task cap cannot exceed daily cap', manifest, 'max_per_task must not exceed');
}

{
    const manifest = clone(baseManifest());
    manifest.spending_policy.route_pause_blocks_new_payments = false;
    expectFail('route pause must block new agent payments', manifest, 'route_pause_blocks_new_payments');
}

{
    const manifest = clone(baseManifest());
    manifest.pq_attestation.evidence_kind = 'route_health';
    expectFail('agent action PQ evidence kind is required', manifest, 'evidence_kind must be agent_action');
}

{
    const manifest = clone(baseManifest());
    manifest.compute_market.required_functions = manifest.compute_market.required_functions.filter((name) => name !== 'submit_agent_job');
    expectFail('runtime submit_agent_job function is required', manifest, 'submit_agent_job');
}

{
    const manifest = clone(baseManifest());
    manifest.evidence.local_3_validator.passed = false;
    expectFail('local 3-validator evidence is required', manifest, 'evidence.local_3_validator.passed must be true');
}

console.log('Neo agent-compute gate tests passed');
