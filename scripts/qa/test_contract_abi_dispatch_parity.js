#!/usr/bin/env node
'use strict';

const fs = require('fs');
const path = require('path');

const root = path.join(__dirname, '..', '..');
const contractsRoot = path.join(root, 'contracts');
let passed = 0;
let failed = 0;

function check(condition, label) {
    if (condition) {
        passed++;
        console.log(`PASS ${label}`);
    } else {
        failed++;
        console.error(`FAIL ${label}`);
    }
}

function functionBody(source, marker) {
    const start = source.indexOf(marker);
    if (start < 0) return '';
    const open = source.indexOf('{', start);
    if (open < 0) return '';
    let depth = 0;
    for (let index = open; index < source.length; index++) {
        if (source[index] === '{') depth++;
        if (source[index] === '}' && --depth === 0) return source.slice(open + 1, index);
    }
    return '';
}

function opcodeCases(body) {
    const values = new Set();
    for (const match of body.matchAll(/^\s*([0-9\s|]+)\s*=>/gm)) {
        for (const value of match[1].match(/\d+/g) || []) values.add(Number(value));
    }
    return values;
}

console.log('\nContract ABI / WASM Dispatch Parity');

for (const contract of fs.readdirSync(contractsRoot).sort()) {
    const abiPath = path.join(contractsRoot, contract, 'abi.json');
    const sourcePath = path.join(contractsRoot, contract, 'src', 'lib.rs');
    if (!fs.existsSync(abiPath) || !fs.existsSync(sourcePath)) continue;

    let abi;
    try {
        abi = JSON.parse(fs.readFileSync(abiPath, 'utf8'));
    } catch (error) {
        check(false, `${contract} ABI parses: ${error.message}`);
        continue;
    }
    const functions = Array.isArray(abi.functions) ? abi.functions : [];
    if (!functions.length) continue;
    const opcodeFunctions = functions.filter((entry) => Number.isInteger(entry.opcode));

    const names = functions.map((entry) => entry.name).filter((name) => typeof name === 'string' && name.length > 0);
    const opcodes = opcodeFunctions.map((entry) => entry.opcode);
    check(new Set(names).size === names.length, `${contract} has unique ABI function names`);
    check(new Set(opcodes).size === opcodes.length, `${contract} has unique ABI opcodes`);

    const source = fs.readFileSync(sourcePath, 'utf8');
    const callMarker = 'pub extern "C" fn call()';
    const callBody = functionBody(source, callMarker);
    if (callBody) {
        const dispatched = opcodeCases(callBody);
        const missingDispatch = opcodes.filter((opcode) => !dispatched.has(opcode));
        check(
            missingDispatch.length === 0,
            `${contract} call dispatcher reaches every ABI opcode${missingDispatch.length ? ` (missing ${missingDispatch.join(', ')})` : ''}`
        );

        const minLenBody = functionBody(source, 'fn dispatch_min_len(');
        if (minLenBody) {
            const accepted = opcodeCases(minLenBody);
            const missingLength = opcodes.filter((opcode) => !accepted.has(opcode));
            check(
                missingLength.length === 0,
                `${contract} dispatch length gate accepts every ABI opcode${missingLength.length ? ` (missing ${missingLength.join(', ')})` : ''}`
            );
        }
    } else {
        const sourceExports = new Set(
            [...source.matchAll(/#\[no_mangle\]\s*pub\s+extern\s+"C"\s+fn\s+(\w+)\s*\(/g)]
                .map((match) => match[1]),
        );
        const missingExports = names.filter((name) => {
            const escaped = name.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
            const exportPattern = new RegExp(`#\\[no_mangle\\][\\s\\S]{0,400}?pub\\s+extern\\s+"C"\\s+fn\\s+${escaped}\\s*\\(`);
            return !exportPattern.test(source);
        });
        check(
            missingExports.length === 0,
            `${contract} named WASM exports cover every ABI function${missingExports.length ? ` (missing ${missingExports.join(', ')})` : ''}`
        );
        const unlistedExports = [...sourceExports].filter((name) => !names.includes(name));
        check(
            unlistedExports.length === 0,
            `${contract} ABI covers every named WASM export${unlistedExports.length ? ` (missing ${unlistedExports.join(', ')})` : ''}`,
        );
    }

    const wasmPath = path.join(contractsRoot, contract, `${contract}.wasm`);
    if (fs.existsSync(wasmPath)) {
        try {
            const module = new WebAssembly.Module(fs.readFileSync(wasmPath));
            const binaryExports = new Set(
                WebAssembly.Module.exports(module)
                    .filter((entry) => entry.kind === 'function')
                    .map((entry) => entry.name),
            );
            const requiredExports = callBody ? ['call'] : names;
            const missingBinaryExports = requiredExports.filter((name) => !binaryExports.has(name));
            check(
                missingBinaryExports.length === 0,
                `${contract} built WASM exports its ABI entrypoints${missingBinaryExports.length ? ` (missing ${missingBinaryExports.join(', ')})` : ''}`,
            );
        } catch (error) {
            check(false, `${contract} built WASM validates: ${error.message}`);
        }
    }
}

if (failed) {
    console.error(`\nContract ABI / WASM dispatch parity: ${passed} passed, ${failed} failed`);
    process.exit(1);
}
console.log(`\nContract ABI / WASM dispatch parity: ${passed} passed, 0 failed`);
