#!/usr/bin/env node
'use strict';

const fs = require('fs');

const HEX32 = /^[0-9a-fA-F]{64}$/;
const HTTP_URL = /^https:\/\/[^ \t\r\n]+$/i;
const REQUIRED_ASSETS = ['wNEO', 'wGAS', 'NEOGASRWD'];
const REQUIRED_PUBLIC_INPUTS = [
    'domain_hash',
    'statement_hash',
    'witness_commitment',
    'reserve_amount',
    'liability_amount',
    'epoch',
    'verifier_version',
];
const REQUIRED_DOMAIN_FIELDS = [
    'lichen_network',
    'neo_network',
    'neo_chain_id',
    'route',
    'asset',
    'product',
    'verifier_version',
];

function usage() {
    return [
        'Usage: node scripts/qa/check_neo_zk_proof_services_gate.js --manifest <path>',
        '',
        'Validates the NX-960 Neo ZK proof-services manifest before any',
        'public reserve/liability proof-service activation. The checker is',
        'fail-closed: missing approvals, proof-statement metadata, privacy',
        'review, benchmark evidence, local verifier evidence, or deployment',
        'approval are release blockers.',
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
    required.forEach((item) => {
        if (!value.includes(item)) {
            errors.push(`${dottedPath} must include ${item}`);
        }
    });
}

function validateManifest(manifest) {
    const errors = [];

    if (!manifest || typeof manifest !== 'object' || Array.isArray(manifest)) {
        return { ok: false, errors: ['manifest must be a JSON object'] };
    }

    if (manifest.id !== 'NX-960-neo-zk-proof-services') {
        errors.push('id must be NX-960-neo-zk-proof-services');
    }
    const version = bigintAt(manifest, 'version', errors);
    if (version < 1n) {
        errors.push('version must be at least 1');
    }
    const network = requireString(manifest, 'network', errors);
    if (!['testnet', 'mainnet'].includes(network)) {
        errors.push('network must be testnet or mainnet');
    }

    requireBooleanTrue(manifest, 'scope.zk_stark_services', errors);
    requireBooleanTrue(manifest, 'scope.neo_route_required', errors);
    requireBooleanTrue(manifest, 'scope.reserve_liability_only', errors);
    for (const futureScope of ['prediction_collateral', 'agent_compute']) {
        if (readPath(manifest, `scope.${futureScope}`) === true) {
            errors.push(`scope.${futureScope} must remain false until its separate strict-order manifest is approved`);
        }
    }

    for (const approval of [
        'product',
        'governance',
        'security',
        'custody',
        'legal_compliance',
        'privacy',
        'deployment',
    ]) {
        requireApproval(manifest, approval, errors);
    }

    requireArrayContainsAll(manifest, 'assets', REQUIRED_ASSETS, errors);

    const proofType = requireString(manifest, 'proof_statement.proof_type', errors);
    if (proofType !== 'reserve_liability') {
        errors.push('proof_statement.proof_type must be reserve_liability');
    }
    const scheme = requireString(manifest, 'proof_statement.zk_scheme', errors);
    if (scheme !== 'plonky3-fri-poseidon2') {
        errors.push('proof_statement.zk_scheme must be plonky3-fri-poseidon2');
    }
    const privacyModel = requireString(manifest, 'proof_statement.privacy_model', errors);
    if (privacyModel !== 'transparent_aggregate_totals_no_address_list_v1') {
        errors.push('proof_statement.privacy_model must be transparent_aggregate_totals_no_address_list_v1');
    }
    bigintAt(manifest, 'proof_statement.verifier_version', errors);
    requireArrayContainsAll(manifest, 'proof_statement.public_inputs', REQUIRED_PUBLIC_INPUTS, errors);
    requireArrayContainsAll(manifest, 'proof_statement.domain_fields', REQUIRED_DOMAIN_FIELDS, errors);
    requireBooleanTrue(manifest, 'proof_statement.reserve_amount_public', errors);
    requireBooleanTrue(manifest, 'proof_statement.liability_amount_public', errors);
    requireBooleanTrue(manifest, 'proof_statement.no_address_list', errors);
    requireBooleanTrue(manifest, 'proof_statement.undercollateralized_statements_rejected', errors);
    requireString(manifest, 'proof_statement.domain_separator_hash', errors, {
        pattern: HEX32,
        description: 'a 32-byte hex hash',
    });
    requireString(manifest, 'proof_statement.statement_schema_hash', errors, {
        pattern: HEX32,
        description: 'a 32-byte hex hash',
    });

    requireBooleanFalse(manifest, 'claims.direct_neox_onchain_verification', errors);
    requireString(manifest, 'claims.public_disclosure_url', errors, {
        pattern: HTTP_URL,
        description: 'an https URL',
    });
    requireString(manifest, 'claims.no_hidden_witness_claim', errors);

    const maxProofMs = bigintAt(manifest, 'benchmarks.max_proof_ms', errors);
    const maxVerifyMs = bigintAt(manifest, 'benchmarks.max_verify_ms', errors);
    if (maxVerifyMs > maxProofMs) {
        errors.push('benchmarks.max_verify_ms should not exceed benchmarks.max_proof_ms');
    }
    requireString(manifest, 'benchmarks.evidence', errors);

    const activationMode = requireString(manifest, 'activation.mode', errors);
    if (!['local_rehearsal', 'post_genesis_governance', 'fresh_genesis'].includes(activationMode)) {
        errors.push('activation.mode must be local_rehearsal, post_genesis_governance, or fresh_genesis');
    }
    requireString(manifest, 'activation.rollback_runbook', errors);
    requireString(manifest, 'activation.deployment_scope', errors);

    for (const evidencePath of [
        'evidence.local_3_validator',
        'evidence.cli_proof_generation',
        'evidence.native_verification',
        'evidence.rpc_verification',
        'evidence.sdk_consumer',
        'evidence.public_input_binding',
        'evidence.replay_rejection',
        'evidence.privacy_leakage_review',
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
        return { error: `unknown argument: ${arg}` };
    }
    if (!manifestPath) {
        return { error: 'missing --manifest <path>' };
    }
    return { manifestPath };
}

function main(argv = process.argv) {
    const args = parseArgs(argv);
    if (args.help) {
        console.log(usage());
        return 0;
    }
    if (args.error) {
        console.error(args.error);
        console.error(usage());
        return 2;
    }

    let manifest;
    try {
        manifest = JSON.parse(fs.readFileSync(args.manifestPath, 'utf8'));
    } catch (error) {
        console.error(`failed to read manifest: ${error.message}`);
        return 2;
    }

    const result = validateManifest(manifest);
    if (!result.ok) {
        console.error('NX-960 Neo ZK proof services gate: FAIL');
        result.errors.forEach((error) => console.error(` - ${error}`));
        return 1;
    }

    console.log('NX-960 Neo ZK proof services gate: PASS');
    return 0;
}

if (require.main === module) {
    process.exitCode = main();
}

module.exports = {
    validateManifest,
};
