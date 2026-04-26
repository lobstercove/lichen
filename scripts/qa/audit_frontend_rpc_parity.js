#!/usr/bin/env node
'use strict';

const { execFileSync } = require('child_process');
const fs = require('fs');
const path = require('path');

const repoRoot = path.join(__dirname, '..', '..');

const frontendRoots = new Set([
    'wallet',
    'explorer',
    'dex',
    'marketplace',
    'developers',
    'programs',
    'monitoring',
    'faucet',
    'website',
]);

const excludedPrefixes = [
    'dex/charting_library/',
    'dex/loadtest/',
    'dex/market-maker/',
    'dex/sdk/dist/',
    'dex/sdk/node_modules/',
];

const ignoredMethods = new Set([
    'GET',
    'POST',
    'PUT',
    'PATCH',
    'DELETE',
    'OPTIONS',
    'HEAD',
    'eth_requestAccounts',
    'wallet_switchEthereumChain',
    'wallet_addEthereumChain',
]);

function read(relativePath) {
    return fs.readFileSync(path.join(repoRoot, relativePath), 'utf8');
}

function extractBalancedBlock(source, openBraceIndex) {
    let depth = 0;
    let inString = null;
    let escaped = false;

    for (let i = openBraceIndex; i < source.length; i++) {
        const ch = source[i];

        if (inString) {
            if (escaped) {
                escaped = false;
            } else if (ch === '\\') {
                escaped = true;
            } else if (ch === inString) {
                inString = null;
            }
            continue;
        }

        if (ch === '"' || ch === "'") {
            inString = ch;
            continue;
        }

        if (ch === '{') {
            depth++;
        } else if (ch === '}') {
            depth--;
            if (depth === 0) {
                return source.slice(openBraceIndex, i + 1);
            }
        }
    }

    throw new Error(`Could not extract balanced block at byte ${openBraceIndex}`);
}

function extractResultMatchBlocks(source) {
    const marker = 'let result = match req.method.as_str() {';
    const blocks = [];
    let offset = 0;

    while (offset < source.length) {
        const markerIndex = source.indexOf(marker, offset);
        if (markerIndex === -1) {
            break;
        }
        const openBraceIndex = source.indexOf('{', markerIndex);
        blocks.push(extractBalancedBlock(source, openBraceIndex));
        offset = openBraceIndex + 1;
    }

    return blocks;
}

function extractMethodsFromMatchBlock(block) {
    const methods = new Set();
    const methodPattern = /"([A-Za-z_][A-Za-z0-9_]*)"\s*(?=\||=>)/g;
    let match;
    while ((match = methodPattern.exec(block)) !== null) {
        methods.add(match[1]);
    }
    return methods;
}

function collectServerMethods() {
    const libBlocks = extractResultMatchBlocks(read('rpc/src/lib.rs'));
    if (libBlocks.length < 3) {
        throw new Error(`Expected at least 3 RPC result match blocks in rpc/src/lib.rs, found ${libBlocks.length}`);
    }

    const wsBlocks = extractResultMatchBlocks(read('rpc/src/ws.rs'));
    if (wsBlocks.length < 1) {
        throw new Error('Expected at least 1 WebSocket match block in rpc/src/ws.rs');
    }

    return {
        native: extractMethodsFromMatchBlock(libBlocks[0]),
        solana: extractMethodsFromMatchBlock(libBlocks[1]),
        evm: extractMethodsFromMatchBlock(libBlocks[2]),
        websocket: extractMethodsFromMatchBlock(wsBlocks[0]),
    };
}

function trackedFrontendFiles() {
    const output = execFileSync('git', ['ls-files'], {
        cwd: repoRoot,
        encoding: 'utf8',
    });

    return output
        .split('\n')
        .filter(Boolean)
        .filter((relativePath) => {
            if (!/\.(?:js|mjs)$/.test(relativePath)) {
                return false;
            }
            const root = relativePath.split('/')[0];
            if (!frontendRoots.has(root)) {
                return false;
            }
            if (relativePath.includes('/node_modules/')) {
                return false;
            }
            return !excludedPrefixes.some((prefix) => relativePath.startsWith(prefix));
        })
        .sort();
}

function lineNumberAt(source, index) {
    let line = 1;
    for (let i = 0; i < index; i++) {
        if (source.charCodeAt(i) === 10) {
            line++;
        }
    }
    return line;
}

