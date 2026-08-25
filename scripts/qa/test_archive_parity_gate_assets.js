#!/usr/bin/env node
'use strict';

const fs = require('fs');
const path = require('path');
const { spawnSync } = require('child_process');
const { builtinModules } = require('module');

const repoRoot = path.join(__dirname, '..', '..');
const harnessPath = 'tests/local-multi-validator-test.sh';
const entrypoints = [
    'tests/e2e-volume.js',
    'tests/e2e-launchpad.js',
];
const supportPaths = [
    'tests/archive-v2-https-source.py',
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

function packageNameFromRequest(request) {
    if (request.startsWith('@')) {
        return request.split('/').slice(0, 2).join('/');
    }
    return request.split('/')[0];
}

function collectLocalDependencies(entrypoint) {
    const pending = [entrypoint];
    const visited = new Set();
    const missing = [];
    const packages = new Map();
    const builtins = new Set([
        ...builtinModules,
        ...builtinModules.map((moduleName) => `node:${moduleName}`),
    ]);

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
        for (const match of executableSource.matchAll(/(?:require|import)\(\s*['"]([^'"]+)['"]\s*\)/g)) {
            const request = match[1];
            if (request.startsWith('./') || request.startsWith('../')) {
                const resolved = resolveLocalRequire(relativePath, request);
                if (!resolved) {
                    missing.push(`${relativePath}: ${request}`);
                } else if (!visited.has(resolved)) {
                    pending.push(resolved);
                }
            } else if (!builtins.has(request)) {
                const packageName = packageNameFromRequest(request);
                const importers = packages.get(packageName) || [];
                importers.push(`${relativePath}: ${request}`);
                packages.set(packageName, importers);
            }
        }
    }

    return { dependencies: Array.from(visited).sort(), missing, packages };
}

function extractWorkflowJob(workflow, jobName) {
    const marker = `  ${jobName}:`;
    const start = workflow.indexOf(marker);
    if (start === -1) {
        return '';
    }
    const remainder = workflow.slice(start + marker.length);
    const nextJob = remainder.search(/\n  [a-zA-Z0-9_-]+:\s*\n/);
    return nextJob === -1
        ? workflow.slice(start)
        : workflow.slice(start, start + marker.length + nextJob);
}

const harness = fs.readFileSync(repoPath(harnessPath), 'utf8');
verifySource(harnessPath);
for (const supportPath of supportPaths) {
    verifySource(supportPath);
}
for (const policyPath of policyPaths) {
    verifySource(policyPath);
}

const allDependencies = new Set();
const allPackages = new Map();
for (const entrypoint of entrypoints) {
    assert(
        harness.includes(`node "$REPO_ROOT/${entrypoint}"`),
        `${harnessPath} invokes ${entrypoint}`,
    );
    const { dependencies, missing, packages } = collectLocalDependencies(entrypoint);
    for (const unresolved of missing) {
        console.error(`  Missing local dependency: ${unresolved}`);
    }
    assert(missing.length === 0, `${entrypoint} resolves every local require`);
    for (const dependency of dependencies) {
        allDependencies.add(dependency);
    }
    for (const [packageName, importers] of packages) {
        const allImporters = allPackages.get(packageName) || [];
        allImporters.push(...importers);
        allPackages.set(packageName, allImporters);
    }
}

for (const dependency of Array.from(allDependencies).sort()) {
    verifySource(dependency);
}

const packageJson = JSON.parse(fs.readFileSync(repoPath('package.json'), 'utf8'));
const packageLock = JSON.parse(fs.readFileSync(repoPath('package-lock.json'), 'utf8'));
const declaredPackages = {
    ...packageJson.dependencies,
    ...packageJson.devDependencies,
    ...packageJson.optionalDependencies,
};
const lockedRootPackages = {
    ...packageLock.packages?.['']?.dependencies,
    ...packageLock.packages?.['']?.devDependencies,
    ...packageLock.packages?.['']?.optionalDependencies,
};
for (const [packageName, importers] of Array.from(allPackages).sort()) {
    assert(
        Object.hasOwn(declaredPackages, packageName),
        `${packageName} is declared for ${importers.join(', ')}`,
    );
    assert(
        Object.hasOwn(lockedRootPackages, packageName)
            && Boolean(packageLock.packages?.[`node_modules/${packageName}`]),
        `${packageName} is pinned by the root package lock`,
    );
}

