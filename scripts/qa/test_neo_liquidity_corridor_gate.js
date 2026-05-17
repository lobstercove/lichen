#!/usr/bin/env node
'use strict';

const assert = require('assert');
const { validateManifest } = require('./check_neo_liquidity_corridor_gate');

const HEX_A = 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa';
const HEX_B = 'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb';
const HEX_C = 'cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc';

function clone(value) {
    return JSON.parse(JSON.stringify(value));
}

function basePair(symbol, pairId, poolId, overrides = {}) {
    const isWneo = symbol.startsWith('wNEO/');
    return {
        symbol,
        pair_id: pairId,
        pool_id: poolId,
        enabled: true,
        whole_lot_aware: isWneo,
        tvl_cap: '1000000000000',
        volume_cap: '5000000000000',
        reward_budget: '10000000000',
        reward_rate_per_slot: '1000',
        ...overrides,
    };
}

function baseManifest() {
    return {
        id: 'NX-950-neo-liquidity-corridor',
        version: 1,
        network: 'testnet',
        scope: {
            liquidity_corridor: true,
            neo_route_required: true,
            uses_existing_dex_rewards: true,
            zk_stark_services: false,
            prediction_collateral: false,
            agent_compute: false,
        },
        approvals: {
            product: { approved: true, approver: 'product', approved_at: '2026-05-16T00:00:00Z', evidence: 'signed product lane approval hash' },
            governance: { approved: true, approver: 'governance', approved_at: '2026-05-16T00:00:00Z', evidence: 'proposal or signed governance approval' },
            security: { approved: true, approver: 'security', approved_at: '2026-05-16T00:00:00Z', evidence: 'security review hash' },
            custody: { approved: true, approver: 'custody', approved_at: '2026-05-16T00:00:00Z', evidence: 'funding and reserve approval hash' },
            legal_compliance: { approved: true, approver: 'legal', approved_at: '2026-05-16T00:00:00Z', evidence: 'disclosure approval hash' },
            market_risk: { approved: true, approver: 'risk', approved_at: '2026-05-16T00:00:00Z', evidence: 'market risk cap signoff hash' },
            deployment: { approved: true, approver: 'deployment', approved_at: '2026-05-16T00:00:00Z', evidence: 'deployment signoff hash' },
        },
        pairs: [
            basePair('wNEO/lUSD', 8, 8),
            basePair('wNEO/LICN', 9, 9),
            basePair('wGAS/lUSD', 10, 10),
            basePair('wGAS/LICN', 11, 11),
        ],
        caps: {
            campaign_budget: '50000000000',
            daily_reward_cap: '5000000000',
            per_wallet_reward_cap: '1000000000',
            max_total_tvl: '5000000000000',
            max_total_volume: '25000000000000',
            loss_limit: '10000000000',
        },
        funding: {
            reward_asset: 'LICN',
            reward_source_policy: 'signed liquidity corridor reward source policy v1',
            funding_tx_or_commitment: 'funding transaction or treasury commitment hash',
            funding_amount: '50000000000',
            treasury_attestation_hash: HEX_A,
            campaign_policy_hash: HEX_B,
        },
        accounting: {
            uses_dex_rewards_self_custody: true,
            rewards_cannot_exceed_campaign_budget: true,
            reward_rate_payload_matches_pairs: true,
            budget_exhaustion_behavior: 'set Neo pair reward rates to zero after campaign budget exhaustion',
            rounding_policy: 'integer base-unit rewards; dust remains in reward treasury',
            audit_outputs: 'signed rate payload, rewards stats, pool caps, and budget exhaustion report',
        },
        risk_controls: {
            route_pause_blocks_new_campaign_activity: true,
            user_exits_remain_available_when_campaign_paused: true,
            whole_lot_wneo_preserved: true,
            no_unbacked_wrapped_assets: true,
            rollback_steps: 'pause campaign rates, keep withdrawals/exits, publish status, verify no trapped funds',
        },
        disclosure: {
            version: '1',
            hash: HEX_C,
            public_url: 'https://lichen.network/disclosures/neo-liquidity-corridor-v1',
            copy_reviewed: true,
        },
        activation: {
            mode: 'post_genesis_governance',
            governance_proposal: {
                proposal_id: 'neo-liquidity-corridor-testnet-proposal-1',
                payload_hash: HEX_B,
                timelock_seconds: '86400',
                rate_change_summary: 'dex_rewards.configure_lp_campaign for pair IDs 8, 9, 10, and 11 according to manifest',
            },
        },
        operations: {
            watchtower_enabled: true,
            rollback_runbook: 'docs/deployment/PRODUCTION_DEPLOYMENT.md#neo-liquidity-corridor-gate',
            monitoring_thresholds: 'Neo liquidity corridor TVL, volume, reward budget, route pause, candle freshness',
            public_status_copy: 'public route status and campaign disclosure copy reviewed',
        },
        evidence: {
            local_3_validator: { passed: true, evidence: 'local 3-validator no-state-copy sync passed' },
            dex_e2e: { passed: true, evidence: 'DEX E2E passed with Neo pairs' },
            amm_router_candles: { passed: true, evidence: 'AMM, router, candles, and WS checks passed' },
            reward_budget_exhaustion: { passed: true, evidence: 'budget exhaustion test kept rewards within cap' },
            route_pause_no_new_activity: { passed: true, evidence: 'route pause blocked new campaign activity' },
            no_trapped_funds: { passed: true, evidence: 'LP remove and bridge/user exits remained available' },
            wallet_dex_disclosure: { passed: true, evidence: 'wallet and DEX disclosure surfaces passed audit' },
            watchtower: { passed: true, evidence: 'watchtower alerts passed' },
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

expectPass('complete Neo liquidity corridor manifest is accepted', baseManifest());

{
    const manifest = clone(baseManifest());
    manifest.approvals.market_risk.approved = false;
    expectFail('market-risk approval is required', manifest, 'approvals.market_risk.approved must be true');
}

{
    const manifest = clone(baseManifest());
    manifest.pairs[0].pair_id = 12;
    expectFail('Neo pair IDs must match genesis DEX IDs', manifest, 'pairs.0.pair_id must equal 8');
}

{
    const manifest = clone(baseManifest());
    manifest.pairs[0].whole_lot_aware = false;
    expectFail('wNEO pairs must remain whole-lot aware', manifest, 'pairs.0.whole_lot_aware must be true');
}

{
    const manifest = clone(baseManifest());
    manifest.pairs[3].enabled = false;
    manifest.pairs[3].reward_budget = '1';
    expectFail('disabled pairs cannot carry non-zero caps or rewards', manifest, 'disabled pairs must have zero');
}

{
    const manifest = clone(baseManifest());
    manifest.caps.campaign_budget = '100';
    expectFail('campaign budget must cover enabled pair budgets', manifest, 'sum(pairs.reward_budget for enabled pairs) must not exceed caps.campaign_budget');
}

{
    const manifest = clone(baseManifest());
    manifest.funding.reward_asset = 'wGAS';
    expectFail('current DEX rewards payout asset must stay LICN', manifest, 'funding.reward_asset must be LICN');
}

{
    const manifest = clone(baseManifest());
    manifest.activation.mode = 'fresh_genesis';
    expectFail('campaign activation uses governed post-genesis payload', manifest, 'activation.mode must be post_genesis_governance');
}

{
    const manifest = clone(baseManifest());
    manifest.scope.zk_stark_services = true;
    expectFail('future lanes stay blocked by NX-950 manifest', manifest, 'scope.zk_stark_services must remain false');
}

console.log('Neo liquidity corridor gate tests passed');