function hasJsonRpcContext(source, index) {
    const context = source.slice(Math.max(0, index - 240), Math.min(source.length, index + 240));
    return /jsonrpc|JSON\.stringify/i.test(context);
}

function addCall(calls, seen, file, source, index, method, kind) {
    const key = `${file}:${index}:${method}:${kind}`;
    if (seen.has(key)) {
        return;
    }
    seen.add(key);
    calls.push({
        file,
        line: lineNumberAt(source, index),
        method,
        kind,
    });
}

function isLikelyFunctionOrMethodDefinition(source, index) {
    const lineStart = source.lastIndexOf('\n', index) + 1;
    const beforeOnLine = source.slice(lineStart, index);
    return /(?:^|\s)(?:async\s+)?function\s+$/.test(beforeOnLine)
        || /^\s*(?:async\s+)?$/.test(beforeOnLine);
}

function shouldSkipDynamicCall(pattern, source, match, variable) {
    if (pattern.kind !== 'dynamic-rpc-helper') {
        return false;
    }

    if (!/method/i.test(variable)) {
        return true;
    }

    const nameIndex = match.index + (pattern.nameOffsetGroup ? match[pattern.nameOffsetGroup].length : 0);
    return isLikelyFunctionOrMethodDefinition(source, nameIndex);
}

