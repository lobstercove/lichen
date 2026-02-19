// Phase 18 — Website Audit Tests
// 7 findings, comprehensive coverage
// Run: node tests/test_website_audit.js

const fs = require('fs');
const path = require('path');

let passed = 0;
let failed = 0;

function assert(condition, label) {
    if (condition) {
        passed++;
        console.log(`  ✅ ${label}`);
    } else {
        failed++;
        console.log(`  ❌ ${label}`);
    }
}

function extractFunction(source, name) {
    const re = new RegExp(`function ${name}\\s*\\(([^)]*)\\)\\s*\\{`);
    const match = source.match(re);
    if (!match) return null;
    let depth = 0;
    const start = match.index;
    for (let i = start; i < source.length; i++) {
        if (source[i] === '{') depth++;
        if (source[i] === '}') { depth--; if (depth === 0) return source.slice(start, i + 1); }
    }
    return null;
}

// ── Load source files ──
const scriptJs = fs.readFileSync(path.join(__dirname, '..', 'website', 'script.js'), 'utf8');
const indexHtml = fs.readFileSync(path.join(__dirname, '..', 'website', 'index.html'), 'utf8');
const websiteCss = fs.readFileSync(path.join(__dirname, '..', 'website', 'website.css'), 'utf8');
const sharedConfig = fs.readFileSync(path.join(__dirname, '..', 'website', 'shared-config.js'), 'utf8');

// ════════════════════════════════════════════════════════════
// F-1: copyCode catch handler — originalHTML scoping fix
// ════════════════════════════════════════════════════════════
console.log('\n── F-1: copyCode originalHTML scoping fix ──');

const copyCodeFn = extractFunction(scriptJs, 'copyCode');
assert(copyCodeFn !== null, 'copyCode function exists');
assert(copyCodeFn !== null && /const originalHTML = button\.innerHTML;\s*\n\s*\n\s*navigator\.clipboard/.test(copyCodeFn),
    'originalHTML declared BEFORE navigator.clipboard.writeText call');
