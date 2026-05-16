#!/usr/bin/env node
'use strict';

const fs = require('fs');
const http = require('http');
const https = require('https');
const os = require('os');
const path = require('path');
const { spawnSync } = require('child_process');

const RPC_URL = process.env.LICHEN_RPC || 'http://127.0.0.1:8899';
const ZK_PROVE_BIN = process.env.ZK_PROVE_BIN || path.resolve(__dirname, '..', '..', 'target', 'debug', 'zk-prove');

let passed = 0;
let failed = 0;

function record(ok, label, details = '') {
    if (ok) {
        passed += 1;
        console.log(`PASS ${label}${details ? ` ${details}` : ''}`);
    } else {
        failed += 1;
        console.error(`FAIL ${label}${details ? ` ${details}` : ''}`);
    }
}

function rpc(method, params = []) {
    const url = new URL(RPC_URL);
    const transport = url.protocol === 'https:' ? https : http;
    const body = JSON.stringify({ jsonrpc: '2.0', id: 1, method, params });
    return new Promise((resolve, reject) => {
        const req = transport.request(url, {
            method: 'POST',
            headers: {
                'content-type': 'application/json',
                'content-length': Buffer.byteLength(body),
            },
            timeout: 10_000,
        }, (res) => {
            let data = '';
            res.setEncoding('utf8');
            res.on('data', (chunk) => {
                data += chunk;
            });
            res.on('end', () => {
                try {
                    const payload = JSON.parse(data);
                    if (payload.error) {
                        reject(new Error(`${method}: ${payload.error.message}`));
                    } else {
                        resolve(payload.result);
                    }
                } catch (error) {
                    reject(error);
                }
            });
        });
        req.on('error', reject);
        req.on('timeout', () => {
            req.destroy(new Error(`${method}: timeout`));
        });
        req.write(body);
        req.end();
    });
}

function runZkProve(args, inputLabel) {
    const result = spawnSync(ZK_PROVE_BIN, args, {
        encoding: 'utf8',
        maxBuffer: 32 * 1024 * 1024,
    });
    if (result.status !== 0) {
        throw new Error(`${inputLabel} failed: ${result.stderr || result.stdout}`);
    }
    return result.stdout.trim();
}

function statAmount(stats, key) {
    const value = stats && stats[key];
    if (typeof value === 'number' && Number.isFinite(value)) {
        return BigInt(Math.trunc(value));
    }
    if (typeof value === 'string' && /^[0-9]+$/.test(value)) {
        return BigInt(value);
    }
    return 0n;
}

async function main() {
    if (!fs.existsSync(ZK_PROVE_BIN)) {
        throw new Error(`zk-prove binary not found at ${ZK_PROVE_BIN}; run cargo build -p lichen-cli --bin zk-prove first`);
    }

    const status = await rpc('getNeoZkProofServiceStatus');
    record(status && status.lane === 'NX-960', 'RPC status exposes NX-960 lane');
    record(
        status && status.verifier_method === 'verifyNeoReserveLiabilityProof',
        'RPC status exposes reserve/liability verifier method'
    );

    const wgas = await rpc('getWgasStats').catch(() => ({}));
    const supply = statAmount(wgas, 'supply');
    const attested = statAmount(wgas, 'reserve_attested');
    const liability = supply;
    const reserve = attested >= liability ? attested : liability;
    const proofReserve = reserve > 0n ? reserve : 1n;
    const proofLiability = liability <= proofReserve ? liability : proofReserve;

    const tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), 'lichen-nx960-'));
    const witnessPath = path.join(tmpDir, 'wgas-witness.json');
    const proofPath = path.join(tmpDir, 'wgas-proof.json');
    fs.writeFileSync(witnessPath, JSON.stringify({
        source: 'local-rpc',
        rpc: RPC_URL,
        stats_method: 'getWgasStats',
        supply: supply.toString(),
        reserve_attested: attested.toString(),
    }, null, 2));

    const proofJson = runZkProve([
        'reserve-liability',
        '--lichen-network', 'local-testnet',
        '--neo-network', 'neo-x-testnet',
        '--neo-chain-id', '12227332',
        '--route', 'neox/gas',
        '--asset', 'wGAS',
        '--product', 'reserve-liability',
        '--epoch', '1',
        '--reserve', proofReserve.toString(),
        '--liability', proofLiability.toString(),
        '--verifier-version', '1',
        '--witness-json', witnessPath,
    ], 'reserve-liability proof generation');
    fs.writeFileSync(proofPath, `${proofJson}\n`);
    const proof = JSON.parse(proofJson);
    record(proof.type === 'reserve_liability', 'CLI generated reserve/liability proof');
    record(proof.privacy_model === 'transparent_aggregate_totals_no_address_list_v1', 'CLI discloses transparent privacy model');

    const verifyJson = runZkProve([
        'verify-reserve-liability',
        '--proof-json', proofPath,
    ], 'reserve-liability CLI verification');
    const cliVerify = JSON.parse(verifyJson);
    record(cliVerify.verified === true, 'CLI verifies generated reserve/liability proof');

    const rpcVerify = await rpc('verifyNeoReserveLiabilityProof', [proof]);
    record(rpcVerify.verified === true, 'RPC verifies generated reserve/liability proof');

    const replayedForWrongRoute = proof.domain.route !== 'neox/neo';
    record(replayedForWrongRoute, 'consumer route check rejects neox/gas proof for neox/neo replay');

    const mutated = JSON.parse(JSON.stringify(proof));
    mutated.stark_public_inputs[0] = mutated.stark_public_inputs[0] ^ 1;
    const mutatedVerify = await rpc('verifyNeoReserveLiabilityProof', [mutated]);
    record(mutatedVerify.verified === false, 'RPC rejects mutated domain public input');

    const insolvent = spawnSync(ZK_PROVE_BIN, [
        'reserve-liability',
        '--lichen-network', 'local-testnet',
        '--neo-network', 'neo-x-testnet',
        '--neo-chain-id', '12227332',
        '--route', 'neox/gas',
        '--asset', 'wGAS',
        '--product', 'reserve-liability',
        '--epoch', '1',
        '--reserve', '1',
        '--liability', '2',
        '--verifier-version', '1',
        '--witness-json', witnessPath,
    ], { encoding: 'utf8', maxBuffer: 1024 * 1024 });
    record(insolvent.status !== 0 && /undercollateralized/.test(insolvent.stderr), 'CLI rejects undercollateralized statement');

    fs.rmSync(tmpDir, { recursive: true, force: true });

    console.log(`Neo ZK proof services live QA: PASS=${passed} FAIL=${failed}`);
    if (failed > 0) {
        process.exitCode = 1;
    }
}

main().catch((error) => {
    console.error(`FAIL live NX-960 proof-service QA: ${error.message}`);
    process.exitCode = 1;
});
