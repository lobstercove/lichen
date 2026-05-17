#!/usr/bin/env node
'use strict';

const fs = require('fs');
const path = require('path');

const HEX32 = /^[0-9a-fA-F]{64}$/;
const BASE58 = /^[1-9A-HJ-NP-Za-km-z]{32,64}$/;
const HTTP_URL = /^https:\/\/[^ \t\r\n]+$/i;

function usage() {
    return [
        'Usage: node scripts/qa/check_neo_public_beta_gate.js --manifest <path>',
        '',
        'Validates the NX-900 Neo public beta approval manifest before public',
        'Neo route or Neo GAS rewards activation. The checker is intentionally',
        'fail-closed: missing approvals, caps, disclosure hashes, funding',
        'evidence, rollback data, or test evidence are release blockers.',
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

function envValue(env, key) {
    const value = env && typeof env === 'object' ? env[key] : undefined;
    if (typeof value === 'number' || typeof value === 'bigint') {
        return String(value);
    }
    return typeof value === 'string' ? value.trim() : '';
}

function validateFreshGenesisEnv(manifest, errors) {
    const env = readPath(manifest, 'activation.fresh_genesis_env');
    if (!env || typeof env !== 'object' || Array.isArray(env)) {
        errors.push('activation.fresh_genesis_env must be present for fresh_genesis activation');
        return;
    }

    const expected = {
        LICHEN_GENESIS_NEO_GAS_REWARDS_ENABLE: '1',
        LICHEN_GENESIS_NEO_GAS_REWARDS_ROUTE_CAP: String(readPath(manifest, 'caps.rewards_route_cap') ?? ''),
        LICHEN_GENESIS_NEO_GAS_REWARDS_PER_USER_CAP: String(readPath(manifest, 'caps.rewards_per_user_cap') ?? ''),
        LICHEN_GENESIS_NEO_GAS_REWARDS_DAILY_IMPORT_CAP: String(readPath(manifest, 'caps.rewards_daily_import_cap') ?? ''),
        LICHEN_GENESIS_NEO_GAS_REWARDS_DAILY_CLAIM_CAP: String(readPath(manifest, 'caps.rewards_daily_claim_cap') ?? ''),
        LICHEN_GENESIS_NEO_GAS_REWARDS_CAMPAIGN_BUDGET: String(readPath(manifest, 'caps.rewards_campaign_budget') ?? ''),
        LICHEN_GENESIS_NEO_GAS_REWARDS_DISCLOSURE_VERSION: String(readPath(manifest, 'disclosure.version') ?? ''),
        LICHEN_GENESIS_NEO_GAS_REWARDS_DISCLOSURE_HASH: String(readPath(manifest, 'disclosure.hash') ?? ''),
        LICHEN_GENESIS_NEO_GAS_REWARDS_POLICY_VERSION: String(readPath(manifest, 'funding.reward_policy_version') ?? ''),
        LICHEN_GENESIS_NEO_GAS_REWARDS_POLICY_HASH: String(readPath(manifest, 'funding.reward_policy_hash') ?? ''),
    };

    for (const [key, expectedValue] of Object.entries(expected)) {
        if (envValue(env, key) !== expectedValue) {
            errors.push(`activation.fresh_genesis_env.${key} must equal manifest value ${expectedValue}`);
        }
    }

    const importer = envValue(env, 'LICHEN_GENESIS_NEO_GAS_REWARDS_IMPORTER_PUBKEY');
    const manifestImporter = readPath(manifest, 'operations.reward_importer_pubkey');
    if (importer && importer !== manifestImporter) {
        errors.push('activation.fresh_genesis_env.LICHEN_GENESIS_NEO_GAS_REWARDS_IMPORTER_PUBKEY must match operations.reward_importer_pubkey when set');
    }
}

function validatePostGenesisGovernance(manifest, errors) {
    requireString(manifest, 'activation.governance_proposal.proposal_id', errors);
    requireString(manifest, 'activation.governance_proposal.payload_hash', errors, {
        pattern: HEX32,
        description: 'a 32-byte lowercase or uppercase hex hash',
    });
    const timelock = bigintAt(manifest, 'activation.governance_proposal.timelock_seconds', errors);
    if (timelock < 1n) {
        errors.push('activation.governance_proposal.timelock_seconds must be positive');
    }
}

function validateManifest(manifest) {
    const errors = [];

    if (!manifest || typeof manifest !== 'object' || Array.isArray(manifest)) {
        return { ok: false, errors: ['manifest must be a JSON object'] };
    }

    if (manifest.id !== 'NX-900-neo-public-beta') {
        errors.push('id must be NX-900-neo-public-beta');
    }
    const version = bigintAt(manifest, 'version', errors);
    if (version < 1n) {
        errors.push('version must be at least 1');
    }
    const network = requireString(manifest, 'network', errors);
    if (!['testnet', 'mainnet'].includes(network)) {
        errors.push('network must be testnet or mainnet');
    }

    requireBooleanTrue(manifest, 'scope.neo_route', errors);
    requireBooleanTrue(manifest, 'scope.neo_gas_rewards', errors);
    for (const futureScope of ['liquidity_corridor', 'zk_stark_services', 'prediction_collateral', 'agent_compute']) {
        if (readPath(manifest, `scope.${futureScope}`) === true) {
            errors.push(`scope.${futureScope} must remain false until its separate strict-order manifest is approved`);
        }
    }

    for (const approval of ['owner', 'security', 'custody', 'legal_compliance', 'deployment']) {
        requireApproval(manifest, approval, errors);
    }
    if (network === 'mainnet' || readPath(manifest, 'scope.neo_gas_rewards') === true) {
        requireApproval(manifest, 'governance', errors);
    }

    const wneoRouteCap = bigintAt(manifest, 'caps.wneo_route_cap', errors);
    const wneoPerUserCap = bigintAt(manifest, 'caps.wneo_per_user_cap', errors);
    const wgasRouteCap = bigintAt(manifest, 'caps.wgas_route_cap', errors);
    const wgasPerUserCap = bigintAt(manifest, 'caps.wgas_per_user_cap', errors);
    const rewardsRouteCap = bigintAt(manifest, 'caps.rewards_route_cap', errors);
    const rewardsPerUserCap = bigintAt(manifest, 'caps.rewards_per_user_cap', errors);
    const rewardsDailyImportCap = bigintAt(manifest, 'caps.rewards_daily_import_cap', errors);
    const rewardsDailyClaimCap = bigintAt(manifest, 'caps.rewards_daily_claim_cap', errors);
    const rewardsCampaignBudget = bigintAt(manifest, 'caps.rewards_campaign_budget', errors);
    if (wneoPerUserCap > wneoRouteCap) errors.push('caps.wneo_per_user_cap must not exceed caps.wneo_route_cap');
    if (wgasPerUserCap > wgasRouteCap) errors.push('caps.wgas_per_user_cap must not exceed caps.wgas_route_cap');
    if (rewardsPerUserCap > rewardsRouteCap) errors.push('caps.rewards_per_user_cap must not exceed caps.rewards_route_cap');
    if (rewardsDailyImportCap > rewardsCampaignBudget) errors.push('caps.rewards_daily_import_cap must not exceed caps.rewards_campaign_budget');
    if (rewardsDailyClaimCap > rewardsCampaignBudget) errors.push('caps.rewards_daily_claim_cap must not exceed caps.rewards_campaign_budget');

    requireString(manifest, 'funding.reward_source_policy', errors);
    bigintAt(manifest, 'funding.wgas_reward_budget', errors);
    requireString(manifest, 'funding.wgas_reward_funding_tx_or_commitment', errors);
    requireString(manifest, 'funding.reserve_attestation_hash', errors, {
        pattern: HEX32,
        description: 'a 32-byte hex hash',
    });
    bigintAt(manifest, 'funding.reward_policy_version', errors);
    requireString(manifest, 'funding.reward_policy_hash', errors, {
        pattern: HEX32,
        description: 'a 32-byte hex hash',
    });

    bigintAt(manifest, 'disclosure.version', errors);
    requireString(manifest, 'disclosure.hash', errors, {
        pattern: HEX32,
        description: 'a 32-byte hex hash',
    });
    requireString(manifest, 'disclosure.public_url', errors, {
        pattern: HTTP_URL,
        description: 'an https URL',
    });
    requireBooleanTrue(manifest, 'disclosure.copy_reviewed', errors);

    const activationMode = requireString(manifest, 'activation.mode', errors);
    if (activationMode === 'fresh_genesis') {
        validateFreshGenesisEnv(manifest, errors);
    } else if (activationMode === 'post_genesis_governance') {
        validatePostGenesisGovernance(manifest, errors);
    } else {
        errors.push('activation.mode must be fresh_genesis or post_genesis_governance');
    }

    requireString(manifest, 'operations.reward_importer_pubkey', errors, {
        pattern: BASE58,
        description: 'a base58 public key',
    });
    requireString(manifest, 'operations.incident_guardian_pubkey', errors, {
        pattern: BASE58,
        description: 'a base58 public key',
    });
    requireBooleanTrue(manifest, 'operations.watchtower_enabled', errors);
    requireString(manifest, 'operations.rollback_runbook', errors);
    requireString(manifest, 'operations.monitoring_thresholds', errors);

    for (const evidencePath of [
        'evidence.local_3_validator',
        'evidence.reused_chain_e2e',
        'evidence.no_trapped_funds',
        'evidence.wallet_audit',
        'evidence.extension_audit',
        'evidence.dex_oracle_candle_smoke',
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
        console.error(`NX-900 Neo public beta gate: FAIL`);
        console.error(`Could not read ${manifestPath}: ${error.message}`);
        process.exit(1);
    }

    const result = validateManifest(manifest);
    if (!result.ok) {
        console.error('NX-900 Neo public beta gate: FAIL');
        for (const error of result.errors) {
            console.error(` - ${error}`);
        }
        process.exit(1);
    }

    console.log('NX-900 Neo public beta gate: PASS');
}

if (require.main === module) {
    main();
}

module.exports = {
    validateManifest,
};
