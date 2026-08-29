#!/usr/bin/env node
'use strict';

const fs = require('fs');
const path = require('path');

const repoRoot = path.join(__dirname, '..', '..');
const dockerfilePath = path.join(repoRoot, 'Dockerfile');
const rootManifestPath = path.join(repoRoot, 'Cargo.toml');
const workflowPath = path.join(repoRoot, '.github', 'workflows', 'ci.yml');

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

function read(relativePath) {
    return fs.readFileSync(path.join(repoRoot, relativePath), 'utf8');
}

function normalize(relativePath) {
    return relativePath.split(path.sep).join('/').replace(/^\.\//, '').replace(/\/$/, '');
}

function cargoManifestExists(relativeDirectory) {
    return fs.existsSync(path.join(repoRoot, relativeDirectory, 'Cargo.toml'));
}

function workspaceMembers(manifest) {
    const workspace = manifest.match(/\[workspace\]([\s\S]*?)(?=\n\[[^\]]+\]|$)/);
    const members = workspace?.[1].match(/\bmembers\s*=\s*\[([\s\S]*?)\]/);
    if (!members) {
        return [];
    }
    return Array.from(members[1].matchAll(/"([^"]+)"/g), (match) => normalize(match[1]));
}

function localPathDependencies(packageDirectory, manifest) {
    const dependencies = [];
    for (const match of manifest.matchAll(/\bpath\s*=\s*"([^"]+)"/g)) {
        if (match[1].endsWith('.rs')) {
            continue;
        }
        const resolved = normalize(path.join(packageDirectory, match[1]));
        if (cargoManifestExists(resolved)) {
            dependencies.push(resolved);
        }
    }
    return dependencies;
}

function collectLocalPackages(rootManifest) {
    const pending = workspaceMembers(rootManifest);
    const packages = new Set();

    // Root-level path patches are local packages even when they are excluded
    // from workspace membership.
    pending.push(...localPathDependencies('.', rootManifest));

    while (pending.length > 0) {
        const packageDirectory = normalize(pending.pop());
        if (packages.has(packageDirectory)) {
            continue;
        }
        packages.add(packageDirectory);
        const manifest = read(`${packageDirectory}/Cargo.toml`);
        pending.push(...localPathDependencies(packageDirectory, manifest));
    }

    return Array.from(packages).sort();
}

function defaultTargetPaths(packageDirectory) {
    const manifest = read(`${packageDirectory}/Cargo.toml`);
    const targets = new Set();
    const sourceDirectory = path.join(repoRoot, packageDirectory, 'src');

    for (const conventional of ['src/lib.rs', 'src/main.rs']) {
        if (fs.existsSync(path.join(repoRoot, packageDirectory, conventional))) {
            targets.add(`${packageDirectory}/${conventional}`);
        }
    }

    const binsDirectory = path.join(sourceDirectory, 'bin');
    if (fs.existsSync(binsDirectory)) {
        for (const entry of fs.readdirSync(binsDirectory, { withFileTypes: true })) {
            if (entry.isFile() && entry.name.endsWith('.rs')) {
                targets.add(`${packageDirectory}/src/bin/${entry.name}`);
            } else if (entry.isDirectory()
                && fs.existsSync(path.join(binsDirectory, entry.name, 'main.rs'))) {
                targets.add(`${packageDirectory}/src/bin/${entry.name}/main.rs`);
            }
        }
    }

    for (const match of manifest.matchAll(/\bpath\s*=\s*"([^"]+\.rs)"/g)) {
        const target = normalize(path.join(packageDirectory, match[1]));
        if (fs.existsSync(path.join(repoRoot, target))) {
            targets.add(target);
        }
    }

    return Array.from(targets).sort();
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

const dockerfile = fs.readFileSync(dockerfilePath, 'utf8');
const rootManifest = fs.readFileSync(rootManifestPath, 'utf8');
const workflow = fs.readFileSync(workflowPath, 'utf8');
const cacheBuildMarker = '# Build dependencies only (cached layer)';
const cacheBuildOffset = dockerfile.indexOf(cacheBuildMarker);
const finalSourcesOffset = dockerfile.indexOf('# Copy real source code');
const preCache = cacheBuildOffset >= 0 ? dockerfile.slice(0, cacheBuildOffset) : '';
const postCache = finalSourcesOffset >= 0 ? dockerfile.slice(finalSourcesOffset) : '';
const dockerJob = extractWorkflowJob(workflow, 'build-docker');

assert(cacheBuildOffset >= 0, 'Dockerfile declares a dependency-cache build boundary');
assert(finalSourcesOffset > cacheBuildOffset, 'Dockerfile copies real sources after the cache build');
assert(
    dockerfile.includes('RUN cargo build --release --locked --jobs "${CARGO_BUILD_JOBS}"'),
    'Dockerfile cache build is locked and serialized',
);
assert(!dockerfile.includes('2>/dev/null'), 'Dockerfile does not hide Cargo diagnostics');
assert(!dockerfile.includes('|| true'), 'Dockerfile does not suppress Cargo build failures');
assert(dockerJob.length > 0, 'CI defines the Docker Build job');
assert(
    dockerJob.includes("if: github.event_name == 'pull_request' || github.ref == 'refs/heads/main'"),
    'CI runs Docker Build on pull requests and main pushes',
);

const localPackages = collectLocalPackages(rootManifest);
assert(localPackages.length > 0, 'local Cargo package closure is non-empty');

for (const packageDirectory of localPackages) {
    const manifestCopy = `COPY ${packageDirectory}/Cargo.toml ${packageDirectory}/Cargo.toml`;
    const sourceCopy = `COPY ${packageDirectory}/ ${packageDirectory}/`;
    const fullSourceAvailableBeforeCache = preCache.includes(sourceCopy);

    assert(
        preCache.includes(manifestCopy) || fullSourceAvailableBeforeCache,
        `${packageDirectory} manifest is available to the cache build`,
    );
    assert(
        fullSourceAvailableBeforeCache || postCache.includes(sourceCopy),
        `${packageDirectory} real source is available to the final build`,
    );

    for (const targetPath of defaultTargetPaths(packageDirectory)) {
        assert(
            fullSourceAvailableBeforeCache || preCache.includes(targetPath),
            `${targetPath} is available to the strict cache build`,
        );
    }
}

console.log(`\nDocker workspace coverage QA: ${passed} passed, ${failed} failed`);
if (failed > 0) {
    process.exit(1);
}
