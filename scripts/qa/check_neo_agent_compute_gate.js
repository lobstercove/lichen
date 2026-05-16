#!/usr/bin/env node
'use strict';

const fs = require('fs');

const HEX32 = /^[0-9a-fA-F]{64}$/;
const HTTPS_URL = /^https:\/\/[^ \t\r\n]+$/i;
const REQUIRED_PAYMENT_ASSETS = ['wGAS', 'wNEO'];
const REQUIRED_RUNTIME_FUNCTIONS = [
    'set_agent_compute_controls',
    'set_agent_spending_policy',
    'disable_agent_spending_policy',
    'submit_agent_job',
    'get_agent_compute_controls',
    'get_agent_spending_policy',
    'get_agent_spend_window',
    'get_agent_job_action',
];
const REQUIRED_REPLAY_DOMAINS = [
    'lichen_network',
    'neo_network',
    'neo_chain_id',
    'route',
    'asset',
    'purpose',
    'agent',
    'policy_hash',
];

function usage() {
    return [
        'Usage: node scripts/qa/check_neo_agent_compute_gate.js --manifest <path>',
        '',
        'Validates the NX-980 Neo agent/compute manifest before any public',
        'agent-compute activation. The checker is fail-closed: missing',
        'spending caps, PQ action evidence, route-pause behavior, task',
        'accounting, disclosure, or local 3-validator evidence are blockers.',
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
    if (typeof value === 'string' && /^[0-9]+$/.test(value.trim())) {
        return value.trim();
    }
    return null;
}

function bigintAt(manifest, dottedPath, errors, { positive = true } = {}) {
    const normalized = asIntegerString(readPath(manifest, dottedPath));
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

function requireBooleanFalse(manifest, dottedPath, errors) {
    if (readPath(manifest, dottedPath) !== false) {
        errors.push(`${dottedPath} must be false`);
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

function requireArrayContainsAll(manifest, dottedPath, required, errors) {
    const value = readPath(manifest, dottedPath);
    if (!Array.isArray(value)) {
        errors.push(`${dottedPath} must be an array`);
        return;
    }
    for (const item of required) {
        if (!value.includes(item)) {
            errors.push(`${dottedPath} must include ${item}`);
        }
    }
}

function validateManifest(manifest) {
    const errors = [];

    if (!manifest || typeof manifest !== 'object' || Array.isArray(manifest)) {
        return { ok: false, errors: ['manifest must be a JSON object'] };
    }
    if (manifest.id !== 'NX-980-neo-agent-compute') {
        errors.push('id must be NX-980-neo-agent-compute');
    }
    const version = bigintAt(manifest, 'version', errors);
    if (version < 1n) {
        errors.push('version must be at least 1');
    }
    const network = requireString(manifest, 'network', errors);
    if (!['testnet', 'mainnet'].includes(network)) {
        errors.push('network must be testnet or mainnet');
    }

    requireBooleanTrue(manifest, 'scope.agent_compute', errors);
    requireBooleanTrue(manifest, 'scope.neo_route_required', errors);
    requireBooleanTrue(manifest, 'scope.local_3_validator_required', errors);
    requireBooleanTrue(manifest, 'scope.no_state_copy_required', errors);
    requireBooleanFalse(manifest, 'scope.public_deployment_allowed', errors);
    if (readPath(manifest, 'scope.prediction_collateral') === true) {
        errors.push('scope.prediction_collateral must stay false until its own strict-order lane is approved');
    }

    for (const approval of [
        'product',
        'governance',
        'security',
        'custody',
        'legal_compliance',
        'deployment',
    ]) {
        requireApproval(manifest, approval, errors);
    }

    requireArrayContainsAll(manifest, 'assets.payment_assets', REQUIRED_PAYMENT_ASSETS, errors);
    requireString(manifest, 'assets.primary_fee_asset', errors);
    requireString(manifest, 'assets.whole_lot_wneo_policy', errors);
    requireBooleanTrue(manifest, 'assets.no_unbacked_minting', errors);

    requireString(manifest, 'spending_policy.policy_hash', errors, {
        pattern: HEX32,
        description: 'a 32-byte hex hash',
    });
    bigintAt(manifest, 'spending_policy.policy_version', errors);
    const daily = bigintAt(manifest, 'spending_policy.max_per_agent_daily', errors);
    const perTask = bigintAt(manifest, 'spending_policy.max_per_task', errors);
    if (perTask > daily) {
        errors.push('spending_policy.max_per_task must not exceed max_per_agent_daily');
    }
    requireBooleanTrue(manifest, 'spending_policy.opt_in_required', errors);
    requireBooleanTrue(manifest, 'spending_policy.agent_disable_available', errors);
    requireBooleanTrue(manifest, 'spending_policy.pq_action_hash_required', errors);
    requireBooleanTrue(manifest, 'spending_policy.route_pause_blocks_new_payments', errors);
    requireBooleanTrue(manifest, 'spending_policy.escrow_exit_unaffected_by_new_payment_pause', errors);

    requireString(manifest, 'pq_attestation.evidence_kind', errors);
    if (readPath(manifest, 'pq_attestation.evidence_kind') !== 'agent_action') {
        errors.push('pq_attestation.evidence_kind must be agent_action');
    }
    requireString(manifest, 'pq_attestation.purpose', errors);
    if (readPath(manifest, 'pq_attestation.purpose') !== 'neo-x-agent-compute') {
        errors.push('pq_attestation.purpose must be neo-x-agent-compute');
    }
    bigintAt(manifest, 'pq_attestation.required_signatures', errors);
    requireBooleanTrue(manifest, 'pq_attestation.stale_evidence_rejected', errors);
    requireBooleanTrue(manifest, 'pq_attestation.signature_mismatch_rejected', errors);
    requireArrayContainsAll(manifest, 'pq_attestation.replay_domains', REQUIRED_REPLAY_DOMAINS, errors);

    requireString(manifest, 'compute_market.contract_symbol', errors);
    if (readPath(manifest, 'compute_market.contract_symbol') !== 'COMPUTE') {
        errors.push('compute_market.contract_symbol must be COMPUTE');
    }
    requireArrayContainsAll(manifest, 'compute_market.required_functions', REQUIRED_RUNTIME_FUNCTIONS, errors);
    requireBooleanTrue(manifest, 'compute_market.per_agent_spend_accounting', errors);
    requireBooleanTrue(manifest, 'compute_market.task_action_hash_accounting', errors);
    requireBooleanTrue(manifest, 'compute_market.normal_compute_flow_unchanged', errors);

    requireString(manifest, 'disclosure.risk_disclosure_url', errors, {
        pattern: HTTPS_URL,
        description: 'an https URL',
    });
    requireString(manifest, 'disclosure.risk_disclosure_hash', errors, {
        pattern: HEX32,
        description: 'a 32-byte hex hash',
    });

    for (const evidencePath of [
        'evidence.unit_contract_policy',
        'evidence.pq_agent_action',
        'evidence.manifest_gate',
        'evidence.rpc_stats',
        'evidence.local_3_validator',
        'evidence.route_pause_blocks_payment',
        'evidence.no_regression_existing_compute',
    ]) {
        requireEvidence(manifest, evidencePath, errors);
    }

    return { ok: errors.length === 0, errors };
}

function main(argv) {
    const manifestIndex = argv.indexOf('--manifest');
    if (manifestIndex === -1 || !argv[manifestIndex + 1]) {
        console.error(usage());
        return 2;
    }
    let manifest;
    try {
        manifest = JSON.parse(fs.readFileSync(argv[manifestIndex + 1], 'utf8'));
    } catch (error) {
        console.error(`Failed to read manifest: ${error.message}`);
        return 2;
    }
    const result = validateManifest(manifest);
    if (!result.ok) {
        console.error('NX-980 Neo agent-compute gate: FAIL');
        for (const error of result.errors) {
            console.error(` - ${error}`);
        }
        return 1;
    }
    console.log('NX-980 Neo agent-compute gate: PASS');
    return 0;
}

if (require.main === module) {
    process.exitCode = main(process.argv.slice(2));
}

module.exports = { validateManifest };
