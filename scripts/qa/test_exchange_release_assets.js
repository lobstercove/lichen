#!/usr/bin/env node
'use strict';

const fs = require('fs');

const read = (path) => fs.readFileSync(path, 'utf8');
const runtimeVersion = read('validator/Cargo.toml').match(/^version = "([^"]+)"/m)?.[1];
const tag = `exchange-testnet-v${runtimeVersion}`;
const asset = `lichen-exchange-testnet-v${runtimeVersion}.tar.gz`;
const readiness = read('scripts/qa/exchange_public_readiness.py');
const packageScript = read('scripts/package-exchange-release.mjs');
const workflow = read('.github/workflows/exchange-release.yml');
const portal = read('developers/exchange-integration.html');

const checks = [
    [readiness.includes(`EXCHANGE_PACKAGE_TAG = "${tag}"`), 'readiness gate matches exchange package tag'],
    [readiness.includes(`"${asset}"`), 'readiness gate matches exchange archive name'],
    [packageScript.includes('rollback_anchor: \'v0.5.265\''), 'package manifest records the signed rollback anchor'],
    [packageScript.includes('docs/guides/EXCHANGE_ADDRESS_VALIDATION_VECTORS.md'), 'package includes address vectors'],
    [packageScript.includes('deploy/release-trust-anchor.json'), 'package includes release trust anchor'],
    [packageScript.includes('monitoring/shared/pq.mjs'), 'package includes self-contained ML-DSA verifier'],
    [workflow.includes('name: Attest exchange archive'), 'workflow attests the exchange archive'],
    [workflow.includes('name: Attest exchange checksums'), 'workflow attests exchange checksums'],
    [workflow.includes('SHA256SUMS.sig'), 'workflow requires detached checksum signature before publication'],
    [workflow.includes('--release-stage candidate'), 'workflow verifies the signed candidate before publication'],
    [workflow.includes('--release-stage published'), 'workflow verifies the complete public release afterward'],
    [portal.includes(tag), 'developer portal links the current exchange package'],
];

let failed = 0;
for (const [condition, label] of checks) {
    if (condition) process.stdout.write(`PASS ${label}\n`);
    else {
        failed += 1;
        process.stderr.write(`FAIL ${label}\n`);
    }
}
if (failed) process.exit(1);
process.stdout.write('\nExchange release assets: PASS\n');
