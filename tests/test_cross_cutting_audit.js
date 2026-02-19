/**
 * Phase 22 — Cross-Cutting Concerns Audit Tests
 * Run: node tests/test_cross_cutting_audit.js
 *
 * Verifies all findings fixed during Phase 22:
 *  C22.1 — moltpunks bare panic!() replaced with error message
 *  C22.5 — Faucet startup panics replaced with graceful exit
 *  C22.6 — shared-config.js added to monitoring, marketplace, developers
 *  C22.7 — favicon.ico added to faucet and monitoring
 *  Plus cross-cutting validations:
 *  - No todo!() or unimplemented!() in production Rust code
 *  - Font Awesome version consistency
 *  - .gitignore completeness
 */
'use strict';

const fs = require('fs');
const path = require('path');

let passed = 0, failed = 0;
function assert(cond, msg) {
    if (cond) { passed++; process.stdout.write(`  ✓ ${msg}\n`); }
    else { failed++; process.stderr.write(`  ✗ ${msg}\n`); }
}

const ROOT = path.resolve(__dirname, '..');

// ═══════════════════════════════════════════════════════════════════════════
// C22.1 — moltpunks bare panic!() replaced with descriptive message
// ═══════════════════════════════════════════════════════════════════════════
console.log('\n── C22.1: moltpunks panic message ──');
{
    const src = fs.readFileSync(path.join(ROOT, 'contracts', 'moltpunks', 'src', 'lib.rs'), 'utf8');

    // No bare panic!() (without a message)
    const barePanics = (src.match(/panic!\(\)/g) || []).length;
    assert(barePanics === 0, `No bare panic!() calls (found ${barePanics})`);

    // Has descriptive panic message
    assert(src.includes('panic!("minter not set or invalid")'),
        'get_minter has descriptive panic message');
}

// ═══════════════════════════════════════════════════════════════════════════
// C22.5 — Faucet startup panics replaced with graceful exit
// ═══════════════════════════════════════════════════════════════════════════
console.log('\n── C22.5: faucet graceful exit on bad config ──');
{
    const src = fs.readFileSync(path.join(ROOT, 'faucet', 'src', 'main.rs'), 'utf8');

    // Count panic! calls related to keypair config
    const keypairPanics = src.split('\n').filter(l =>
        /panic!.*keypair|panic!.*FAUCET_KEYPAIR|panic!.*Invalid keypair/i.test(l)
    ).length;
    assert(keypairPanics === 0,
        `No panic! calls for keypair config errors (found ${keypairPanics})`);

    // Has graceful process::exit
    assert(src.includes('std::process::exit(1)'),
        'Uses std::process::exit(1) for config errors');

    // Has eprintln for error messages
    assert(src.includes('eprintln!'),
        'Uses eprintln! for config error messages');

    // Mainnet guard panic is intentional and should remain
    assert(src.includes('panic!("❌ Faucet cannot run on mainnet!")'),
        'Mainnet guard panic is preserved (intentional)');
}

