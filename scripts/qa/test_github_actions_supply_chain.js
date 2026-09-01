#!/usr/bin/env node
'use strict';

const fs = require('fs');
const path = require('path');

const repoRoot = path.join(__dirname, '..', '..');
const workflowsDir = path.join(repoRoot, '.github', 'workflows');
const fullShaPattern = /^[0-9a-f]{40}$/;
const approvedExternalActions = new Map([
    ['Swatinem/rust-cache@6323deb102c322ba6fcbdcafc7e3dddab59af2b6', 'node24'], // v2.9.2
    ['actions/attest@1e69f48acb82d1966a394da916b4c1698aa569d6', 'node24'], // v4.2.2
    ['actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1', 'node24'], // v7.0.1
    ['actions/download-artifact@3e5f45b2cfb9172054b4087a40e8e0b5a5461e7c', 'node24'], // v8.0.1
    ['actions/setup-node@820762786026740c76f36085b0efc47a31fe5020', 'node24'], // v7.0.0
    ['actions/setup-python@5fda3b95a4ea91299a34e894583c3862153e4b97', 'node24'], // v7.0.0
    ['actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a', 'node24'], // v7.0.1
    ['docker/build-push-action@53b7df96c91f9c12dcc8a07bcb9ccacbed38856a', 'node24'], // v7.3.0
    ['docker/setup-buildx-action@37fe631027851001ddb9b187196cc803df7f5f0e', 'node24'], // v4.3.0
    ['dtolnay/rust-toolchain@4360b52568e2003a75bf9bc1d59f33a8e3fc893c', 'composite'], // stable 2026-08-29
    ['github/codeql-action/upload-sarif@486fec2a3ea2626afcd8c7e9208b4f515078dd7e', 'node24'], // codeql-bundle-v2.26.4
    ['ossf/scorecard-action@2d1146689b8cda280b9bc96326124645441f03bc', 'docker'], // v2.4.4
    ['softprops/action-gh-release@3d0d9888cb7fd7b750713d6e236d1fcb99157228', 'node24'], // v3.0.2
]);
const approvedRuntimeClasses = new Set(['node24', 'composite', 'docker']);

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

function workflowFiles() {
    return fs.readdirSync(workflowsDir)
        .filter((name) => name.endsWith('.yml') || name.endsWith('.yaml'))
        .sort()
        .map((name) => path.join(workflowsDir, name));
}

function extractTopLevelPermissions(source) {
    const lines = source.split(/\r?\n/);
    const start = lines.findIndex((line) => /^permissions:\s*$/.test(line));
    if (start === -1) {
        return null;
    }

    const permissions = new Map();
    for (let index = start + 1; index < lines.length; index += 1) {
        const line = lines[index];
        if (/^\S/.test(line)) {
            break;
        }

        const match = line.match(/^\s+([A-Za-z0-9_-]+):\s*([A-Za-z0-9_-]+)\s*$/);
        if (match) {
            permissions.set(match[1], match[2]);
        }
    }
    return permissions;
}

function collectUses(source) {
    return source.split(/\r?\n/).flatMap((line, index) => {
        const match = line.match(/^\s*(?:-\s*)?uses:\s*([^#\s]+)\s*(?:#.*)?$/);
        if (!match) {
            return [];
        }
        return [{
            ref: match[1],
            line: index + 1,
        }];
    });
}

function isPinnedActionRef(ref) {
    if (ref.startsWith('./') || ref.startsWith('../')) {
        return true;
    }

    const separatorIndex = ref.lastIndexOf('@');
    if (separatorIndex === -1) {
        return false;
    }

    const version = ref.slice(separatorIndex + 1);
    return fullShaPattern.test(version);
}

function hasExplicitStableToolchain(source, lineNumber) {
    const lines = source.split(/\r?\n/);
    const start = lineNumber - 1;
    const baseIndent = lines[start].match(/^(\s*)/)[1].length;

    for (let index = start + 1; index < lines.length; index += 1) {
        const line = lines[index];
        const indent = line.match(/^(\s*)/)[1].length;
        if (line.trim().startsWith('- ') && indent <= baseIndent) {
            break;
        }
        if (/^\s*toolchain:\s*stable\s*$/.test(line)) {
            return true;
        }
    }
    return false;
}

for (const filePath of workflowFiles()) {
    const relativePath = path.relative(repoRoot, filePath);
    const source = fs.readFileSync(filePath, 'utf8');
    const topLevelPermissions = extractTopLevelPermissions(source);

    assert(topLevelPermissions !== null, `${relativePath} declares default workflow permissions`);
    if (topLevelPermissions) {
        const permissions = Array.from(topLevelPermissions.entries());
        assert(
            permissions.length === 1 && topLevelPermissions.get('contents') === 'read',
            `${relativePath} default workflow token is contents: read only`,
        );
    }

    const uses = collectUses(source);
    assert(uses.length > 0, `${relativePath} has action references to audit`);
    for (const actionUse of uses) {
        assert(
            isPinnedActionRef(actionUse.ref),
            `${relativePath}:${actionUse.line} pins ${actionUse.ref} to a commit SHA`,
        );

        if (!actionUse.ref.startsWith('./') && !actionUse.ref.startsWith('../')) {
            const runtimeClass = approvedExternalActions.get(actionUse.ref);
            assert(
                approvedRuntimeClasses.has(runtimeClass),
                `${relativePath}:${actionUse.line} uses a reviewed non-Node-20 action runtime`,
            );
        }

        if (actionUse.ref.startsWith('dtolnay/rust-toolchain@')) {
            assert(
                hasExplicitStableToolchain(source, actionUse.line),
                `${relativePath}:${actionUse.line} declares the Rust stable toolchain explicitly`,
            );
        }
    }
}

const releaseWorkflowPath = path.join(workflowsDir, 'release.yml');
const releaseWorkflow = fs.readFileSync(releaseWorkflowPath, 'utf8');
const releaseSdkInstall = releaseWorkflow.indexOf('npm --prefix sdk/js ci --ignore-scripts');
const releaseSdkBuild = releaseWorkflow.indexOf('npm --prefix sdk/js run build');
const releaseWalletAudit = releaseWorkflow.indexOf('node scripts/qa/test_wallet_audit.js');
assert(
    releaseSdkInstall !== -1
        && releaseSdkBuild > releaseSdkInstall
        && releaseWalletAudit > releaseSdkBuild,
    'release workflow installs and builds the JS SDK before wallet audits',
);

console.log(`\nGitHub Actions supply-chain QA: ${passed} passed, ${failed} failed`);
if (failed > 0) {
    process.exit(1);
}
