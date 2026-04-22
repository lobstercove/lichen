const fs = require('fs');
const path = require('path');

const REPO_ROOT = path.resolve(__dirname, '..');

const HELPER_FAMILIES = [
    {
        id: 'shared-config-standard',
        source: 'wallet/shared-config.js',
        targets: [
            'developers/shared-config.js',
            'dex/shared-config.js',
            'explorer/shared-config.js',
            'marketplace/shared-config.js',
            'monitoring/shared-config.js',
            'programs/shared-config.js',
            'website/shared-config.js',
        ],
        note: 'Standard public-portal shared config family. Faucet remains an intentional variant.',
    },
    {
        id: 'wallet-connect-extension',
        source: 'wallet/shared/wallet-connect.js',
        targets: [
            'explorer/shared/wallet-connect.js',
            'faucet/shared/wallet-connect.js',
            'monitoring/shared/wallet-connect.js',
            'programs/shared/wallet-connect.js',
        ],
        note: 'Extension-only wallet-connect family. Developers, Marketplace, and DEX remain intentional variants.',
    },
    {
        id: 'shared-utils-standard',
        source: 'wallet/shared/utils.js',
        targets: [
            'developers/shared/utils.js',
            'dex/shared/utils.js',
            'faucet/shared/utils.js',
            'marketplace/shared/utils.js',
            'monitoring/shared/utils.js',
            'programs/shared/utils.js',
            'wallet/extension/shared/utils.js',
        ],
        note: 'Standard shared utility family. Explorer keeps dedicated shared/page utility variants.',
    },
];

function resolveRepoPath(relativePath) {
    return path.join(REPO_ROOT, ...relativePath.split('/'));
}

function readFile(relativePath) {
    return fs.readFileSync(resolveRepoPath(relativePath), 'utf8');
}

function findDrift() {
    const drift = [];

    for (const family of HELPER_FAMILIES) {
        const sourceContent = readFile(family.source);
        for (const target of family.targets) {
            const targetContent = readFile(target);
            if (targetContent !== sourceContent) {
                drift.push({
                    familyId: family.id,
                    source: family.source,
                    target,
                });
            }
        }
    }

    return drift;
}

function syncFamilies() {
    const updated = [];

    for (const family of HELPER_FAMILIES) {
        const sourceContent = readFile(family.source);
        for (const target of family.targets) {
            const targetPath = resolveRepoPath(target);
            const targetContent = fs.readFileSync(targetPath, 'utf8');
            if (targetContent === sourceContent) {
                continue;
            }
            fs.writeFileSync(targetPath, sourceContent, 'utf8');
            updated.push({ familyId: family.id, source: family.source, target });
        }
    }

    return updated;
}

function runCli() {
    const checkOnly = process.argv.includes('--check');

    if (checkOnly) {
        const drift = findDrift();
        if (!drift.length) {
            console.log('Frontend shared helpers: no drift detected across canonical families.');
            return 0;
        }

        console.error('Frontend shared helpers: drift detected.');
        for (const entry of drift) {
            console.error(`- ${entry.familyId}: ${entry.target} diverged from ${entry.source}`);
        }
        return 1;
    }

    const updated = syncFamilies();
    if (!updated.length) {
        console.log('Frontend shared helpers: already synchronized.');
        return 0;
    }

    console.log(`Frontend shared helpers: synchronized ${updated.length} file(s).`);
    for (const entry of updated) {
        console.log(`- ${entry.familyId}: ${entry.target} <= ${entry.source}`);
    }
    return 0;
}

if (require.main === module) {
    process.exitCode = runCli();
}

module.exports = {
    HELPER_FAMILIES,
    REPO_ROOT,
    findDrift,
    syncFamilies,
};