function collectFrontendCalls(files) {
    const calls = [];
    const dynamicCalls = [];
    const seen = new Set();
    const dynamicSeen = new Set();
    const literalPatterns = [
        { kind: 'rpc-call', re: /\b(?:this\.)?rpc\.call\(\s*(['"])([A-Za-z_][A-Za-z0-9_]*)\1/g },
        { kind: 'rpc-client-call', re: /\brpcClient\.call\(\s*(['"])([A-Za-z_][A-Za-z0-9_]*)\1/g },
        { kind: 'rpc-factory-call', re: /\brpc\(\)\.call\(\s*(['"])([A-Za-z_][A-Za-z0-9_]*)\1/g },
        { kind: 'rpc-helper', re: /(^|[^.\w$])rpc\(\s*(['"])([A-Za-z_][A-Za-z0-9_]*)\2/g, methodGroup: 3 },
        { kind: 'solana-compat-helper', re: /\bsolanaCompatRpc\(\s*(['"])([A-Za-z_][A-Za-z0-9_]*)\1/g },
        { kind: 'named-rpc-helper', re: /\b(?:trustedLichenRpcCall|lichenRpcCall|rpcCall|callRpc)\(\s*(['"])([A-Za-z_][A-Za-z0-9_]*)\1/g },
        { kind: 'json-rpc-body', re: /\bmethod\s*:\s*(['"])([A-Za-z_][A-Za-z0-9_]*)\1/g, requireJsonRpcContext: true },
    ];

    const dynamicPatterns = [
        { kind: 'dynamic-rpc-call', re: /\b(?:this\.)?rpc\.call\(\s*([A-Za-z_$][A-Za-z0-9_$]*)\s*(?:,|\))/g },
        { kind: 'dynamic-rpc-client-call', re: /\brpcClient\.call\(\s*([A-Za-z_$][A-Za-z0-9_$]*)\s*(?:,|\))/g },
        { kind: 'dynamic-rpc-factory-call', re: /\brpc\(\)\.call\(\s*([A-Za-z_$][A-Za-z0-9_$]*)\s*(?:,|\))/g },
        { kind: 'dynamic-rpc-helper', re: /(^|[^.\w$])rpc\(\s*([A-Za-z_$][A-Za-z0-9_$]*)\s*(?:,|\))/g, variableGroup: 2, nameOffsetGroup: 1 },
        { kind: 'dynamic-json-rpc-body', re: /\bmethod\s*:\s*([A-Za-z_$][A-Za-z0-9_$]*)/g, requireJsonRpcContext: true },
    ];

    for (const file of files) {
        const source = read(file);

        for (const pattern of literalPatterns) {
            pattern.re.lastIndex = 0;
            let match;
            while ((match = pattern.re.exec(source)) !== null) {
                if (pattern.requireJsonRpcContext && !hasJsonRpcContext(source, match.index)) {
                    continue;
                }
                const method = match[pattern.methodGroup || 2];
                addCall(calls, seen, file, source, match.index, method, pattern.kind);
            }
        }

        for (const pattern of dynamicPatterns) {
            pattern.re.lastIndex = 0;
            let match;
            while ((match = pattern.re.exec(source)) !== null) {
                if (pattern.requireJsonRpcContext && !hasJsonRpcContext(source, match.index)) {
                    continue;
                }
                const variable = match[pattern.variableGroup || 1];
                if (shouldSkipDynamicCall(pattern, source, match, variable)) {
                    continue;
                }
                const key = `${file}:${match.index}:${variable}:${pattern.kind}`;
                if (dynamicSeen.has(key)) {
                    continue;
                }
                dynamicSeen.add(key);
                dynamicCalls.push({
                    file,
                    line: lineNumberAt(source, match.index),
                    variable,
                    kind: pattern.kind,
                });
            }
        }
    }

    return { calls, dynamicCalls };
}

function classifyCall(call, serverMethods) {
    const method = call.method;

    if (ignoredMethods.has(method) || method.startsWith('licn_') || method.startsWith('wallet_')) {
        return { status: 'ignored', layer: 'provider-or-http' };
    }

    if (method === 'ping') {
        return { status: 'supported', layer: 'websocket-keepalive' };
    }

    if (call.kind === 'solana-compat-helper') {
        return serverMethods.solana.has(method)
            ? { status: 'supported', layer: 'solana' }
            : { status: 'unknown', layer: 'solana' };
    }

    if (serverMethods.native.has(method)) {
        return { status: 'supported', layer: 'native' };
    }
    if (serverMethods.evm.has(method)) {
        return { status: 'supported', layer: 'evm' };
    }
    if (serverMethods.solana.has(method)) {
        return { status: 'supported', layer: 'solana' };
    }
    if (serverMethods.websocket.has(method)) {
        return { status: 'supported', layer: 'websocket' };
    }

    return { status: 'unknown', layer: 'none' };
}

function groupByMethod(calls) {
    const groups = new Map();
    for (const call of calls) {
        if (!groups.has(call.method)) {
            groups.set(call.method, []);
        }
        groups.get(call.method).push(call);
    }
    return Array.from(groups.entries()).sort(([a], [b]) => a.localeCompare(b));
}

function formatCall(call) {
    return `${call.file}:${call.line} (${call.kind})`;
}

function main() {
    const serverMethods = collectServerMethods();
    const files = trackedFrontendFiles();
    const { calls, dynamicCalls } = collectFrontendCalls(files);

    const supported = [];
    const ignored = [];
    const unknown = [];

    for (const call of calls) {
        const classification = classifyCall(call, serverMethods);
        call.layer = classification.layer;
        if (classification.status === 'supported') {
            supported.push(call);
        } else if (classification.status === 'ignored') {
            ignored.push(call);
        } else {
            unknown.push(call);
        }
    }

    console.log('Lichen frontend RPC parity audit');
    console.log('================================');
    console.log(`Server native methods: ${serverMethods.native.size}`);
    console.log(`Server Solana-compat methods: ${serverMethods.solana.size}`);
    console.log(`Server EVM methods: ${serverMethods.evm.size}`);
    console.log(`Server WebSocket methods: ${serverMethods.websocket.size}`);
    console.log(`Frontend files scanned: ${files.length}`);
    console.log(`Literal frontend RPC calls: ${calls.length}`);
    console.log(`Supported calls: ${supported.length}`);
    console.log(`Ignored provider/HTTP calls: ${ignored.length}`);
    console.log(`Dynamic/manual calls: ${dynamicCalls.length}`);
    console.log(`Unknown live RPC calls: ${unknown.length}`);

    if (dynamicCalls.length > 0) {
        console.log('\nDynamic/manual RPC calls, review by feature owner:');
        for (const call of dynamicCalls) {
            console.log(`  - ${call.file}:${call.line} (${call.kind}, variable: ${call.variable})`);
        }
    }

    if (unknown.length > 0) {
        console.log('\nUnknown live frontend RPC calls:');
        for (const [method, methodCalls] of groupByMethod(unknown)) {
            console.log(`  - ${method}`);
            for (const call of methodCalls) {
                console.log(`      ${formatCall(call)}`);
            }
        }
        process.exitCode = 1;
    }
}

main();
