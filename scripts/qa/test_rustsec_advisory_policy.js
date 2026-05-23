#!/usr/bin/env node
'use strict';

const fs = require('fs');
const path = require('path');

const repoRoot = path.join(__dirname, '..', '..');
const policyPath = path.join(repoRoot, 'security', 'rustsec-advisory-exceptions.json');
const auditConfigPath = path.join(repoRoot, '.cargo', 'audit.toml');
const denyConfigPath = path.join(repoRoot, 'deny.toml');
const rustsecIdPattern = /^RUSTSEC-\d{4}-\d{4}$/;
const expiryPattern = /^\d{4}-\d{2}-\d{2}$/;
const allowedTools = new Set(['cargo-audit', 'cargo-deny']);

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

function extractAdvisoryIgnores(filePath) {
    const source = fs.readFileSync(filePath, 'utf8');
    const advisorySection = source.match(/\[advisories\]([\s\S]*?)(?:\n\[|$)/);
    if (!advisorySection) {
        return null;
    }

    const ignoreList = advisorySection[1].match(/ignore\s*=\s*\[([\s\S]*?)\]/);
    if (!ignoreList) {
        return [];
    }

    return Array.from(ignoreList[1].matchAll(/"(RUSTSEC-\d{4}-\d{4})"/g), (match) => match[1]);
}

function sorted(values) {
    return Array.from(values).sort();
}

function sameList(left, right) {
    return JSON.stringify(left) === JSON.stringify(right);
}

function isFutureDate(dateString) {
    if (!expiryPattern.test(dateString)) {
        return false;
    }
    const [year, month, day] = dateString.split('-').map((part) => Number(part));
    const parsed = new Date(Date.UTC(year, month - 1, day));
    if (
        parsed.getUTCFullYear() !== year
        || parsed.getUTCMonth() !== month - 1
        || parsed.getUTCDate() !== day
    ) {
        return false;
    }

    const today = new Date().toISOString().slice(0, 10);
    return dateString > today;
}

const policy = readJson(policyPath);
const exceptions = Array.isArray(policy.exceptions) ? policy.exceptions : [];
const policyIds = exceptions.map((entry) => entry.id);
const sortedPolicyIds = sorted(policyIds);
const auditPolicyIds = sorted(exceptions
    .filter((entry) => Array.isArray(entry.tools) && entry.tools.includes('cargo-audit'))
    .map((entry) => entry.id));
const denyPolicyIds = sorted(exceptions
    .filter((entry) => Array.isArray(entry.tools) && entry.tools.includes('cargo-deny'))
    .map((entry) => entry.id));

assert(policy.schema_version === 1, 'RustSec exception policy schema version is current');
assert(exceptions.length > 0, 'RustSec exception policy contains tracked exceptions');
assert(sameList(policyIds, sortedPolicyIds), 'RustSec exception IDs are sorted for review');
assert(new Set(policyIds).size === policyIds.length, 'RustSec exception IDs are unique');

for (const entry of exceptions) {
    const id = entry.id || '<missing id>';
    assert(rustsecIdPattern.test(entry.id || ''), `${id} has a valid RustSec advisory ID`);
    assert(typeof entry.crate === 'string' && entry.crate.length > 1, `${id} records the affected crate`);
    assert(typeof entry.owner === 'string' && entry.owner.length >= 4, `${id} records an owner`);
    assert(
        Array.isArray(entry.tools)
            && entry.tools.length > 0
            && sameList(entry.tools, sorted(entry.tools))
            && entry.tools.every((tool) => allowedTools.has(tool)),
        `${id} declares sorted supported advisory tools`,
    );
    assert(isFutureDate(entry.expires || ''), `${id} has a valid future expiry date`);
    assert(typeof entry.reason === 'string' && entry.reason.length >= 60, `${id} records a substantive reason`);
    assert(typeof entry.mitigation === 'string' && entry.mitigation.length >= 60, `${id} records a mitigation or removal path`);
    assert(!/\b(?:todo|tbd|unknown)\b/i.test(`${entry.reason} ${entry.mitigation}`), `${id} does not use placeholder rationale`);
}

const auditIgnores = extractAdvisoryIgnores(auditConfigPath);
const denyIgnores = extractAdvisoryIgnores(denyConfigPath);

assert(auditIgnores !== null, '.cargo/audit.toml declares an advisories section');
assert(denyIgnores !== null, 'deny.toml declares an advisories section');

if (auditIgnores && denyIgnores) {
    assert(sameList(sorted(auditIgnores), auditPolicyIds), '.cargo/audit.toml mirrors the central cargo-audit exception IDs');
    assert(sameList(sorted(denyIgnores), denyPolicyIds), 'deny.toml mirrors the central cargo-deny exception IDs');
    assert(sameList(auditIgnores, sorted(auditIgnores)), '.cargo/audit.toml ignore IDs are sorted');
    assert(sameList(denyIgnores, sorted(denyIgnores)), 'deny.toml ignore IDs are sorted');
}

console.log(`\nRustSec advisory policy QA: ${passed} passed, ${failed} failed`);
if (failed > 0) {
    process.exit(1);
}
