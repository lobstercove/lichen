#!/usr/bin/env node
'use strict';

const fs = require('fs');
const path = require('path');

const root = path.join(__dirname, '..', '..');
const contractPath = path.join(root, 'contracts', 'bountyboard', 'src', 'lib.rs');
const playgroundPath = path.join(root, 'programs', 'js', 'playground-complete.js');
const write = process.argv.includes('--write');

function productionContractSource() {
    const source = fs.readFileSync(contractPath, 'utf8');
    const testsMarker = '// ============================================================================\n// TESTS';
    const testsOffset = source.indexOf(testsMarker);
    if (testsOffset < 0) throw new Error('BountyBoard test boundary not found');
    return source.slice(0, testsOffset).trimEnd() + '\n';
}

function escapeTemplateLiteral(source) {
    return source
        .replaceAll('\\', '\\\\')
        .replaceAll('`', '\\`')
        .replaceAll('${', '\\${');
}

function expectedPlaygroundSource(playground) {
    const exampleOffset = playground.indexOf('    bounty: {');
    if (exampleOffset < 0) throw new Error('BountyBoard playground example not found');
    const valueMarker = "            'lib.rs': `";
    const valueOffset = playground.indexOf(valueMarker, exampleOffset);
    if (valueOffset < 0) throw new Error('BountyBoard lib.rs template start not found');
    const contentStart = valueOffset + valueMarker.length;
    const contentEnd = playground.indexOf("`,\n            'Cargo.toml':", contentStart);
    if (contentEnd < 0) throw new Error('BountyBoard lib.rs template end not found');
    const embedded = escapeTemplateLiteral(productionContractSource());
    return playground.slice(0, contentStart) + embedded + playground.slice(contentEnd);
}

const current = fs.readFileSync(playgroundPath, 'utf8');
const expected = expectedPlaygroundSource(current);
if (current === expected) {
    console.log('BountyBoard playground source matches the production contract');
    process.exit(0);
}
if (!write) {
    console.error('BountyBoard playground source drifted from contracts/bountyboard/src/lib.rs');
    console.error('Run: node scripts/qa/sync_bountyboard_playground.js --write');
    process.exit(1);
}
fs.writeFileSync(playgroundPath, expected);
console.log('Updated BountyBoard playground source from the production contract');
