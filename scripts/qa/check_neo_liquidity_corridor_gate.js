#!/usr/bin/env node
'use strict';

const fs = require('fs');
const path = require('path');

const HEX32 = /^[0-9a-fA-F]{64}$/;
const HTTP_URL = /^https:\/\/[^ \t\r\n]+$/i;

const EXPECTED_PAIRS = new Map([
    ['wNEO/lUSD', { pair_id: 8, pool_id: 8, whole_lot_required: true }],
    ['wNEO/LICN', { pair_id: 9, pool_id: 9, whole_lot_required: true }],
    ['wGAS/lUSD', { pair_id: 10, pool_id: 10, whole_lot_required: false }],
    ['wGAS/LICN', { pair_id: 11, pool_id: 11, whole_lot_required: false }],
]);

function usage() {
    return [
        'Usage: node scripts/qa/check_neo_liquidity_corridor_gate.js --manifest <path>',
        '',
        'Validates the NX-950 Neo Liquidity Corridor manifest before any',
        'wNEO/wGAS DEX incentive campaign is activated. The checker is',
        'fail-closed: missing approvals, caps, funding, governance payload,',
        'rollback controls, disclosure, or DEX evidence are release blockers.',
    ].join('\n');
}

function readPath(object, dottedPath) {
    return dottedPath.split('.').reduce((value, key) => {
        if (value && typeof value === 'object' && Object.prototype.hasOwnProperty.call(value, key)) {
            return value[key];
        }
        return undefined;
    }, object);
}

function asIntegerString(value) {
    if (typeof value === 'number' && Number.isSafeInteger(value) && value >= 0) {
        return String(value);
    }
    if (typeof value === 'bigint' && value >= 0n) {
        return String(value);
    }
    if (typeof value === 'string' && /^[0-9]+$/.test(value.trim())) {
        return value.trim();
    }
    return null;
}

function bigintAt(manifest, dottedPath, errors, { positive = true } = {}) {
    const raw = readPath(manifest, dottedPath);
    const normalized = asIntegerString(raw);
    if (normalized === null) {
        errors.push(`${dottedPath} must be an integer string or safe integer`);
        return 0n;
    }
    const value = BigInt(normalized);
    if (positive && value <= 0n) {
        errors.push(`${dottedPath} must be greater than zero`);
    }
    return value;
}

function requireString(manifest, dottedPath, errors, { pattern, description } = {}) {
    const value = readPath(manifest, dottedPath);
    if (typeof value !== 'string' || value.trim() === '') {
        errors.push(`${dottedPath} must be a non-empty string`);
        return '';
    }
    const trimmed = value.trim();
    if (pattern && !pattern.test(trimmed)) {
        errors.push(`${dottedPath} must be ${description || `match ${pattern}`}`);
    }
    return trimmed;
}

function requireBooleanTrue(manifest, dottedPath, errors) {
    if (readPath(manifest, dottedPath) !== true) {
        errors.push(`${dottedPath} must be true`);
    }
}

function requireApproval(manifest, name, errors) {
    const base = `approvals.${name}`;
    requireBooleanTrue(manifest, `${base}.approved`, errors);
    requireString(manifest, `${base}.approver`, errors);
    requireString(manifest, `${base}.approved_at`, errors);
    requireString(manifest, `${base}.evidence`, errors);
}

function requireEvidence(manifest, dottedPath, errors) {
    requireBooleanTrue(manifest, `${dottedPath}.passed`, errors);
    requireString(manifest, `${dottedPath}.evidence`, errors);
}

function requireHash(manifest, dottedPath, errors) {
    requireString(manifest, dottedPath, errors, {
        pattern: HEX32,
        description: 'a 32-byte hex hash',
    });
}