// ═══════════════════════════════════════════════════════════════════════════
// C22.6 — shared-config.js added to monitoring, marketplace, developers
// ═══════════════════════════════════════════════════════════════════════════
console.log('\n── C22.6: shared-config.js inclusion ──');
{
    const frontends = [
        { name: 'monitoring', dir: 'monitoring' },
        { name: 'marketplace', dir: 'marketplace' },
        { name: 'developers', dir: 'developers' },
    ];

    for (const { name, dir } of frontends) {
        const html = fs.readFileSync(path.join(ROOT, dir, 'index.html'), 'utf8');
        assert(html.includes('shared-config.js'),
            `${name}/index.html includes shared-config.js`);
    }

    // Verify already-existing frontends still have it
    const existingCheck = ['dex', 'wallet', 'faucet', 'explorer', 'website'];
    for (const dir of existingCheck) {
        const html = fs.readFileSync(path.join(ROOT, dir, 'index.html'), 'utf8');
        assert(html.includes('shared-config.js'),
            `${dir}/index.html still includes shared-config.js`);
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// C22.7 — favicon.ico in all frontend directories
// ═══════════════════════════════════════════════════════════════════════════
console.log('\n── C22.7: favicon.ico presence ──');
{
    const dirs = ['dex', 'wallet', 'explorer', 'developers', 'marketplace', 'website', 'faucet', 'monitoring'];
    for (const dir of dirs) {
        const faviconPath = path.join(ROOT, dir, 'favicon.ico');
        const exists = fs.existsSync(faviconPath);
        assert(exists, `${dir}/favicon.ico exists`);
        if (exists) {
            const stat = fs.statSync(faviconPath);
            assert(stat.size > 0, `${dir}/favicon.ico is non-empty (${stat.size} bytes)`);
        }
    }

    // Check HTML link tags for faucet and monitoring (newly added)
    const faucetHtml = fs.readFileSync(path.join(ROOT, 'faucet', 'index.html'), 'utf8');
    assert(faucetHtml.includes('favicon.ico'), 'faucet/index.html has favicon link tag');

    const monitorHtml = fs.readFileSync(path.join(ROOT, 'monitoring', 'index.html'), 'utf8');
    assert(monitorHtml.includes('favicon.ico'), 'monitoring/index.html has favicon link tag');
}

// ═══════════════════════════════════════════════════════════════════════════
// Cross-cutting: No todo!() or unimplemented!() in production Rust
// ═══════════════════════════════════════════════════════════════════════════
console.log('\n── CC-1: No todo!/unimplemented! in production Rust ──');
{
    const rustDirs = ['core/src', 'validator/src', 'rpc/src', 'cli/src', 'p2p/src',
                      'faucet/src', 'custody/src', 'sdk/src'];
    let todoCount = 0;
    let unimplCount = 0;

    for (const dir of rustDirs) {
        const fullDir = path.join(ROOT, dir);
        if (!fs.existsSync(fullDir)) continue;
        const files = fs.readdirSync(fullDir).filter(f => f.endsWith('.rs'));
        for (const file of files) {
            const src = fs.readFileSync(path.join(fullDir, file), 'utf8');
            // Exclude test modules
            const prodSrc = src.replace(/#\[cfg\(test\)\][\s\S]*?(?=\n(?:#\[|pub |fn |mod |$))/g, '');
            const todos = (prodSrc.match(/\btodo!\b/g) || []).length;
            const unimpls = (prodSrc.match(/\bunimplemented!\b/g) || []).length;
            todoCount += todos;
            unimplCount += unimpls;
        }
    }
    assert(todoCount === 0, `No todo!() in production Rust code (found ${todoCount})`);
    assert(unimplCount === 0, `No unimplemented!() in production Rust code (found ${unimplCount})`);
}

// ═══════════════════════════════════════════════════════════════════════════
// Cross-cutting: Font Awesome version consistency
// ═══════════════════════════════════════════════════════════════════════════
console.log('\n── CC-2: Font Awesome version consistency ──');
{
    const htmlFiles = [
        'dex/index.html', 'wallet/index.html', 'explorer/index.html',
        'developers/index.html', 'marketplace/index.html', 'website/index.html',
        'faucet/index.html', 'monitoring/index.html', 'programs/index.html',
    ];
    const versions = new Set();

    for (const f of htmlFiles) {
        const html = fs.readFileSync(path.join(ROOT, f), 'utf8');
        const match = html.match(/font-awesome\/([\d.]+)\//);
        if (match) versions.add(match[1]);
    }
    assert(versions.size === 1, `All frontends use same FA version (found: ${[...versions].join(', ')})`);
    assert(versions.has('6.5.1'), 'Font Awesome version is 6.5.1');
}

// ═══════════════════════════════════════════════════════════════════════════
// Cross-cutting: .gitignore completeness
// ═══════════════════════════════════════════════════════════════════════════
console.log('\n── CC-3: .gitignore completeness ──');
{
    const gitignore = fs.readFileSync(path.join(ROOT, '.gitignore'), 'utf8');

    const requiredPatterns = [
        'target/',       // Rust build
        'node_modules/', // Node
        '.env',          // Environment secrets
        '*.pem',         // Private keys
        'data/',         // RocksDB state
    ];

    for (const pat of requiredPatterns) {
        assert(gitignore.includes(pat), `.gitignore covers ${pat}`);
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Cross-cutting: shared-theme.css consistency
// ═══════════════════════════════════════════════════════════════════════════
console.log('\n── CC-4: shared-theme.css consistency ──');
{
    // monitoring uses standalone css/monitoring.css (self-contained dashboard)
    const frontends = ['dex', 'wallet', 'explorer', 'developers', 'marketplace',
                       'website', 'faucet'];
    let missing = [];
    for (const dir of frontends) {
        const html = fs.readFileSync(path.join(ROOT, dir, 'index.html'), 'utf8');
        if (!html.includes('shared-theme.css') && !html.includes('shared-theme')) {
            missing.push(dir);
        }
    }
    assert(missing.length === 0,
        `All standard frontends reference shared-theme.css (missing: ${missing.join(', ') || 'none'})`);

    // Monitoring uses its own standalone CSS
    const monHtml = fs.readFileSync(path.join(ROOT, 'monitoring', 'index.html'), 'utf8');
    assert(monHtml.includes('css/monitoring.css'),
        'monitoring uses standalone css/monitoring.css');
}

// ═══════════════════════════════════════════════════════════════════════════
// Cross-cutting: No private key material in log statements
// ═══════════════════════════════════════════════════════════════════════════
console.log('\n── CC-5: No private key leaks in logs ──');
{
    const sensitiveFiles = [
        'rpc/src/lib.rs', 'faucet/src/main.rs', 'custody/src/main.rs',
        'validator/src/main.rs',
    ];
    let leaks = 0;
    for (const f of sensitiveFiles) {
        const src = fs.readFileSync(path.join(ROOT, f), 'utf8');
        // Check for logging secret_key(), private_key, or seed values
        const logPatterns = /(?:info!|warn!|error!|debug!|println!)\(.*(?:secret_key\(\)|private_key|\.seed\(\)).*\)/g;
        const matches = src.match(logPatterns) || [];
        leaks += matches.length;
    }
    assert(leaks === 0, `No private key material in log statements (found ${leaks})`);
}

// ═══════════════════════════════════════════════════════════════════════════
// Summary
// ═══════════════════════════════════════════════════════════════════════════
console.log(`\n${'═'.repeat(60)}`);
console.log(`Phase 22 Tests: ${passed} passed, ${failed} failed (${passed + failed} total)`);
if (failed > 0) {
    console.error('PHASE 22 TESTS FAILED');
    process.exit(1);
}
console.log('All Phase 22 tests passed ✅');
