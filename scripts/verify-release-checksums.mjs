#!/usr/bin/env node
import { readFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const __filename = fileURLToPath(import.meta.url);
const repoRoot = path.resolve(path.dirname(__filename), '..');
const releaseDir = path.resolve(process.argv[2] || process.cwd());
const sumsPath = path.join(releaseDir, 'SHA256SUMS');
const sigPath = path.join(releaseDir, 'SHA256SUMS.sig');
const trustAnchorPath = path.join(repoRoot, 'deploy', 'release-trust-anchor.json');
const pqModulePath = path.join(repoRoot, 'monitoring', 'shared', 'pq.mjs');

const trustAnchor = JSON.parse(await readFile(trustAnchorPath, 'utf8'));
const expectedSigner = String(trustAnchor.release_signer || '').trim();
if (!expectedSigner) {
  throw new Error(`${trustAnchorPath} does not define release_signer`);
}

const { publicKeyToAddress, verifySignature } = await import(pathToFileURL(pqModulePath).href);
const message = new Uint8Array(await readFile(sumsPath));
const signature = JSON.parse(await readFile(sigPath, 'utf8'));
const signer = await publicKeyToAddress(
  signature.public_key.bytes,
  signature.public_key.scheme_version || signature.scheme_version || 1,
);

if (signer !== expectedSigner) {
  throw new Error(`SHA256SUMS signer mismatch: got ${signer}, expected ${expectedSigner}`);
}

if (!(await verifySignature(signature, message, expectedSigner))) {
  throw new Error('SHA256SUMS PQ signature verification failed');
}

console.log(`SHA256SUMS PQ signature verified by ${signer}`);