function validatePairs(manifest, errors) {
    const pairs = readPath(manifest, 'pairs');
    if (!Array.isArray(pairs) || pairs.length === 0) {
        errors.push('pairs must be a non-empty array');
        return {
            enabledCount: 0,
            totalRewardBudget: 0n,
            totalTvlCap: 0n,
            totalVolumeCap: 0n,
            seenPairs: new Set(),
        };
    }

    let enabledCount = 0;
    let totalRewardBudget = 0n;
    let totalTvlCap = 0n;
    let totalVolumeCap = 0n;
    const seenPairs = new Set();

    pairs.forEach((pair, index) => {
        const base = `pairs.${index}`;
        if (!pair || typeof pair !== 'object' || Array.isArray(pair)) {
            errors.push(`${base} must be an object`);
            return;
        }
        const symbol = requireString(manifest, `${base}.symbol`, errors);
        const expected = EXPECTED_PAIRS.get(symbol);
        if (!expected) {
            errors.push(`${base}.symbol must be one of ${Array.from(EXPECTED_PAIRS.keys()).join(', ')}`);
            return;
        }
        if (seenPairs.has(symbol)) {
            errors.push(`${base}.symbol duplicates ${symbol}`);
        }
        seenPairs.add(symbol);

        const pairId = bigintAt(manifest, `${base}.pair_id`, errors);
        const poolId = bigintAt(manifest, `${base}.pool_id`, errors);
        if (pairId !== BigInt(expected.pair_id)) {
            errors.push(`${base}.pair_id must equal ${expected.pair_id} for ${symbol}`);
        }
        if (poolId !== BigInt(expected.pool_id)) {
            errors.push(`${base}.pool_id must equal ${expected.pool_id} for ${symbol}`);
        }

        if (expected.whole_lot_required) {
            requireBooleanTrue(manifest, `${base}.whole_lot_aware`, errors);
        }

        const enabled = readPath(manifest, `${base}.enabled`) === true;
        const tvlCap = bigintAt(manifest, `${base}.tvl_cap`, errors, { positive: enabled });
        const volumeCap = bigintAt(manifest, `${base}.volume_cap`, errors, { positive: enabled });
        const rewardBudget = bigintAt(manifest, `${base}.reward_budget`, errors, { positive: enabled });
        const rewardRate = bigintAt(manifest, `${base}.reward_rate_per_slot`, errors, { positive: enabled });

        if (!enabled && (tvlCap !== 0n || volumeCap !== 0n || rewardBudget !== 0n || rewardRate !== 0n)) {
            errors.push(`${base} disabled pairs must have zero tvl_cap, volume_cap, reward_budget, and reward_rate_per_slot`);
        }
        if (enabled) {
            enabledCount += 1;
            totalTvlCap += tvlCap;
            totalVolumeCap += volumeCap;
            totalRewardBudget += rewardBudget;
        }
    });

    if (enabledCount === 0) {
        errors.push('pairs must enable at least one Neo liquidity corridor pair');
    }

    return { enabledCount, totalRewardBudget, totalTvlCap, totalVolumeCap, seenPairs };
}

function validateActivation(manifest, errors) {
    const mode = requireString(manifest, 'activation.mode', errors);
    if (mode !== 'post_genesis_governance') {
        errors.push('activation.mode must be post_genesis_governance; fresh chains apply the same governed campaign payload after genesis');
    }
    requireString(manifest, 'activation.governance_proposal.proposal_id', errors);
    requireHash(manifest, 'activation.governance_proposal.payload_hash', errors);
    const timelock = bigintAt(manifest, 'activation.governance_proposal.timelock_seconds', errors);
    if (timelock < 1n) {
        errors.push('activation.governance_proposal.timelock_seconds must be positive');
    }
    const rateSummary = requireString(
        manifest,
        'activation.governance_proposal.rate_change_summary',
        errors
    );
    if (!rateSummary.includes('dex_rewards.configure_lp_campaign')) {
        errors.push('activation.governance_proposal.rate_change_summary must reference dex_rewards.configure_lp_campaign');
    }
}

