#!/usr/bin/env node
'use strict';

const fs = require('fs');
const path = require('path');

const repoRoot = path.join(__dirname, '..', '..');
const allowedRuntimePaths = new Set([
    'core/src/codec.rs',
]);

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

function walk(dir, results = []) {
    for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
        if (entry.name === 'target' || entry.name === '.git') {
            continue;
        }
        const fullPath = path.join(dir, entry.name);
        if (entry.isDirectory()) {
            walk(fullPath, results);
        } else if (entry.isFile() && entry.name.endsWith('.rs')) {
            results.push(fullPath);
        }
    }
    return results;
}

const offenders = [];
for (const filePath of walk(repoRoot)) {
    const relativePath = path.relative(repoRoot, filePath);
    if (allowedRuntimePaths.has(relativePath)) {
        continue;
    }
    const source = fs.readFileSync(filePath, 'utf8');
    const lines = source.split(/\r?\n/);
    lines.forEach((line, index) => {
        if (/\bbincode::/.test(line)) {
            offenders.push(`${relativePath}:${index + 1}: ${line.trim()}`);
        }
    });
}

assert(
    offenders.length === 0,
    'Rust code uses the central legacy bincode codec instead of direct bincode calls',
);

for (const offender of offenders) {
    console.log(`    ${offender}`);
}

console.log(`\nLegacy bincode codec policy QA: ${passed} passed, ${failed} failed`);
if (failed > 0) {
    process.exit(1);
}
