#!/usr/bin/env node
'use strict';

const fs = require('fs');
const http = require('http');
const https = require('https');
const path = require('path');
const { stableJsonStringify } = require('../monitoring/shared/utils.js');

const REPO_ROOT = path.resolve(__dirname, '..');
const PQ_MODULE_PATH = path.join(REPO_ROOT, 'monitoring', 'shared', 'pq.js');

let pqModulePromise = null;

function loadPqModule() {
    if (!pqModulePromise) {
        const pqModuleSource = fs.readFileSync(PQ_MODULE_PATH, 'utf8');
        const pqModuleUrl = `data:text/javascript;base64,${Buffer.from(pqModuleSource, 'utf8').toString('base64')}`;
        pqModulePromise = import(pqModuleUrl);
    }
    return pqModulePromise;
}

function usage() {
    console.error('Usage: node scripts/generate-signed-metadata-manifest.js --rpc <url> --network <name> --keypair <keypair.json> --out <manifest.json> [--page-limit <n>]');
}

function parseArgs(argv) {
    const options = {
        rpc: '',
        network: '',
        keypair: '',
        out: '',
        pageLimit: 500,
    };

    for (let i = 0; i < argv.length; i++) {
        const arg = argv[i];
        if (arg === '--rpc' && argv[i + 1]) {
            options.rpc = argv[++i];
        } else if (arg === '--network' && argv[i + 1]) {
            options.network = argv[++i];
        } else if (arg === '--keypair' && argv[i + 1]) {
            options.keypair = argv[++i];
        } else if (arg === '--out' && argv[i + 1]) {
            options.out = argv[++i];
        } else if (arg === '--page-limit' && argv[i + 1]) {
            options.pageLimit = Math.max(1, Math.min(2000, Number(argv[++i]) || 500));
        } else {
            throw new Error(`Unknown or incomplete argument: ${arg}`);
        }
    }

    if (!options.rpc || !options.network || !options.keypair || !options.out) {
        throw new Error('Missing required arguments');
    }

    return options;
}

function rpcCall(rpcUrl, method, params) {
    const url = new URL(rpcUrl);
    const transport = url.protocol === 'https:' ? https : http;
    const payload = JSON.stringify({
        jsonrpc: '2.0',
        id: Date.now(),
        method,
        params: params || [],
    });

    return new Promise((resolve, reject) => {
        const req = transport.request({
            protocol: url.protocol,
            hostname: url.hostname,
            port: url.port,
            path: url.pathname || '/',
            method: 'POST',
            headers: {
                'Content-Type': 'application/json',
                'Content-Length': Buffer.byteLength(payload),
            },
        }, (res) => {
            let body = '';
            res.setEncoding('utf8');
            res.on('data', (chunk) => {
                body += chunk;
            });
            res.on('end', () => {
                if (res.statusCode < 200 || res.statusCode >= 300) {
                    reject(new Error(`${method} HTTP ${res.statusCode}: ${body}`));
                    return;
                }
                try {
                    const json = JSON.parse(body);
                    if (json.error) {
                        reject(new Error(`${method} RPC error: ${json.error.message || JSON.stringify(json.error)}`));
                        return;
                    }
                    resolve(json.result);
                } catch (error) {
                    reject(new Error(`${method} invalid JSON response: ${error.message}`));
                }
            });
        });

        req.on('error', reject);
        req.write(payload);
        req.end();
    });
}

async function fetchAllSymbolRegistry(rpcUrl, pageLimit) {
    const entries = [];
    let cursor = null;

    while (true) {
        const request = cursor
            ? [{ limit: pageLimit, cursor }]
            : [{ limit: pageLimit }];
        const result = await rpcCall(rpcUrl, 'getAllSymbolRegistry', request);
        const batch = Array.isArray(result && result.entries) ? result.entries : [];
        entries.push.apply(entries, batch);

        if (!result || !result.has_more || !result.next_cursor) {
            break;
        }
        cursor = result.next_cursor;
    }

    const deduped = new Map();
    entries.forEach((entry) => {
        if (!entry || !entry.symbol || !entry.program) return;
        deduped.set(String(entry.symbol).toUpperCase(), {
            symbol: entry.symbol,
            program: entry.program,
            owner: entry.owner || null,
            name: entry.name || null,
            template: entry.template || null,
            metadata: entry.metadata && typeof entry.metadata === 'object' ? entry.metadata : null,
            decimals: entry.decimals == null ? null : entry.decimals,
        });
    });

    return Array.from(deduped.values()).sort((left, right) => String(left.symbol).localeCompare(String(right.symbol)));
}

async function signPayload(payloadBytes, keypairPath) {
    const { publicKeyToAddress, signMessage } = await loadPqModule();
    const keypairJson = JSON.parse(fs.readFileSync(path.resolve(keypairPath), 'utf8'));
    const seedBytes = Array.isArray(keypairJson.privateKey)
        ? Uint8Array.from(keypairJson.privateKey)
        : null;

    if (!seedBytes || seedBytes.length !== 32) {
        throw new Error('Signing keypair must include a 32-byte privateKey seed array');
    }

    const signature = await signMessage(seedBytes, payloadBytes);
    const signer = await publicKeyToAddress(signature.public_key.bytes, signature.scheme_version);
    return { signer, signature };
}

async function main() {
    let options;
    try {
        options = parseArgs(process.argv.slice(2));
    } catch (error) {
        usage();
        console.error(error.message);
        process.exit(1);
    }

    const symbolRegistry = await fetchAllSymbolRegistry(options.rpc, options.pageLimit);
    const generatedAt = new Date().toISOString();
    const payload = {
        schema_version: 1,
        network: options.network,
        generated_at: generatedAt,
        source_rpc: options.rpc,
        symbol_registry: symbolRegistry,
    };
    const payloadBytes = Buffer.from(stableJsonStringify(payload), 'utf8');
    const signedPayload = await signPayload(payloadBytes, options.keypair);
    const envelope = {
        schema_version: 1,
        manifest_type: 'signed_metadata',
        signed_at: generatedAt,
        signer: signedPayload.signer,
        payload,
        signature: signedPayload.signature,
    };

    fs.mkdirSync(path.dirname(path.resolve(options.out)), { recursive: true });
    fs.writeFileSync(path.resolve(options.out), JSON.stringify(envelope, null, 2) + '\n');

    console.log(`Signed metadata manifest written to ${path.resolve(options.out)}`);
    console.log(`  network: ${options.network}`);
    console.log(`  signer:  ${signedPayload.signer}`);
    console.log(`  symbols: ${symbolRegistry.length}`);
}

main().catch((error) => {
    console.error(error.message || error);
    process.exit(1);
});