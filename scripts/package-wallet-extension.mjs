import crypto from 'node:crypto';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(__dirname, '..');
const extensionRoot = path.join(repoRoot, 'wallet', 'extension');
const manifestPath = path.join(extensionRoot, 'manifest.json');
const distRoot = path.join(repoRoot, 'dist', 'wallet-extension');

const args = parseArgs(process.argv.slice(2));
const manifest = JSON.parse(fs.readFileSync(manifestPath, 'utf8'));
const version = manifest.version;
const expectedReleaseTag = `wallet-extension-v${version}`;
const releaseTag = args.releaseTag || expectedReleaseTag;

if (releaseTag !== expectedReleaseTag) {
    throw new Error(`Manifest version ${version} does not match release tag ${releaseTag}`);
}

fs.rmSync(distRoot, { recursive: true, force: true });
fs.mkdirSync(distRoot, { recursive: true });

const runtimeArchiveName = `LichenWallet-extension-v${version}.zip`;
const storeBundleName = `LichenWallet-extension-store-submission-v${version}.zip`;
const runtimeArchivePath = path.join(distRoot, runtimeArchiveName);
const storeBundlePath = path.join(distRoot, storeBundleName);
const latestJsonPath = path.join(distRoot, 'latest.json');
const checksumsPath = path.join(distRoot, 'SHA256SUMS');

const tempRoot = fs.mkdtempSync(path.join(os.tmpdir(), 'lichen-wallet-extension-'));

try {
    const runtimeDir = path.join(tempRoot, 'runtime');
    const storeBundleDir = path.join(tempRoot, 'store-submission');

    fs.mkdirSync(runtimeDir, { recursive: true });
    fs.mkdirSync(storeBundleDir, { recursive: true });

    copyRuntimeTree(runtimeDir);
    normalizeTimes(runtimeDir);
    zipDirectory(runtimeDir, runtimeArchivePath);

    copyStoreBundleTree(storeBundleDir);
    normalizeTimes(storeBundleDir);
    zipDirectory(storeBundleDir, storeBundlePath);

    const runtimeSha256 = sha256(runtimeArchivePath);
    const storeBundleSha256 = sha256(storeBundlePath);
    const latestPayload = {
        name: manifest.name,
        version,
        releaseTag,
        generatedAt: new Date().toISOString(),
        channel: 'browser-store',
        artifacts: {
            runtimeZip: {
                file: runtimeArchiveName,
                sha256: runtimeSha256,
            },
            storeSubmissionBundle: {
                file: storeBundleName,
                sha256: storeBundleSha256,
            },
        },
        autoUpdate: {
            chrome: 'Automatic after publication in the Chrome Web Store.',
            edge: 'Automatic after publication in Microsoft Edge Add-ons.',
            sideload: 'Manual updates only for unpacked or direct ZIP installations.',
        },
    };

    fs.writeFileSync(latestJsonPath, JSON.stringify(latestPayload, null, 2) + '\n');
    const latestSha256 = sha256(latestJsonPath);

    const checksumLines = [
        `${runtimeSha256}  ${runtimeArchiveName}`,
        `${storeBundleSha256}  ${storeBundleName}`,
        `${latestSha256}  latest.json`,
    ];
    fs.writeFileSync(checksumsPath, checksumLines.join('\n') + '\n');

    console.log(`Created ${runtimeArchivePath}`);
    console.log(`Created ${storeBundlePath}`);
    console.log(`Created ${latestJsonPath}`);
    console.log(`Created ${checksumsPath}`);
} finally {
    fs.rmSync(tempRoot, { recursive: true, force: true });
}

function parseArgs(argv) {
    const parsed = {};
    for (let index = 0; index < argv.length; index += 1) {
        const arg = argv[index];
        if (arg === '--release-tag') {
            parsed.releaseTag = argv[index + 1];
            index += 1;
        }
    }
    return parsed;
}

function copyRuntimeTree(targetDir) {
    const rootEntries = fs.readdirSync(extensionRoot, { withFileTypes: true });
    for (const entry of rootEntries) {
        if (entry.name.startsWith('.')) continue;
        if (entry.name === 'README.md') continue;
        if (entry.name === 'store') continue;

        const sourcePath = path.join(extensionRoot, entry.name);
        const destPath = path.join(targetDir, entry.name);
        copyEntry(sourcePath, destPath);
    }
}

function copyStoreBundleTree(targetDir) {
    const bundleEntries = [
        ['README.md', 'README.md'],
        ['manifest.json', 'manifest.json'],
        ['store', 'store'],
    ];

    for (const [sourceName, destName] of bundleEntries) {
        const sourcePath = path.join(extensionRoot, sourceName);
        const destPath = path.join(targetDir, destName);
        copyEntry(sourcePath, destPath);
    }
}

function copyEntry(sourcePath, destPath) {
    const stat = fs.lstatSync(sourcePath);
    if (stat.isSymbolicLink()) {
        throw new Error(`Refusing to package symbolic link: ${sourcePath}`);
    }
    if (stat.isDirectory()) {
        fs.mkdirSync(destPath, { recursive: true });
        for (const child of fs.readdirSync(sourcePath, { withFileTypes: true })) {
            if (child.name.startsWith('.')) continue;
            copyEntry(path.join(sourcePath, child.name), path.join(destPath, child.name));
        }
        return;
    }
    if (!stat.isFile()) {
        throw new Error(`Refusing to package non-regular file: ${sourcePath}`);
    }

    fs.mkdirSync(path.dirname(destPath), { recursive: true });
    fs.copyFileSync(sourcePath, destPath);
}

function zipDirectory(sourceDir, outputFile) {
    fs.rmSync(outputFile, { force: true });
    const files = listFiles(sourceDir);
    if (files.length === 0) throw new Error(`Refusing to create an empty wallet archive from ${sourceDir}`);
    try {
        execFileSync('zip', ['-X', '-q', outputFile, ...files], { cwd: sourceDir, stdio: 'inherit' });
    } catch (error) {
        throw new Error('The `zip` command is required to package the wallet extension');
    }
}

function listFiles(root, relative = '') {
    const files = [];
    for (const entry of fs.readdirSync(path.join(root, relative), { withFileTypes: true })) {
        const child = path.join(relative, entry.name);
        if (entry.isDirectory()) files.push(...listFiles(root, child));
        else if (entry.isFile()) files.push(child.split(path.sep).join('/'));
        else throw new Error(`Refusing to package non-regular directory entry: ${child}`);
    }
    return files.sort();
}

function normalizeTimes(root) {
    const zipEpoch = new Date('1980-01-01T00:00:00.000Z');
    for (const entry of fs.readdirSync(root, { withFileTypes: true })) {
        const target = path.join(root, entry.name);
        if (entry.isDirectory()) normalizeTimes(target);
        fs.utimesSync(target, zipEpoch, zipEpoch);
    }
    fs.utimesSync(root, zipEpoch, zipEpoch);
}

function sha256(filePath) {
    const hash = crypto.createHash('sha256');
    hash.update(fs.readFileSync(filePath));
    return hash.digest('hex');
}
