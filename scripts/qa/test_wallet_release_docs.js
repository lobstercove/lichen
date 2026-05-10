#!/usr/bin/env node
'use strict';

const fs = require('fs');
const path = require('path');
const { requirePrivateDocs } = require('./private_docs_guard');

const ROOT = path.resolve(__dirname, '..', '..');

const files = {
    releasePlan: path.join(ROOT, 'docs', 'internal', 'wallet', 'EXTENSION_RELEASE_PLAN.md'),
    readinessPlan: path.join(ROOT, 'docs', 'internal', 'wallet', 'EXTENSION_PRODUCTION_READINESS_PLAN.md'),
    readme: path.join(ROOT, 'wallet', 'extension', 'README.md'),
    storeChecklist: path.join(ROOT, 'wallet', 'extension', 'store', 'submission-checklist.md'),
    packageJson: path.join(ROOT, 'package.json'),
    packageScript: path.join(ROOT, 'scripts', 'package-wallet-extension.mjs'),
    walletJs: path.join(ROOT, 'wallet', 'js', 'wallet.js'),
    walletSharedConfig: path.join(ROOT, 'wallet', 'shared-config.js'),
    restrictionService: path.join(ROOT, 'wallet', 'extension', 'src', 'core', 'restriction-service.js'),
    providerRouter: path.join(ROOT, 'wallet', 'extension', 'src', 'core', 'provider-router.js'),
    inpageProvider: path.join(ROOT, 'wallet', 'extension', 'src', 'content', 'inpage-provider.js'),
};

let passed = 0;
let failed = 0;

function read(file) {
    return fs.readFileSync(file, 'utf8');
}

function test(name, fn) {
    try {
        fn();
        passed += 1;
        process.stdout.write(`  PASS ${name}\n`);
    } catch (error) {
        failed += 1;
        process.stderr.write(`  FAIL ${name}: ${error.message}\n`);
    }
}

function assert(condition, message) {
    if (!condition) {
        throw new Error(message);
    }
}

function assertIncludes(source, needle, label) {
    assert(source.includes(needle), `${label} missing '${needle}'`);
}

function assertNotIncludes(source, needle, label) {
    assert(!source.includes(needle), `${label} still contains '${needle}'`);
}

function assertAllDocsInclude(docs, needles) {
    for (const [label, source] of Object.entries(docs)) {
        for (const needle of needles) {
            assertIncludes(source, needle, label);
        }
    }
}