assert(copyCodeFn !== null && !/(\.then\(\(\)\s*=>\s*\{[^}]*const originalHTML)/.test(copyCodeFn),
    'originalHTML is NOT declared inside .then() callback');

// Verify the catch block can access originalHTML (it's in outer scope now)
const thenBlock = copyCodeFn ? copyCodeFn.match(/\.then\(\(\)\s*=>\s*\{/) : null;
const catchBlock = copyCodeFn ? copyCodeFn.match(/\.catch\(/) : null;
assert(thenBlock !== null, '.then() callback exists');
assert(catchBlock !== null, '.catch() callback exists');

// Verify catch block restores button properly
assert(copyCodeFn !== null && /\.catch\([^)]*\)\s*\.\s*catch|\}[\s\S]*?\.catch\([\s\S]*?button\.innerHTML\s*=\s*originalHTML/.test(copyCodeFn),
    'catch block restores originalHTML');

// Verify catch block also resets color
assert(copyCodeFn !== null && /catch[\s\S]*?button\.style\.color\s*=\s*['"]#FF6B6B['"]/.test(copyCodeFn),
    'catch block sets error color (#FF6B6B)');

// Verify catch resets color back to empty after timeout
assert(copyCodeFn !== null && /catch[\s\S]*?setTimeout[\s\S]*?button\.style\.color\s*=\s*['"]["']/.test(copyCodeFn),
    'catch block resets color back to empty string after timeout');

// ════════════════════════════════════════════════════════════
// F-2: Mobile nav-actions visibility
// ════════════════════════════════════════════════════════════
console.log('\n── F-2: Mobile nav-actions visibility ──');

// JS must toggle nav-actions
assert(/navActions/.test(scriptJs), 'script.js references navActions element');
assert(/querySelector\(['"]\.nav-actions['"]\)/.test(scriptJs), 'script.js selects .nav-actions');
assert(/navActions\.classList\.toggle\(['"]active['"]\)/.test(scriptJs) || 
       /navActions\)?\s*\.classList\.toggle\(['"]active['"]\)/.test(scriptJs),
    'script.js toggles active class on navActions');

// CSS must show .nav-actions.active on mobile
assert(/\.nav-actions\.active\s*\{/.test(websiteCss), 'website.css has .nav-actions.active rule');
assert(/\.nav-actions\.active[\s\S]*?display:\s*flex/.test(websiteCss),
    '.nav-actions.active sets display:flex');

// Old .nav.mobile-open pattern removed
assert(!/\.nav\.mobile-open/.test(websiteCss), 'old .nav.mobile-open selector removed from CSS');

// The .nav-menu.active rule should exist in injected styles (script.js) or website.css
assert(/\.nav-menu\.active\s*\{/.test(scriptJs) || /\.nav-menu\.active\s*\{/.test(websiteCss),
    '.nav-menu.active rule exists');

// ════════════════════════════════════════════════════════════
// F-3: Footer links no longer point to raw markdown files
// ════════════════════════════════════════════════════════════
console.log('\n── F-3: Footer resource links ──');

// Extract footer section
const footerMatch = indexHtml.match(/<footer[\s\S]*?<\/footer>/);
const footer = footerMatch ? footerMatch[0] : '';

assert(footer.length > 100, 'footer element found in HTML');
assert(!/href=["']\.\.\/docs\/README\.md["']/.test(footer), 'no link to ../docs/README.md');
assert(!/href=["']\.\.\/docs\/VISION\.md["']/.test(footer), 'no link to ../docs/VISION.md');
assert(!/href=["']\.\.\/docs\/WHITEPAPER\.md["']/.test(footer), 'no link to ../docs/WHITEPAPER.md');
assert(!/href=["']\.\.\/docs\/VALIDATOR_SETUP\.md["']/.test(footer), 'no link to ../docs/VALIDATOR_SETUP.md');

// Footer resource links now use data-molt-app
const footerResourceCol = footer.match(/Resources[\s\S]*?<\/ul>/);
const footerResources = footerResourceCol ? footerResourceCol[0] : '';
assert(/data-molt-app=["']developers["']/.test(footerResources), 'footer resources use data-molt-app="developers"');

// Check that developer portal paths are used
assert(/data-molt-path=["']\/architecture\.html["']/.test(footerResources) ||
       /data-molt-path=["']\/getting-started\.html["']/.test(footerResources),
    'footer resources link to developer portal pages');

// Validator guide link in footer
assert(/data-molt-path=["']\/validator\.html["']/.test(footerResources),
    'footer validator guide links to developer portal validator page');

// No .md file links anywhere in footer at all
assert(!/\.md["']/.test(footer), 'no .md links in footer at all');

// ════════════════════════════════════════════════════════════
// F-4: Validator CTA link fixed
// ════════════════════════════════════════════════════════════
console.log('\n── F-4: Validator CTA link ──');

// The validator section CTA
const validatorSection = indexHtml.match(/id=["']validators["'][\s\S]*?(?=<section|$)/);
const validatorHtml = validatorSection ? validatorSection[0] : '';

assert(!(/href=["']docs\/skills\/VALIDATOR_SKILL\.md["']/.test(validatorHtml)),
    'broken validator skill link removed');
assert(/data-molt-app=["']developers["']/.test(validatorHtml),
    'validator CTA uses data-molt-app="developers"');
assert(/data-molt-path=["']\/validator\.html["']/.test(validatorHtml),
    'validator CTA links to /validator.html');
assert(/Start Validating Now/.test(validatorHtml),
    'Start Validating Now text preserved');

// ════════════════════════════════════════════════════════════
// F-5: Accessibility — aria-labels
// ════════════════════════════════════════════════════════════
console.log('\n── F-5: Accessibility — aria-labels ──');

// Nav toggle has aria-label
assert(/id=["']navToggle["'][^>]*aria-label/.test(indexHtml),
    'nav toggle button has aria-label');
assert(/aria-label=["']Toggle navigation menu["']/.test(indexHtml),
    'nav toggle aria-label says "Toggle navigation menu"');

// All copy buttons have aria-label
const copyButtons = indexHtml.match(/<button[^>]*class=["']copy-btn["'][^>]*>/g) || [];
assert(copyButtons.length >= 7, `found ${copyButtons.length} copy buttons (expected >= 7)`);
const copyButtonsWithAria = copyButtons.filter(btn => /aria-label/.test(btn));
assert(copyButtonsWithAria.length === copyButtons.length,
    `all ${copyButtons.length} copy buttons have aria-label`);

// Verify the aria-label content is descriptive
assert(copyButtons.every(btn => /aria-label=["']Copy code to clipboard["']/.test(btn)),
    'all copy buttons say "Copy code to clipboard"');

// ════════════════════════════════════════════════════════════
// F-6: formatNumber guards against non-numeric input
// ════════════════════════════════════════════════════════════
console.log('\n── F-6: formatNumber non-numeric guard ──');

const formatFn = extractFunction(scriptJs, 'formatNumber');
assert(formatFn !== null, 'formatNumber function exists');

// Must check for typeof or isFinite
assert(formatFn !== null && /typeof num/.test(formatFn), 'formatNumber checks typeof num');
assert(formatFn !== null && /isFinite/.test(formatFn), 'formatNumber checks isFinite');

// Build and test the function
if (formatFn) {
    const fn = new Function('return ' + formatFn)();
    
    assert(fn(1234567) === '1.2M', 'formatNumber(1234567) => "1.2M"');
    assert(fn(45000) === '45.0K', 'formatNumber(45000) => "45.0K"');
    assert(fn(999) === '999', 'formatNumber(999) => "999"');
    assert(fn(0) === '0', 'formatNumber(0) => "0"');
    assert(fn(undefined) === '—', 'formatNumber(undefined) => "—"');
    assert(fn(null) === '—', 'formatNumber(null) => "—"');
    assert(fn('hello') === '—', 'formatNumber("hello") => "—"');
    assert(fn({error: 'fail'}) === '—', 'formatNumber({error: "fail"}) => "—"');
    assert(fn(NaN) === '—', 'formatNumber(NaN) => "—"');
    assert(fn(Infinity) === '—', 'formatNumber(Infinity) => "—"');
    assert(fn(-Infinity) === '—', 'formatNumber(-Infinity) => "—"');
}

// ════════════════════════════════════════════════════════════
// F-7: WebSocket cleanup on page unload / visibility change
// ════════════════════════════════════════════════════════════
console.log('\n── F-7: WebSocket cleanup ──');

assert(/beforeunload/.test(scriptJs), 'script.js registers beforeunload handler');
assert(/visibilitychange/.test(scriptJs), 'script.js registers visibilitychange handler');
assert(/document\.hidden/.test(scriptJs), 'script.js checks document.hidden');

// On hidden, should disconnect
assert(/document\.hidden[\s\S]*?disconnectWebsiteWS/.test(scriptJs) ||
       /if\s*\(\s*document\.hidden\s*\)\s*\{[\s\S]*?disconnectWebsiteWS/.test(scriptJs),
    'disconnects WebSocket when page is hidden');

// On visible, should reconnect
const visBlock = scriptJs.match(/visibilitychange[\s\S]*?(?=window\.addEventListener|\/\/ Console|$)/);
assert(visBlock !== null && /connectWebsiteWS/.test(visBlock[0]),
    'reconnects WebSocket when page becomes visible');

// beforeunload should disconnect
const unloadBlock = scriptJs.match(/beforeunload[\s\S]*?(?=document\.addEventListener|\/\/ Console|$)/);
assert(unloadBlock !== null && /disconnectWebsiteWS/.test(unloadBlock[0]),
    'disconnects WebSocket on beforeunload');

// disconnectWebsiteWS clears the reconnect timer
const disconnectFn = extractFunction(scriptJs, 'disconnectWebsiteWS');
assert(disconnectFn !== null, 'disconnectWebsiteWS function exists');
assert(disconnectFn !== null && /clearTimeout/.test(disconnectFn),
    'disconnectWebsiteWS clears reconnect timer');

// ════════════════════════════════════════════════════════════
// STRUCTURAL INTEGRITY CHECKS
// ════════════════════════════════════════════════════════════
console.log('\n── Structural Integrity ──');

// shared-config.js exists and has all apps
assert(/MOLT_CONFIG/.test(sharedConfig), 'shared-config.js defines MOLT_CONFIG');
assert(/explorer/.test(sharedConfig), 'MOLT_CONFIG has explorer');
assert(/wallet/.test(sharedConfig), 'MOLT_CONFIG has wallet');
assert(/developers/.test(sharedConfig), 'MOLT_CONFIG has developers');
assert(/faucet/.test(sharedConfig), 'MOLT_CONFIG has faucet');
assert(/marketplace/.test(sharedConfig), 'MOLT_CONFIG has marketplace');
assert(/dex/.test(sharedConfig), 'MOLT_CONFIG has dex');

// data-molt-app links exist in HTML
const moltAppLinks = indexHtml.match(/data-molt-app=["'][^"']+["']/g) || [];
assert(moltAppLinks.length >= 6, `found ${moltAppLinks.length} data-molt-app links (expected >= 6)`);

// shared-config.js included before script.js
const scriptOrder = indexHtml.match(/<script[^>]*>[\s\S]*?<\/body>/);
assert(scriptOrder && /shared-config\.js[\s\S]*script\.js/.test(scriptOrder[0]),
    'shared-config.js loaded before script.js');

// Network selector exists with correct options
assert(/id=["']websiteNetworkSelect["']/.test(indexHtml), 'network selector exists');
assert(/value=["']mainnet["']/.test(indexHtml), 'mainnet option exists');
assert(/value=["']testnet["']/.test(indexHtml), 'testnet option exists');
assert(/value=["']local-testnet["']/.test(indexHtml), 'local-testnet option exists');

// Hero stats elements exist
assert(/id=["']stat-block["']/.test(indexHtml), 'stat-block element exists');
assert(/id=["']stat-validators["']/.test(indexHtml), 'stat-validators element exists');

// All 27 contracts listed
const contractCards = indexHtml.match(/class=["']contract-card/g) || [];
assert(contractCards.length === 27, `found ${contractCards.length} contract cards (expected 27)`);

// Ecosystem link
assert(/Browse All 27 Contracts/.test(indexHtml), '"Browse All 27 Contracts" CTA exists');

// Roadmap phases
assert(/Phase 1:.*Foundation/.test(indexHtml), 'Phase 1 roadmap exists');
assert(/Phase 2:.*Awakening/.test(indexHtml), 'Phase 2 roadmap exists');
assert(/Phase 3:.*Swarming/.test(indexHtml), 'Phase 3 roadmap exists');

// RPC class has essential methods
assert(/class MoltChainRPC/.test(scriptJs), 'MoltChainRPC class exists');
assert(/async getSlot/.test(scriptJs), 'getSlot method exists');
assert(/async getValidators/.test(scriptJs), 'getValidators method exists');
assert(/async getBalance/.test(scriptJs), 'getBalance method exists');
assert(/async sendTransaction/.test(scriptJs), 'sendTransaction method exists');

// WebSocket functions exist
assert(/function connectWebsiteWS/.test(scriptJs), 'connectWebsiteWS function exists');
assert(/function disconnectWebsiteWS/.test(scriptJs), 'disconnectWebsiteWS function exists');

// Intersection observer for animations
assert(/IntersectionObserver/.test(scriptJs), 'IntersectionObserver used for scroll animations');

// Parallax effect
assert(/requestAnimationFrame\(updateParallax\)/.test(scriptJs), 'parallax uses requestAnimationFrame');

// API tabs and wizard tabs setup
assert(/function setupApiTabs/.test(scriptJs), 'setupApiTabs function exists');
assert(/function setupWizardTabs/.test(scriptJs), 'setupWizardTabs function exists');

// CSS responsive breakpoints
assert(/@media \(max-width: 768px\)/.test(websiteCss), 'website.css has 768px breakpoint');
assert(/@media \(max-width: 480px\)/.test(websiteCss), 'website.css has 480px breakpoint');
assert(/@media \(max-width: 1200px\)/.test(websiteCss), 'website.css has 1200px breakpoint');

// HTML meta tags
assert(/<meta name=["']description["']/.test(indexHtml), 'meta description tag exists');
assert(/<meta name=["']viewport["']/.test(indexHtml), 'viewport meta tag exists');
assert(/<link rel=["']icon["']/.test(indexHtml), 'favicon link exists');

// ════════════════════════════════════════════════════════════
console.log('\n' + '═'.repeat(60));
console.log(`Phase 18 — Website Audit: ${passed} passed, ${failed} failed (${passed + failed} total)`);
console.log('═'.repeat(60));
process.exit(failed > 0 ? 1 : 0);