function validateManifest(manifest) {
    const errors = [];

    if (!manifest || typeof manifest !== 'object' || Array.isArray(manifest)) {
        return { ok: false, errors: ['manifest must be a JSON object'] };
    }

    if (manifest.id !== 'NX-950-neo-liquidity-corridor') {
        errors.push('id must be NX-950-neo-liquidity-corridor');
    }
    const version = bigintAt(manifest, 'version', errors);
    if (version < 1n) {
        errors.push('version must be at least 1');
    }
    const network = requireString(manifest, 'network', errors);
    if (!['testnet', 'mainnet'].includes(network)) {
        errors.push('network must be testnet or mainnet');
    }

    requireBooleanTrue(manifest, 'scope.liquidity_corridor', errors);
    requireBooleanTrue(manifest, 'scope.neo_route_required', errors);
    requireBooleanTrue(manifest, 'scope.uses_existing_dex_rewards', errors);
    for (const futureScope of ['zk_stark_services', 'prediction_collateral', 'agent_compute']) {
        if (readPath(manifest, `scope.${futureScope}`) === true) {
            errors.push(`scope.${futureScope} must remain false until its separate strict-order manifest is approved`);
        }
    }

    for (const approval of ['product', 'governance', 'security', 'custody', 'legal_compliance', 'market_risk', 'deployment']) {
        requireApproval(manifest, approval, errors);
    }

    const pairSummary = validatePairs(manifest, errors);
    const campaignBudget = bigintAt(manifest, 'caps.campaign_budget', errors);
    const dailyRewardCap = bigintAt(manifest, 'caps.daily_reward_cap', errors);
    const perWalletRewardCap = bigintAt(manifest, 'caps.per_wallet_reward_cap', errors);
    const maxTotalTvl = bigintAt(manifest, 'caps.max_total_tvl', errors);
    const maxTotalVolume = bigintAt(manifest, 'caps.max_total_volume', errors);
    const lossLimit = bigintAt(manifest, 'caps.loss_limit', errors);

    if (pairSummary.totalRewardBudget > campaignBudget) {
        errors.push('sum(pairs.reward_budget for enabled pairs) must not exceed caps.campaign_budget');
    }
    if (pairSummary.totalTvlCap > maxTotalTvl) {
        errors.push('sum(pairs.tvl_cap for enabled pairs) must not exceed caps.max_total_tvl');
    }
    if (pairSummary.totalVolumeCap > maxTotalVolume) {
        errors.push('sum(pairs.volume_cap for enabled pairs) must not exceed caps.max_total_volume');
    }
    if (dailyRewardCap > campaignBudget) {
        errors.push('caps.daily_reward_cap must not exceed caps.campaign_budget');
    }
    if (perWalletRewardCap > campaignBudget) {
        errors.push('caps.per_wallet_reward_cap must not exceed caps.campaign_budget');
    }
    if (lossLimit > campaignBudget) {
        errors.push('caps.loss_limit must not exceed caps.campaign_budget');
    }

    const rewardAsset = requireString(manifest, 'funding.reward_asset', errors);
    if (rewardAsset !== 'LICN') {
        errors.push('funding.reward_asset must be LICN because the current dex_rewards contract pays LICN');
    }
    requireString(manifest, 'funding.reward_source_policy', errors);
    requireString(manifest, 'funding.funding_tx_or_commitment', errors);
    const fundingAmount = bigintAt(manifest, 'funding.funding_amount', errors);
    requireHash(manifest, 'funding.treasury_attestation_hash', errors);
    requireHash(manifest, 'funding.campaign_policy_hash', errors);
    if (fundingAmount < campaignBudget) {
        errors.push('funding.funding_amount must cover caps.campaign_budget');
    }

    requireBooleanTrue(manifest, 'accounting.uses_dex_rewards_self_custody', errors);
    requireBooleanTrue(manifest, 'accounting.rewards_cannot_exceed_campaign_budget', errors);
    requireBooleanTrue(manifest, 'accounting.reward_rate_payload_matches_pairs', errors);
    requireString(manifest, 'accounting.budget_exhaustion_behavior', errors);
    requireString(manifest, 'accounting.rounding_policy', errors);
    requireString(manifest, 'accounting.audit_outputs', errors);

    requireBooleanTrue(manifest, 'risk_controls.route_pause_blocks_new_campaign_activity', errors);
    requireBooleanTrue(manifest, 'risk_controls.user_exits_remain_available_when_campaign_paused', errors);
    requireBooleanTrue(manifest, 'risk_controls.whole_lot_wneo_preserved', errors);
    requireBooleanTrue(manifest, 'risk_controls.no_unbacked_wrapped_assets', errors);
    requireString(manifest, 'risk_controls.rollback_steps', errors);

    requireString(manifest, 'disclosure.version', errors);
    requireHash(manifest, 'disclosure.hash', errors);
    requireString(manifest, 'disclosure.public_url', errors, {
        pattern: HTTP_URL,
        description: 'an https URL',
    });
    requireBooleanTrue(manifest, 'disclosure.copy_reviewed', errors);

    validateActivation(manifest, errors);

    requireBooleanTrue(manifest, 'operations.watchtower_enabled', errors);
    requireString(manifest, 'operations.rollback_runbook', errors);
    requireString(manifest, 'operations.monitoring_thresholds', errors);
    requireString(manifest, 'operations.public_status_copy', errors);

    for (const evidencePath of [
        'evidence.local_3_validator',
        'evidence.dex_e2e',
        'evidence.amm_router_candles',
        'evidence.reward_budget_exhaustion',
        'evidence.route_pause_no_new_activity',
        'evidence.no_trapped_funds',
        'evidence.wallet_dex_disclosure',
        'evidence.watchtower',
    ]) {
        requireEvidence(manifest, evidencePath, errors);
    }

    return { ok: errors.length === 0, errors };
}

function parseArgs(argv) {
    let manifestPath = '';
    for (let index = 2; index < argv.length; index += 1) {
        const arg = argv[index];
        if (arg === '--help' || arg === '-h') {
            return { help: true };
        }
        if (arg === '--manifest') {
            manifestPath = argv[index + 1] || '';
            index += 1;
            continue;
        }
        if (!manifestPath && !arg.startsWith('-')) {
            manifestPath = arg;
            continue;
        }
        throw new Error(`unknown argument: ${arg}`);
    }
    return { manifestPath };
}

function main() {
    let args;
    try {
        args = parseArgs(process.argv);
    } catch (error) {
        console.error(error.message);
        console.error(usage());
        process.exit(2);
    }

    if (args.help) {
        console.log(usage());
        return;
    }
    if (!args.manifestPath) {
        console.error('Missing --manifest <path>');
        console.error(usage());
        process.exit(2);
    }

    const manifestPath = path.resolve(args.manifestPath);
    let manifest;
    try {
        manifest = JSON.parse(fs.readFileSync(manifestPath, 'utf8'));
    } catch (error) {
        console.error('NX-950 Neo liquidity corridor gate: FAIL');
        console.error(`Could not read ${manifestPath}: ${error.message}`);
        process.exit(1);
    }

    const result = validateManifest(manifest);
    if (!result.ok) {
        console.error('NX-950 Neo liquidity corridor gate: FAIL');
        for (const error of result.errors) {
            console.error(` - ${error}`);
        }
        process.exit(1);
    }

    console.log('NX-950 Neo liquidity corridor gate: PASS');
}

if (require.main === module) {
    main();
}

module.exports = {
    validateManifest,
};