function main() {
    requirePrivateDocs('Wallet Release Docs QA', [files.releasePlan, files.readinessPlan]);

    const sources = Object.fromEntries(
        Object.entries(files).map(([key, file]) => [key, read(file)]),
    );
    const packageJson = JSON.parse(sources.packageJson);
    const docs = {
        releasePlan: sources.releasePlan,
        readinessPlan: sources.readinessPlan,
        readme: sources.readme,
    };

    process.stdout.write('\nWallet Release Docs QA\n\n');

    test('wallet release docs include the shipped restriction governance methods', () => {
        assertAllDocsInclude(docs, [
            'lichen_getRestrictionStatus',
            'lichen_canTransfer',
            'lichen_getContractLifecycleStatus',
            'canTransfer',
            'getIncidentStatus',
        ]);
    });

    test('wallet release docs describe trusted endpoint pinning and custom RPC boundary', () => {
        assertAllDocsInclude(docs, [
            'trusted',
            'custom RPC',
            'metadata',
            'bridge',
        ]);
        assertIncludes(sources.restrictionService, 'getTrustedRpcEndpoint', 'restriction-service.js');
        assertNotIncludes(sources.restrictionService, 'getConfiguredRpcEndpoint', 'restriction-service.js');
    });

    test('wallet release docs require pre-signing restriction checks before key decryption', () => {
        assertAllDocsInclude(docs, [
            'before private key decryption',
            'before signing',
        ]);
        assertIncludes(sources.providerRouter, 'enforceRestrictionPreflight(txObject, activeWallet, context)', 'provider-router.js');
        assertIncludes(sources.restrictionService, 'preflightNativeTransferRestrictions', 'restriction-service.js');
        assertIncludes(sources.walletJs, "walletRestrictionMethod('canTransfer', 'canTransfer')", 'wallet.js');
    });

    test('wallet release docs preserve the dapp read-only boundary', () => {
        assertAllDocsInclude(docs, [
            'read-only',
            'dapps cannot suppress',
            'mutation builders',
        ]);
        [
            'buildRestrictAccountTx',
            'buildUnrestrictAccountTx',
            'buildSetContractLifecycleTx',
            'buildLiftRestrictionTx',
            'buildExtendRestrictionTx',
        ].forEach((mutation) => {
            assertNotIncludes(sources.inpageProvider, mutation, 'inpage-provider.js');
            assertNotIncludes(sources.providerRouter, `case '${mutation}'`, 'provider-router.js');
        });
    });

    test('wallet release checklist requires the full automated gate and manual restriction smoke', () => {
        [
            'npm run test-wallet-docs',
            'npm run test-wallet',
            'npm run test-wallet-extension',
            'npm run test-frontend-assets',
            'node tests/test_frontend_trust_boundaries.js',
            'npm run validate-wallet-extension-release',
            'npm run package-wallet-extension',
            'Manual Smoke Checklist',
            'restriction-warning smoke',
        ].forEach((needle) => {
            assert(
                sources.releasePlan.includes(needle)
                    || sources.readinessPlan.includes(needle)
                    || sources.storeChecklist.includes(needle),
                `release documentation missing '${needle}'`,
            );
        });
    });

    test('package scripts run wallet release docs during extension validation', () => {
        assertIncludes(packageJson.scripts['test-wallet-docs'], 'test_wallet_release_docs.js', 'package.json');
        assertIncludes(packageJson.scripts['validate-wallet-extension-release'], 'npm run test-wallet-docs', 'package.json');
        assertIncludes(packageJson.scripts['prepare-wallet-extension-release'], 'npm run validate-wallet-extension-release', 'package.json');
    });

    test('README is current release documentation instead of scaffold instructions', () => {
        assertIncludes(sources.readme, '# LichenWallet Extension', 'README.md');
        assertIncludes(sources.readme, 'Restriction-Governance Safety', 'README.md');
        assertIncludes(sources.readme, 'Dapp Restriction Preflight', 'README.md');
        assertNotIncludes(sources.readme, 'Extension Scaffold', 'README.md');
        assertNotIncludes(sources.readme, 'Next Step', 'README.md');
    });

    test('packaging and store docs keep README and review docs in the store bundle', () => {
        assertIncludes(sources.packageScript, "['README.md', 'README.md']", 'package-wallet-extension.mjs');
        assertIncludes(sources.packageScript, "['manifest.json', 'manifest.json']", 'package-wallet-extension.mjs');
        assertIncludes(sources.packageScript, "['store', 'store']", 'package-wallet-extension.mjs');
        assertIncludes(sources.storeChecklist, 'store/permissions-justification.md', 'submission-checklist.md');
        assertIncludes(sources.storeChecklist, 'store/submission-checklist.md', 'submission-checklist.md');
    });

    test('web wallet source still publishes canonical restriction RPC names', () => {
        [
            'getAccountRestrictionStatus',
            'getAssetRestrictionStatus',
            'getAccountAssetRestrictionStatus',
            'canSend',
            'canReceive',
            'canTransfer',
        ].forEach((method) => {
            assertIncludes(sources.walletSharedConfig, method, 'wallet/shared-config.js');
        });
        assertIncludes(sources.walletSharedConfig, 'nativeAsset', 'wallet/shared-config.js');
    });

    process.stdout.write(`\nWallet Release Docs QA: ${passed} passed, ${failed} failed\n`);
    if (failed > 0) {
        process.exitCode = 1;
    }
}

main();
