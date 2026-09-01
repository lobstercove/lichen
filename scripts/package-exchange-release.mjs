import crypto from 'node:crypto';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(scriptDir, '..');
const args = parseArgs(process.argv.slice(2));
const runtimeVersion = read(repoRoot, 'validator/Cargo.toml').match(/^version = "([^"]+)"/m)?.[1];

if (!runtimeVersion) throw new Error('validator/Cargo.toml is missing its package version');

const expectedTag = `exchange-testnet-v${runtimeVersion}`;
const releaseTag = args.releaseTag || expectedTag;
if (releaseTag !== expectedTag) {
    throw new Error(`Runtime version ${runtimeVersion} requires exchange tag ${expectedTag}, received ${releaseTag}`);
}

const sourceCommit = execFileSync('git', ['rev-parse', 'HEAD'], {
    cwd: repoRoot,
    encoding: 'utf8',
}).trim();
const packageName = `lichen-exchange-testnet-v${runtimeVersion}`;
const distRoot = path.join(repoRoot, 'dist', 'exchange');
const archivePath = path.join(distRoot, `${packageName}.tar.gz`);
const checksumsPath = path.join(distRoot, 'SHA256SUMS');
const tempRoot = fs.mkdtempSync(path.join(os.tmpdir(), 'lichen-exchange-release-'));
const packageRoot = path.join(tempRoot, packageName);

const includedFiles = [
    'developers/exchange-integration.html',
    'docs/guides/EXCHANGE_INTEGRATION.md',
    'docs/guides/EXCHANGE_CHAIN_METADATA.md',
    'docs/guides/EXCHANGE_ADDRESS_VALIDATION_VECTORS.md',
    'docs/deployment/EXCHANGE_OPERATIONS_PACK.md',
    'docs/strategy/EXCHANGE_LISTING_READINESS_PLAN_2026-06-29.md',
    'docs/strategy/EXCHANGE_LISTING_READINESS_TRACKER.md',
    'scripts/qa/exchange_public_readiness.py',
    'deploy/release-trust-anchor.json',
    'scripts/verify-release-checksums.mjs',
    'monitoring/shared/pq.mjs',
    'seeds.json',
];

try {
    fs.rmSync(distRoot, { recursive: true, force: true });
    fs.mkdirSync(packageRoot, { recursive: true });

    for (const relative of includedFiles) {
        const source = path.join(repoRoot, relative);
        if (!fs.existsSync(source)) throw new Error(`Missing exchange package input: ${relative}`);
        const destination = path.join(packageRoot, relative);
        fs.mkdirSync(path.dirname(destination), { recursive: true });
        fs.copyFileSync(source, destination);
    }

    const manifest = {
        schema_version: 1,
        package: 'Lichen testnet exchange integration',
        tag: releaseTag,
        validator_release: `v${runtimeVersion}`,
        rollback_anchor: 'v0.5.265',
        source_commit: sourceCommit,
        scope: 'testnet-only until the signed mainnet exchange handoff',
        rpc: 'https://testnet-api.lichen.network',
        websocket: 'wss://testnet-api.lichen.network/ws',
        developer_portal: 'https://developers.lichen.network/exchange-integration',
        status_page: 'https://exchanges.lichen.network',
        prepublication_readiness_gate: 'python3 scripts/qa/exchange_public_readiness.py --scope testnet --status-approved --release-stage candidate --candidate-dir /path/to/candidate-assets --report /path/to/exchange-public-readiness-report.json',
        postpublication_readiness_gate: 'python3 scripts/qa/exchange_public_readiness.py --scope testnet --status-approved --release-stage published',
        files: includedFiles,
    };
    fs.writeFileSync(path.join(packageRoot, 'MANIFEST.json'), `${JSON.stringify(manifest, null, 2)}\n`);

    normalizeTimes(packageRoot);
    fs.mkdirSync(distRoot, { recursive: true });
    const tarArgs = process.platform === 'linux'
        ? ['--sort=name', '--mtime=@0', '--owner=0', '--group=0', '--numeric-owner', '-czf', archivePath, packageName]
        : ['-czf', archivePath, packageName];
    execFileSync('tar', tarArgs, { cwd: tempRoot, stdio: 'inherit' });

    const digest = sha256(archivePath);
    fs.writeFileSync(checksumsPath, `${digest}  ${path.basename(archivePath)}\n`);
    process.stdout.write(`Created ${archivePath}\nCreated ${checksumsPath}\n`);
} finally {
    fs.rmSync(tempRoot, { recursive: true, force: true });
}

function parseArgs(argv) {
    const parsed = {};
    for (let index = 0; index < argv.length; index += 1) {
        if (argv[index] === '--release-tag') {
            parsed.releaseTag = argv[index + 1];
            index += 1;
        }
    }
    return parsed;
}

function read(root, relative) {
    return fs.readFileSync(path.join(root, relative), 'utf8');
}

function normalizeTimes(root) {
    const epoch = new Date(0);
    for (const entry of fs.readdirSync(root, { withFileTypes: true })) {
        const target = path.join(root, entry.name);
        if (entry.isDirectory()) normalizeTimes(target);
        fs.utimesSync(target, epoch, epoch);
    }
    fs.utimesSync(root, epoch, epoch);
}

function sha256(file) {
    return crypto.createHash('sha256').update(fs.readFileSync(file)).digest('hex');
}
