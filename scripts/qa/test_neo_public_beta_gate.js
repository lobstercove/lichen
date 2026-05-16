#!/usr/bin/env node
'use strict';

const assert = require('assert');
const { validateManifest } = require('./check_neo_public_beta_gate');

const HEX_A = 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa';
const HEX_B = 'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb';
const PUB_A = '8NWMyb2m6bvp6ZByNVCh3evqjtwHWKCVFSU8mycN3ZC';
const PUB_B = '7DAS23aV2Bs95MRQnE6AZvHQJBv2KCbr6n2n18WHP9uY';

function clone(value) {
    return JSON.parse(JSON.stringify(value));
}

function baseManifest() {
    return {
        id: 'NX-900-neo-public-beta',
        version: 1,
        network: 'testnet',
        scope: {
            neo_route: true,
            neo_gas_rewards: true,
            liquidity_corridor: false,
            zk_stark_services: false,
            prediction_collateral: false,
            agent_compute: false,
        },
        approvals: {
            owner: { approved: true, approver: 'owner', approved_at: '2026-05-16T00:00:00Z', evidence: 'signed owner approval hash' },
            governance: { approved: true, approver: 'community_treasury', approved_at: '2026-05-16T00:00:00Z', evidence: 'proposal id or signed governance approval' },
            security: { approved: true, approver: 'security', approved_at: '2026-05-16T00:00:00Z', evidence: 'security checklist hash' },
            custody: { approved: true, approver: 'custody', approved_at: '2026-05-16T00:00:00Z', evidence: 'custody funding approval hash' },
            legal_compliance: { approved: true, approver: 'legal', approved_at: '2026-05-16T00:00:00Z', evidence: 'disclosure approval hash' },
            deployment: { approved: true, approver: 'deployment', approved_at: '2026-05-16T00:00:00Z', evidence: 'deployment signoff hash' },
        },
        caps: {
            wneo_route_cap: '1000000000000',
            wneo_per_user_cap: '100000000000',
            wgas_route_cap: '1000000000000',
            wgas_per_user_cap: '100000000000',
            rewards_route_cap: '1000000000000',
            rewards_per_user_cap: '100000000000',
            rewards_daily_import_cap: '1000000000',
            rewards_daily_claim_cap: '1000000000',
            rewards_campaign_budget: '10000000000',
        },
        funding: {
            reward_source_policy: 'signed Neo rewards source policy v1',
            wgas_reward_budget: '10000000000',
            wgas_reward_funding_tx_or_commitment: 'funding tx or reserve commitment hash',
            reserve_attestation_hash: HEX_A,
            reward_policy_version: 1,
            reward_policy_hash: HEX_B,
        },
        disclosure: {
            version: 1,
            hash: HEX_A,
            public_url: 'https://lichen.network/disclosures/neo-gas-rewards-v1',
            copy_reviewed: true,
        },
        activation: {
            mode: 'fresh_genesis',
            fresh_genesis_env: {
                LICHEN_GENESIS_NEO_GAS_REWARDS_ENABLE: '1',
                LICHEN_GENESIS_NEO_GAS_REWARDS_ROUTE_CAP: '1000000000000',
                LICHEN_GENESIS_NEO_GAS_REWARDS_PER_USER_CAP: '100000000000',
                LICHEN_GENESIS_NEO_GAS_REWARDS_DAILY_IMPORT_CAP: '1000000000',
                LICHEN_GENESIS_NEO_GAS_REWARDS_DAILY_CLAIM_CAP: '1000000000',
                LICHEN_GENESIS_NEO_GAS_REWARDS_CAMPAIGN_BUDGET: '10000000000',
                LICHEN_GENESIS_NEO_GAS_REWARDS_DISCLOSURE_VERSION: '1',
                LICHEN_GENESIS_NEO_GAS_REWARDS_DISCLOSURE_HASH: HEX_A,
                LICHEN_GENESIS_NEO_GAS_REWARDS_POLICY_VERSION: '1',
                LICHEN_GENESIS_NEO_GAS_REWARDS_POLICY_HASH: HEX_B,
                LICHEN_GENESIS_NEO_GAS_REWARDS_IMPORTER_PUBKEY: PUB_A,
            },
        },
        operations: {
            reward_importer_pubkey: PUB_A,
            incident_guardian_pubkey: PUB_B,
            watchtower_enabled: true,
            rollback_runbook: 'docs/deployment/PRODUCTION_DEPLOYMENT.md#neo-public-beta-gate',
            monitoring_thresholds: 'watchtower neo gas rewards thresholds configured',
        },
        evidence: {
            local_3_validator: { passed: true, evidence: 'PASS=42 FAIL=0 fresh local cluster' },
            reused_chain_e2e: { passed: true, evidence: 'PASS=42 FAIL=0 reused local chain' },
            no_trapped_funds: { passed: true, evidence: 'exit returned exact principal' },
            wallet_audit: { passed: true, evidence: 'wallet audit passed' },
            extension_audit: { passed: true, evidence: 'extension audit passed' },
            dex_oracle_candle_smoke: { passed: true, evidence: 'candles and oracle smoke passed' },
            watchtower: { passed: true, evidence: 'watchtower neo rewards alerts passed' },
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

expectPass('fresh genesis public beta manifest is accepted', baseManifest());

{
    const manifest = clone(baseManifest());
    manifest.approvals.legal_compliance.approved = false;
    expectFail('legal/compliance approval is required', manifest, 'approvals.legal_compliance.approved must be true');
}

{
    const manifest = clone(baseManifest());
    manifest.caps.rewards_per_user_cap = '2000000000000';
    expectFail('rewards per-user cap cannot exceed route cap', manifest, 'caps.rewards_per_user_cap must not exceed caps.rewards_route_cap');
}

{
    const manifest = clone(baseManifest());
    manifest.activation.fresh_genesis_env.LICHEN_GENESIS_NEO_GAS_REWARDS_CAMPAIGN_BUDGET = '1';
    expectFail('fresh genesis env must match approved caps', manifest, 'LICHEN_GENESIS_NEO_GAS_REWARDS_CAMPAIGN_BUDGET');
}

{
    const manifest = clone(baseManifest());
    manifest.scope.liquidity_corridor = true;
    expectFail('future lanes stay blocked by NX-900 manifest', manifest, 'scope.liquidity_corridor must remain false');
}

{
    const manifest = clone(baseManifest());
    manifest.activation = {
        mode: 'post_genesis_governance',
        governance_proposal: {
            proposal_id: 'neo-public-beta-testnet-proposal-1',
            payload_hash: HEX_B,
            timelock_seconds: 86400,
        },
    };
    expectPass('post-genesis governance activation manifest is accepted', manifest);
}

console.log('Neo public beta gate tests passed');
