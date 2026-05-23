#!/usr/bin/env node
'use strict';

const fs = require('fs');
const path = require('path');
const { spawnSync } = require('child_process');

const repoRoot = path.join(__dirname, '..', '..');
const manifestPath = path.join(repoRoot, 'security', 'release-audit-summaries.json');
const idPattern = /^(AUD|E2E)-\d{3}$/;
const allowedStatuses = new Set(['tracked-gate', 'tracked-live-gate']);
const forbiddenPathPrefixes = [
    'data/',
    'docs/',
    'infra/',
    'internal-docs/',
    'logs/',
    'target/',
    'tests/',
];
const secretPattern = /(?:BEGIN [A-Z ]*PRIVATE KEY|LICHEN_KEYPAIR_PASSWORD\s*=|CUSTODY_[A-Z0-9_]*TOKEN\s*=|AKIA[0-9A-Z]{16}|-----BEGIN)/;

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

function readJson(filePath) {
    return JSON.parse(fs.readFileSync(filePath, 'utf8'));
}

function sorted(values) {
    return Array.from(values).sort();
}

function sameList(left, right) {
    return JSON.stringify(left) === JSON.stringify(right);
}

function repoPathExists(relativePath) {
    return fs.existsSync(path.join(repoRoot, relativePath));
}

function isIgnored(relativePath) {
    const result = spawnSync('git', ['check-ignore', '-q', '--', relativePath], {
        cwd: repoRoot,
        stdio: 'ignore',
    });
    return result.status === 0;
}

function usesPrivateOrGeneratedPath(relativePath) {
    return forbiddenPathPrefixes.some((prefix) => relativePath === prefix.slice(0, -1) || relativePath.startsWith(prefix));
}

function commandScriptPaths(command) {
    return Array.from(command.matchAll(/(?:^|\s)(scripts\/qa\/[A-Za-z0-9_.\/-]+)(?=\s|$)/g), (match) => match[1]);
}

const rawManifest = fs.readFileSync(manifestPath, 'utf8');
const manifest = readJson(manifestPath);
const summaries = Array.isArray(manifest.summaries) ? manifest.summaries : [];
const ids = summaries.map((summary) => summary.id);

assert(manifest.schema_version === 1, 'release audit summary schema version is current');
assert(typeof manifest.policy === 'string' && manifest.policy.length >= 80, 'manifest records a public artifact policy');
assert(Array.isArray(manifest.generated_artifact_policy) && manifest.generated_artifact_policy.length > 0, 'manifest lists generated artifact exclusions');
assert(summaries.length >= 5, 'manifest contains release audit and E2E summaries');
assert(new Set(ids).size === ids.length, 'summary IDs are unique');
assert(sameList(ids, sorted(ids)), 'summary IDs are sorted for review');
assert(!secretPattern.test(rawManifest), 'manifest does not contain obvious secret material');

for (const summary of summaries) {
    const id = summary.id || '<missing id>';
    assert(idPattern.test(id), `${id} has a stable summary ID`);
    assert(typeof summary.area === 'string' && summary.area.length >= 8, `${id} records an audit area`);
    assert(allowedStatuses.has(summary.status), `${id} has an allowed tracked status`);
    assert(typeof summary.command === 'string' && summary.command.length >= 12, `${id} records a runnable command`);
    assert(typeof summary.expected_evidence === 'string' && summary.expected_evidence.length >= 80, `${id} records expected evidence`);
    assert(typeof summary.artifact_policy === 'string' && summary.artifact_policy.length >= 50, `${id} records artifact handling`);
    assert(Array.isArray(summary.tracked_sources) && summary.tracked_sources.length > 0, `${id} records tracked source paths`);

    for (const source of summary.tracked_sources || []) {
        assert(typeof source === 'string' && source.length > 0, `${id} source path is non-empty`);
        assert(!path.isAbsolute(source), `${id} source path is repository-relative`);
        assert(!usesPrivateOrGeneratedPath(source), `${id} source path avoids private/generated areas: ${source}`);
        assert(repoPathExists(source), `${id} source path exists: ${source}`);
        assert(!isIgnored(source), `${id} source path is not ignored: ${source}`);
    }

    const scripts = commandScriptPaths(summary.command);
    assert(scripts.length > 0, `${id} command references a scripts/qa gate`);
    for (const script of scripts) {
        assert(repoPathExists(script), `${id} command script exists: ${script}`);
        assert(!isIgnored(script), `${id} command script is not ignored: ${script}`);
    }
}

console.log(`\nRelease audit summary manifest QA: ${passed} passed, ${failed} failed`);
if (failed > 0) {
    process.exit(1);
}
