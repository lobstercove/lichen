#!/usr/bin/env node
'use strict';

const fs = require('fs');
const path = require('path');

const repoRoot = path.join(__dirname, '..', '..');
const workflowsDir = path.join(repoRoot, '.github', 'workflows');
const fullShaPattern = /^[0-9a-f]{40}$/;

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

        if (actionUse.ref.startsWith('dtolnay/rust-toolchain@')) {
            assert(
                hasExplicitStableToolchain(source, actionUse.line),
                `${relativePath}:${actionUse.line} declares the Rust stable toolchain explicitly`,
            );
        }
    }
}

console.log(`\nGitHub Actions supply-chain QA: ${passed} passed, ${failed} failed`);
if (failed > 0) {
    process.exit(1);
}
