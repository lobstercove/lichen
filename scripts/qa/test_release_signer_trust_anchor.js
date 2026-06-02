#!/usr/bin/env node
'use strict';

const fs = require('fs');
const path = require('path');

const repoRoot = path.join(__dirname, '..', '..');
const keypairPath = path.join(repoRoot, 'keypairs', 'release-signing-key.json');
const pqModulePath = path.join(repoRoot, 'monitoring', 'shared', 'pq.mjs');

const sharedUtilsFiles = [
    'developers/shared/utils.js',
    'dex/shared/utils.js',
    'explorer/shared/utils.js',
    'faucet/shared/utils.js',
    'marketplace/shared/utils.js',
    'monitoring/shared/utils.js',
    'programs/shared/utils.js',
    'wallet/extension/shared/utils.js',
    'wallet/shared/utils.js',
];

let passed = 0;
let failed = 0;

function assert(condition, label) {
    if (condition) {
        passed += 1;
        console.log(`  PASS ${label}`);
    } else {
        failed += 1;
        console.log(`  FAIL ${label}`);
    }
}

function readText(relativePath) {
    return fs.readFileSync(path.join(repoRoot, relativePath), 'utf8');
}

function readJson(relativePath) {
    return JSON.parse(readText(relativePath));
}

function extractSharedMetadataSignerMap(source) {
    const match = source.match(/var LICHEN_SIGNED_METADATA_SIGNERS = Object\.freeze\(\{([\s\S]*?)\}\);/);
    if (!match) return null;

    const entries = {};
    for (const entry of match[1].matchAll(/['"]?([A-Za-z0-9-]+)['"]?\s*:\s*'([^']+)'/g)) {
        entries[entry[1]] = entry[2];
    }
    return entries;
}

async function deriveReleaseSignerAddress() {
    const keypair = readJson('keypairs/release-signing-key.json');
    const seed = Array.isArray(keypair.privateKey) ? Uint8Array.from(keypair.privateKey) : null;
    if (!seed || seed.length !== 32) {
        throw new Error(`${keypairPath} must contain a 32-byte privateKey seed array`);
    }

    const { publicKeyToAddress, signMessage } = await import(`file://${pqModulePath}`);
    const signature = await signMessage(seed, Buffer.from('lichen-release-signer-trust-anchor', 'utf8'));
    return publicKeyToAddress(signature.public_key.bytes, signature.scheme_version || 1);
}

(async () => {
    const releaseSigner = await deriveReleaseSignerAddress();
    console.log(`Release signer derived from keypair: ${releaseSigner}`);

    for (const relativePath of sharedUtilsFiles) {
        const signerMap = extractSharedMetadataSignerMap(readText(relativePath));
        assert(Boolean(signerMap), `${relativePath} defines signed metadata trust roots`);
        if (!signerMap) continue;

        for (const network of ['mainnet', 'testnet', 'local-testnet', 'local-mainnet']) {
            assert(
                signerMap[network] === releaseSigner,
                `${relativePath} ${network} signer matches release keypair`
            );
        }
    }

    const updater = readText('validator/src/updater.rs');
    assert(
        updater.includes(`RELEASE_SIGNING_ADDRESS_BASE58: &str = "${releaseSigner}"`),
        'validator updater release signer matches release keypair'
    );

    const runbook = readText('deploy/mainnet-launch-runbook.md');
    assert(
        runbook.includes(releaseSigner),
        'mainnet launch runbook records the release signer trust anchor'
    );

    const rollingDeploy = readText('scripts/rolling-release-deploy.sh');
    assert(
        rollingDeploy.includes(`LICHEN_RELEASE_SIGNING_ADDRESS:-${releaseSigner}`),
        'rolling release deploy verifies against the release signer trust anchor'
    );

    console.log(`\nRelease signer trust anchor QA: ${passed} passed, ${failed} failed`);
    if (failed > 0) process.exit(1);
})().catch((error) => {
    console.error(error.message || error);
    process.exit(1);
});
