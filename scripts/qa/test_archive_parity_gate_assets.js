#!/usr/bin/env node
'use strict';

const fs = require('fs');
const path = require('path');
const { spawnSync } = require('child_process');

const repoRoot = path.join(__dirname, '..', '..');
const harnessPath = 'tests/local-multi-validator-test.sh';
const entrypoints = [
    'tests/e2e-volume.js',
    'tests/e2e-launchpad.js',
];
const policyPaths = [
    'docs/deployment/ARCHIVE_PARITY_REPAIR_PLAN_2026-07-09.md',
    'docs/deployment/TESTNET_STATE_AND_SYNC_POLICY.md',
];

let passed = 0;
let failed = 0;

function assert(condition, label) {
    if (condition) {
        passed += 1;
        console.log(`  PASS ${label}`);
    } else {
        failed += 1;
        console.error(`  FAIL ${label}`);
    }
}

function repoPath(relativePath) {
    return path.join(repoRoot, relativePath);
}

function isFile(relativePath) {
    try {
        return fs.statSync(repoPath(relativePath)).isFile();
    } catch {
        return false;
    }
}

function isTracked(relativePath) {
    return spawnSync(
        'git',
        ['ls-files', '--error-unmatch', '--', relativePath],
        { cwd: repoRoot, stdio: 'ignore' },
    ).status === 0;
}

function isIgnored(relativePath) {
    return spawnSync(
        'git',
        ['check-ignore', '--no-index', '-q', '--', relativePath],
        { cwd: repoRoot, stdio: 'ignore' },
    ).status === 0;
}

function verifySource(relativePath) {
    assert(isFile(relativePath), `${relativePath} exists`);
    assert(isTracked(relativePath), `${relativePath} is Git-tracked`);
    assert(!isIgnored(relativePath), `${relativePath} is not ignored`);
}

function resolveLocalRequire(fromPath, request) {
    const base = path.posix.normalize(path.posix.join(path.posix.dirname(fromPath), request));
    for (const candidate of [base, `${base}.js`, path.posix.join(base, 'index.js')]) {
        if (isFile(candidate)) {
            return candidate;
        }
    }
    return null;
}

function collectLocalDependencies(entrypoint) {
    const pending = [entrypoint];
    const visited = new Set();
    const missing = [];

    while (pending.length > 0) {
        const relativePath = pending.pop();
        if (visited.has(relativePath)) {
            continue;
        }
        visited.add(relativePath);

        const source = fs.readFileSync(repoPath(relativePath), 'utf8');
        const executableSource = source
            .replace(/\/\*[\s\S]*?\*\//g, '')
            .replace(/^\s*\/\/.*$/gm, '');
        for (const match of executableSource.matchAll(/require\(\s*['"](\.{1,2}\/[^'"]+)['"]\s*\)/g)) {
            const resolved = resolveLocalRequire(relativePath, match[1]);
            if (!resolved) {
                missing.push(`${relativePath}: ${match[1]}`);
            } else if (!visited.has(resolved)) {
                pending.push(resolved);
            }
        }
    }

    return { dependencies: Array.from(visited).sort(), missing };
}

const harness = fs.readFileSync(repoPath(harnessPath), 'utf8');
verifySource(harnessPath);
for (const policyPath of policyPaths) {
    verifySource(policyPath);
}

const allDependencies = new Set();
for (const entrypoint of entrypoints) {
    assert(
        harness.includes(`node "$REPO_ROOT/${entrypoint}"`),
        `${harnessPath} invokes ${entrypoint}`,
    );
    const { dependencies, missing } = collectLocalDependencies(entrypoint);
    for (const unresolved of missing) {
        console.error(`  Missing local dependency: ${unresolved}`);
    }
    assert(missing.length === 0, `${entrypoint} resolves every local require`);
    for (const dependency of dependencies) {
        allDependencies.add(dependency);
    }
}

for (const dependency of Array.from(allDependencies).sort()) {
    verifySource(dependency);
}

const releaseWorkflow = fs.readFileSync(repoPath('.github/workflows/release.yml'), 'utf8');
assert(
    releaseWorkflow.includes('LICHEN_RUN_VOLUME_E2E=1'),
    'release workflow enables strict volume journeys',
);
assert(
    releaseWorkflow.includes('LICHEN_RUN_LAUNCHPAD_E2E=1'),
    'release workflow enables launchpad journeys',
);
assert(
    releaseWorkflow.includes('bash tests/local-multi-validator-test.sh 4'),
    'release workflow runs the tracked four-validator harness',
);

console.log(`\nArchive parity gate asset QA: ${passed} passed, ${failed} failed`);
if (failed > 0) {
    process.exit(1);
}
