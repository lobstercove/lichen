#!/usr/bin/env node
'use strict';

const fs = require('fs');
const http = require('http');
const https = require('https');
const os = require('os');
const path = require('path');
const { spawnSync } = require('child_process');
const { stableJsonStringify } = require('../monitoring/shared/utils.js');

const REPO_ROOT = path.resolve(__dirname, '..');

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

function signPayloadWithRust(payloadBytes, keypairPath) {
    const tempDir = fs.mkdtempSync(path.join(os.tmpdir(), 'signed-metadata-signer-'));
    const cargoTomlPath = path.join(tempDir, 'Cargo.toml');
    const srcDir = path.join(tempDir, 'src');
    const mainRsPath = path.join(srcDir, 'main.rs');

    fs.mkdirSync(srcDir, { recursive: true });
    fs.writeFileSync(cargoTomlPath, `[package]\nname = \"signed-metadata-signer\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nserde = { version = \"1.0\", features = [\"derive\"] }\nserde_json = \"1.0\"\nlichen-core = { path = \"${REPO_ROOT.replace(/\\/g, '\\\\')}\/core\" }\n`);
    fs.writeFileSync(mainRsPath, `use lichen_core::account::Keypair;\nuse serde::{Deserialize, Serialize};\nuse std::{env, fs};\n\n#[derive(Deserialize)]\nstruct KeypairFile {\n    #[serde(rename = \"privateKey\")]\n    private_key: Vec<u8>,\n}\n\n#[derive(Serialize)]\nstruct SignedPayload {\n    signer: String,\n    signature: lichen_core::PqSignature,\n}\n\nfn main() {\n    let args: Vec<String> = env::args().collect();\n    if args.len() < 3 {\n        eprintln!(\"Usage: signer <payload-file> <keypair-json>\");\n        std::process::exit(1);\n    }\n\n    let payload = fs::read(&args[1]).expect(\"Failed to read payload\");\n    let keypair_json = fs::read_to_string(&args[2]).expect(\"Failed to read keypair file\");\n    let keypair_file: KeypairFile = serde_json::from_str(&keypair_json).expect(\"Invalid keypair JSON\");\n    let seed: [u8; 32] = keypair_file.private_key.as_slice().try_into().expect(\"privateKey must be 32 bytes\");\n    let keypair = Keypair::from_seed(&seed);\n    let signature = keypair.sign(&payload);\n    let result = SignedPayload {\n        signer: keypair.pubkey().to_base58(),\n        signature,\n    };\n    println!(\"{}\", serde_json::to_string(&result).expect(\"serialize signature\"));\n}\n`);

    const payloadPath = path.join(tempDir, 'payload.json');
    fs.writeFileSync(payloadPath, payloadBytes);

    const run = spawnSync('cargo', [
        'run',
        '--quiet',
        '--manifest-path', cargoTomlPath,
        '--',
        payloadPath,
        path.resolve(keypairPath),
    ], {
        cwd: REPO_ROOT,
        encoding: 'utf8',
        env: {
            ...process.env,
            CARGO_TARGET_DIR: path.join(REPO_ROOT, 'target', 'cargo-tools'),
        },
    });

    fs.rmSync(tempDir, { recursive: true, force: true });

    if (run.status !== 0) {
        throw new Error((run.stderr || run.stdout || 'Rust signer failed').trim());
    }

    return JSON.parse(run.stdout);
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
    const signedPayload = signPayloadWithRust(payloadBytes, options.keypair);
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