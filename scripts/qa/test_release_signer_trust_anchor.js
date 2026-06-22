#!/usr/bin/env node
'use strict';

const fs = require('fs');
const path = require('path');

const repoRoot = path.join(__dirname, '..', '..');
const keypairPath = process.env.LICHEN_RELEASE_SIGNING_KEYPAIR || path.join(repoRoot, 'keypairs', 'release-signing-key.json');
const trustAnchorPath = path.join(repoRoot, 'deploy', 'release-trust-anchor.json');
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

function readJsonPath(filePath) {
    return JSON.parse(fs.readFileSync(filePath, 'utf8'));
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
    const keypair = readJsonPath(keypairPath);
    const seed = Array.isArray(keypair.privateKey) ? Uint8Array.from(keypair.privateKey) : null;
    if (!seed || seed.length !== 32) {
        throw new Error(`${keypairPath} must contain a 32-byte privateKey seed array`);
    }

    const { publicKeyToAddress, signMessage } = await import(`file://${pqModulePath}`);
    const signature = await signMessage(seed, Buffer.from('lichen-release-signer-trust-anchor', 'utf8'));
    return publicKeyToAddress(signature.public_key.bytes, signature.scheme_version || 1);
}

(async () => {
    const trustAnchor = readJson('deploy/release-trust-anchor.json');
    const releaseSigner = String(trustAnchor.release_signer || '').trim();
    assert(/^[1-9A-HJ-NP-Za-km-z]+$/.test(releaseSigner), 'public release signer trust anchor is base58');
    console.log(`Release signer public trust anchor: ${releaseSigner}`);

    if (fs.existsSync(keypairPath)) {
        const derivedSigner = await deriveReleaseSignerAddress();
        console.log(`Release signer derived from local keypair: ${derivedSigner}`);
        assert(derivedSigner === releaseSigner, 'local release keypair matches public trust anchor');
    } else {
        console.log(`Local release keypair not present; CI is validating public trust anchor ${trustAnchorPath}`);
    }

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
    assert(
        runbook.includes('node "$REPO_ROOT/scripts/verify-release-checksums.mjs" .') &&
            runbook.includes('export LICHEN_RELEASE_TAG=v0.5.195'),
        'mainnet launch runbook verifies detached release checksums and documents signed rollback'
    );

    const readme = readText('README.md');
    assert(
        readme.includes('SHA256SUMS.sig') &&
            readme.includes('node scripts/verify-release-checksums.mjs .'),
        'README manual install verifies detached release checksum signature'
    );

    const verifier = readText('scripts/verify-release-checksums.mjs');
    assert(
        verifier.includes('deploy') &&
            verifier.includes('release-trust-anchor.json') &&
            verifier.includes('verifySignature(signature, message, expectedSigner)'),
        'manual release checksum verifier uses the repo release trust anchor'
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