const releaseWorkflow = fs.readFileSync(repoPath('.github/workflows/release.yml'), 'utf8');
const rollingReleaseDeploy = fs.readFileSync(repoPath('scripts/rolling-release-deploy.sh'), 'utf8');
const archiveJob = extractWorkflowJob(releaseWorkflow, 'archive-parity-local-gate');
const releaseBuildJob = extractWorkflowJob(releaseWorkflow, 'build');
const setupNodeOffset = archiveJob.indexOf('actions/setup-node@');
const npmCiOffset = archiveJob.indexOf('npm ci --ignore-scripts');
const harnessOffset = archiveJob.indexOf('bash tests/local-multi-validator-test.sh 4');
const contractDownloadOffset = releaseBuildJob.indexOf('name: Download genesis contract bundle');
const contractStageOffset = releaseBuildJob.indexOf('name: Stage the tested genesis contracts for binary embedding');
const releaseBinaryBuildOffset = releaseBuildJob.indexOf('name: Build release binary');
assert(archiveJob.length > 0, 'release workflow defines the archive parity job');
assert(setupNodeOffset >= 0, 'archive parity job installs pinned Node.js');
assert(archiveJob.includes('node-version: "22"'), 'archive parity job uses Node.js 22');
assert(npmCiOffset >= 0, 'archive parity job installs locked journey dependencies');
assert(
    setupNodeOffset < npmCiOffset && npmCiOffset < harnessOffset,
    'archive parity job installs dependencies before the four-validator harness',
);
assert(
    releaseWorkflow.includes('LICHEN_RUN_VOLUME_E2E=1'),
    'release workflow enables strict volume journeys',
);
assert(
    releaseWorkflow.includes('LICHEN_RUN_LAUNCHPAD_E2E=1'),
    'release workflow enables launchpad journeys',
);
assert(
    harnessOffset >= 0,
    'release workflow runs the tracked four-validator harness',
);
assert(
    contractDownloadOffset >= 0
        && contractStageOffset > contractDownloadOffset
        && releaseBinaryBuildOffset > contractStageOffset,
    'signed platform binaries embed the exact contract bundle that passed the contract gate',
);
assert(
    releaseBuildJob.includes('cp "$wasm" "$destination"')
        && releaseBuildJob.includes('test "$contract_count" -gt 0'),
    'release binary build fails closed when the tested contract bundle cannot be staged',
);
assert(
    releaseWorkflow.includes('--bin lichen-archive-v2')
        && releaseWorkflow.includes('cp target/${{ matrix.target }}/release/lichen-archive-v2 ${{ matrix.artifact }}/')
        && releaseWorkflow.includes('Copy-Item target/${{ matrix.target }}/release/lichen-archive-v2.exe ${{ matrix.artifact }}/'),
    'release workflow builds and packages the Archive V2 operator CLI on every platform',
);
assert(
    rollingReleaseDeploy.includes('require_archive_bin_sha "$archive" "$root" lichen-archive-v2')
        && rollingReleaseDeploy.includes('check_file_hash /usr/local/bin/lichen-archive-v2 "$EXPECTED_ARCHIVE_V2_SHA" lichen-archive-v2'),
    'signed release deployment requires, installs, and verifies the Archive V2 operator CLI',
);

console.log(`\nArchive parity gate asset QA: ${passed} passed, ${failed} failed`);
if (failed > 0) {
    process.exit(1);
}
