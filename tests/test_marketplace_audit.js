// Phase 15 — Marketplace Audit Tests
// 7 findings, comprehensive coverage
// Run: node tests/test_marketplace_audit.js

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
        const ch = source[i];
        // Skip string literals and template literals
        if (ch === '"' || ch === "'" || ch === '`') {
            const q = ch;
            i++;
            while (i < source.length && source[i] !== q) {
                if (source[i] === '\\') i++; // skip escaped char
                i++;
            }
            continue;
        }
        // Skip single-line comments
        if (ch === '/' && source[i + 1] === '/') {
            while (i < source.length && source[i] !== '\n') i++;
            continue;
        }
        // Skip multi-line comments
        if (ch === '/' && source[i + 1] === '*') {
            i += 2;
            while (i < source.length - 1 && !(source[i] === '*' && source[i + 1] === '/')) i++;
            i++;
            continue;
        }
        if (ch === '{') depth++;
        if (ch === '}') { depth--; if (depth === 0) return source.slice(start, i + 1); }
    }
    return null;
}

function buildEscapeHtml(source) {
    const fn = extractFunction(source, 'escapeHtml');
    if (!fn) return null;
    return new Function('return ' + fn)();
}

// ── Load source files ──
const marketplaceJs = fs.readFileSync(path.join(__dirname, '..', 'marketplace', 'js', 'marketplace.js'), 'utf8');
const browseJs = fs.readFileSync(path.join(__dirname, '..', 'marketplace', 'js', 'browse.js'), 'utf8');
const itemJs = fs.readFileSync(path.join(__dirname, '..', 'marketplace', 'js', 'item.js'), 'utf8');
const createJs = fs.readFileSync(path.join(__dirname, '..', 'marketplace', 'js', 'create.js'), 'utf8');
const profileJs = fs.readFileSync(path.join(__dirname, '..', 'marketplace', 'js', 'profile.js'), 'utf8');

// ════════════════════════════════════════════════════════════
// M-1: XSS in marketplace.js — loadFeaturedCollections, loadTopCreators, loadRecentSales
// ════════════════════════════════════════════════════════════
console.log('\n── M-1: marketplace.js XSS fix ──');

assert(marketplaceJs.includes('function escapeHtml('), 'M-1.1  escapeHtml function exists');

// loadFeaturedCollections escapes collection data
assert(marketplaceJs.includes("escapeHtml(collection.id)"), 'M-1.2  collection.id escaped in onclick');
assert(marketplaceJs.includes("escapeHtml(collection.name)"), 'M-1.3  collection.name escaped');
assert(marketplaceJs.includes("escapeHtml(collection.banner)"), 'M-1.4  collection.banner escaped in style');
assert(marketplaceJs.includes("escapeHtml(collection.avatar)"), 'M-1.5  collection.avatar escaped');
assert(marketplaceJs.includes("escapeHtml(collection.floor)"), 'M-1.6  collection.floor escaped');

// loadTopCreators escapes creator data
assert(marketplaceJs.includes("escapeHtml(creator.id)"), 'M-1.7  creator.id escaped in onclick');
assert(marketplaceJs.includes("escapeHtml(creator.avatar)"), 'M-1.8  creator.avatar escaped');
assert(marketplaceJs.includes("escapeHtml(creator.name)"), 'M-1.9  creator.name escaped');

// loadRecentSales escapes sale data
assert(marketplaceJs.includes("escapeHtml(sale.id)"), 'M-1.10 sale.id escaped in onclick');
assert(marketplaceJs.includes("escapeHtml(sale.image)"), 'M-1.11 sale.image escaped in style');
assert(marketplaceJs.includes("escapeHtml(sale.nft)"), 'M-1.12 sale.nft escaped');
assert(marketplaceJs.includes("escapeHtml(sale.collection)"), 'M-1.13 sale.collection escaped');
assert(marketplaceJs.includes("escapeHtml(sale.price)"), 'M-1.14 sale.price escaped');
assert(marketplaceJs.includes("escapeHtml(sale.from)"), 'M-1.15 sale.from escaped');
assert(marketplaceJs.includes("escapeHtml(sale.to)"), 'M-1.16 sale.to escaped');

// Validate escapeHtml function
const mkEsc = buildEscapeHtml(marketplaceJs);
assert(mkEsc !== null, 'M-1.17 escapeHtml function parses');
if (mkEsc) {
    assert(mkEsc('<script>alert(1)</script>') === '&lt;script&gt;alert(1)&lt;/script&gt;', 'M-1.18 escapeHtml handles < >');
    assert(mkEsc('"hello"') === '&quot;hello&quot;', 'M-1.19 escapeHtml handles quotes');
    assert(mkEsc("it's") === "it&#39;s", 'M-1.20 escapeHtml handles single quotes');
    assert(mkEsc('a&b') === 'a&amp;b', 'M-1.21 escapeHtml handles &');
    assert(mkEsc(null) === '', 'M-1.22 escapeHtml handles null');
    assert(mkEsc(undefined) === '', 'M-1.23 escapeHtml handles undefined');
}

// ════════════════════════════════════════════════════════════
// M-2: XSS in profile.js — no escapeHtml
// ════════════════════════════════════════════════════════════
console.log('\n── M-2: profile.js XSS fix ──');

assert(profileJs.includes('function escapeHtml('), 'M-2.1  escapeHtml function added');
assert(profileJs.includes('function safeImageUrl('), 'M-2.2  safeImageUrl function added');

// renderNFTGrid escaping
assert(profileJs.includes("escapeHtml(nft.id || nft.token || '')"), 'M-2.3  nft.id escaped in onclick');
assert(profileJs.includes("escapeHtml(nft.collection || nft.collection_name || 'Unknown')"), 'M-2.4  nft.collection escaped');
assert(profileJs.includes("escapeHtml(name)"), 'M-2.5  nft name escaped');
assert(profileJs.includes("escapeHtml(price)"), 'M-2.6  nft price escaped');

// loadActivity escaping
assert(profileJs.includes("escapeHtml(event.token)"), 'M-2.7  event.token escaped in onclick');
assert(profileJs.includes("escapeHtml(event.type || '-')"), 'M-2.8  event.type escaped');
assert(profileJs.includes("escapeHtml(event.item || '-')"), 'M-2.9  event.item escaped');
assert(profileJs.includes("escapeHtml(event.from || '')"), 'M-2.10 event.from escaped');
assert(profileJs.includes("escapeHtml(event.to || '')"), 'M-2.11 event.to escaped');

// safeImageUrl validation
const safeImgFn = extractFunction(profileJs, 'safeImageUrl');
assert(safeImgFn !== null, 'M-2.12 safeImageUrl function parses');
if (safeImgFn) {
    const safeImage = new Function('return ' + safeImgFn)();
    assert(safeImage('https://example.com/img.png') === 'https://example.com/img.png', 'M-2.13 allows https');
    assert(safeImage('http://example.com/img.png') === 'http://example.com/img.png', 'M-2.14 allows http');
    assert(safeImage('ipfs://Qmabc') === 'https://ipfs.io/ipfs/Qmabc', 'M-2.15 converts ipfs to https');
    assert(safeImage('javascript:alert(1)') === null, 'M-2.16 rejects javascript: protocol');
    assert(safeImage('data:text/html,<script>') === null, 'M-2.17 rejects data: protocol');
    assert(safeImage(null) === null, 'M-2.18 handles null');
    assert(safeImage('linear-gradient(135deg, #f00, #0f0)') !== null, 'M-2.19 allows linear-gradient');
}

// renderNFTGrid uses safeImageUrl for image URLs
assert(profileJs.includes('safeImageUrl(imageUrl)'), 'M-2.20 renderNFTGrid uses safeImageUrl');
assert(profileJs.includes('encodeURI(safeUrl)'), 'M-2.21 URL encoded in background-image');

// ════════════════════════════════════════════════════════════
// M-3: XSS in browse.js list view
// ════════════════════════════════════════════════════════════
console.log('\n── M-3: browse.js list view XSS fix ──');

assert(browseJs.includes('function escapeHtml('), 'M-3.1  escapeHtml function exists');

// List view escaping — search in the full browse.js source
assert(browseJs.includes("escapeHtml(nft.id) + '\\')"), 'M-3.2  list view nft.id escaped in onclick');
assert(browseJs.includes("escapeHtml(nft.collection || 'Unknown')"), 'M-3.3  list view nft.collection escaped');
assert(browseJs.includes("escapeHtml(nft.name || 'Unnamed')"), 'M-3.4  list view nft.name escaped');
assert(browseJs.includes("escapeHtml(nft.price) + ' MOLT</div>'"), 'M-3.5  list view nft.price escaped');
assert(browseJs.includes("escapeHtml(nft.rarity || 'Common')"), 'M-3.6  list view nft.rarity escaped');
assert(browseJs.includes("escapeHtml((nft.rarity || 'Common').toLowerCase())"), 'M-3.7  list view rarity class escaped');

// loadCollections escaping
assert(browseJs.includes("escapeHtml(c.name || c.symbol"), 'M-3.8  collection name escaped in filter');
assert(browseJs.includes("escapeHtml(c.id || c.program_id"), 'M-3.9  collection id escaped in filter value');

// normalizeImage rejects non-standard protocols
assert(browseJs.includes("uri.startsWith('linear-gradient')"), 'M-3.10 normalizeImage allows linear-gradient');

// ════════════════════════════════════════════════════════════
// M-4: XSS in item.js — loadMoreFromCollection, loadActivity
// ════════════════════════════════════════════════════════════
console.log('\n── M-4: item.js XSS fix ──');

assert(itemJs.includes('function escapeHtml('), 'M-4.1  escapeHtml function exists');
assert(itemJs.includes('function safeImageUrl('), 'M-4.2  safeImageUrl function added');

// loadMoreFromCollection escaping
const moreSection = itemJs.slice(itemJs.indexOf('loadMoreFromCollection'));
assert(moreSection.includes("escapeHtml(nft.id || nft.token)"), 'M-4.3  nft.id escaped in onclick');
assert(moreSection.includes("escapeHtml(nft.name || '#'"), 'M-4.4  nft.name escaped');
assert(moreSection.includes("escapeHtml(price)"), 'M-4.5  price escaped');
assert(moreSection.includes("safeImageUrl(rawUrl)"), 'M-4.6  image URL validated via safeImageUrl');
assert(moreSection.includes("encodeURI(safeUrl)"), 'M-4.7  URL encoded in background-image');

// loadActivity escaping
const activitySection = itemJs.slice(itemJs.indexOf('async function loadActivity'));
assert(activitySection.includes("escapeHtml(event.type || event.kind || 'Event')"), 'M-4.8  event.type escaped');
assert(activitySection.includes("escapeHtml(price)"), 'M-4.9  activity price escaped');

// ════════════════════════════════════════════════════════════
// M-5: XSS in create.js — file preview, property rendering
// ════════════════════════════════════════════════════════════
console.log('\n── M-5: create.js XSS fix ──');

assert(createJs.includes('function escapeHtml('), 'M-5.1  escapeHtml function added');

// showFilePreview escapes file.name
assert(createJs.includes("escapeHtml(file.name)"), 'M-5.2  file.name escaped in preview');
assert(!createJs.includes("+ file.name + ' ('"), 'M-5.3  raw file.name no longer used');

// renderProperties escapes trait values
assert(createJs.includes("escapeHtml(prop.trait_type || '')"), 'M-5.4  trait_type escaped in value attribute');
assert(createJs.includes("escapeHtml(prop.value || '')"), 'M-5.5  prop.value escaped in value attribute');

// Validate escapeHtml handles attribute injection
const createEsc = buildEscapeHtml(createJs);
assert(createEsc !== null, 'M-5.6  escapeHtml function parses');
if (createEsc) {
    // Attribute breakout: value=" would inject into value="..."
    assert(createEsc('" onfocus="alert(1)').indexOf('"') === -1, 'M-5.7  escapeHtml prevents attribute breakout');
    assert(createEsc("' onfocus='alert(1)").indexOf("'") === -1, 'M-5.8  escapeHtml prevents single-quote breakout');
}

// ════════════════════════════════════════════════════════════
// M-6: Image URL injection — profile.js, item.js, browse.js
// ════════════════════════════════════════════════════════════
console.log('\n── M-6: Image URL injection fix ──');

// profile.js: safeImageUrl checks protocol
assert(profileJs.includes("safeImageUrl(imageUrl)"), 'M-6.1  profile.js uses safeImageUrl for images');
assert(!profileJs.includes("'background-image: url(' + url + ')"), 'M-6.2  profile.js no raw url() injection');

// item.js: safeImageUrl in loadMoreFromCollection
assert(itemJs.includes("safeImageUrl(rawUrl)"), 'M-6.3  item.js uses safeImageUrl for collection images');

// browse.js: normalizeImage rejects unknown protocols
const browseNormalize = extractFunction(browseJs, 'normalizeImage');
assert(browseNormalize !== null, 'M-6.4  browse.js normalizeImage exists');
if (browseNormalize) {
    // Provide stub for gradientFromHash dependency
    const testNormImg = new Function('gradientFromHash', 'return ' + browseNormalize);
    const normImg = testNormImg(function(s) { return 'gradient-' + s; });
    assert(typeof normImg === 'function', 'M-6.5  normalizeImage is callable');
    // Test: non-standard protocol returns gradient (not passthrough)
    const result = normImg('javascript:alert(1)', 'test');
    assert(!result.includes('javascript:'), 'M-6.6  normalizeImage rejects javascript: protocol');
    const dataResult = normImg('data:text/html,<h1>hi</h1>', 'test');
    assert(!dataResult.includes('data:'), 'M-6.7  normalizeImage rejects data: protocol');
}

// item.js safeImageUrl rejects dangerous protocols
const itemSafeImg = extractFunction(itemJs, 'safeImageUrl');
assert(itemSafeImg !== null, 'M-6.8  item.js safeImageUrl exists');
if (itemSafeImg) {
    const sImg = new Function('return ' + itemSafeImg)();
    assert(sImg('javascript:alert(1)') === null, 'M-6.9  rejects javascript: in item.js');
    assert(sImg('data:image/png;base64,abc') === null, 'M-6.10 rejects data: in item.js');
    assert(sImg('ftp://evil.com/img.png') === null, 'M-6.11 rejects ftp: in item.js');
}

// ════════════════════════════════════════════════════════════
// M-7: Mint input length limits in create.js
// ════════════════════════════════════════════════════════════
console.log('\n── M-7: Mint input length limits ──');

assert(createJs.includes('name.length > 128'), 'M-7.1  Name length check (128 chars)');
assert(createJs.includes("'NFT name must be 128 characters or fewer'"), 'M-7.2  Name limit error message');
assert(createJs.includes('description.length > 2048'), 'M-7.3  Description length check (2048 chars)');
assert(createJs.includes("'Description must be 2048 characters or fewer'"), 'M-7.4  Description limit error message');

// Verify the length checks are BEFORE the upload check (order matters)
const nameCheckPos = createJs.indexOf('name.length > 128');
const descCheckPos = createJs.indexOf('description.length > 2048');
const uploadCheckPos = createJs.indexOf("if (!uploadedFile)");
assert(nameCheckPos < uploadCheckPos, 'M-7.5  Name check is before upload check');
assert(descCheckPos < uploadCheckPos, 'M-7.6  Description check is before upload check');

// ════════════════════════════════════════════════════════════
// Cross-cutting: verify no raw innerHTML injection remains
// ════════════════════════════════════════════════════════════
console.log('\n── Cross-cutting verification ──');

// marketplace.js: all 4 render functions should use escapeHtml
const mkTrendingSection = marketplaceJs.slice(marketplaceJs.indexOf('loadTrendingNFTs'));
assert(mkTrendingSection.includes("escapeHtml(nft.id)"), 'CC-1  loadTrendingNFTs nft.id is escaped');
assert(mkTrendingSection.includes("escapeHtml(nft.collection)"), 'CC-2  loadTrendingNFTs nft.collection is escaped');

// browse.js grid view (already fixed in prior audit) still works
const gridViewSection = browseJs.slice(browseJs.indexOf('if (currentView ==='), browseJs.indexOf('} else {'));
assert(gridViewSection.includes("escapeHtml(nft.id)"), 'CC-3  grid view nft.id is escaped');
assert(gridViewSection.includes("escapeHtml(nft.collection || 'Unknown')"), 'CC-4  grid view nft.collection is escaped');

// item.js renderNFTDetail uses escapeHtml for image src
assert(itemJs.includes("escapeHtml(imageUrl)"), 'CC-5  renderNFTDetail image src escaped');
assert(itemJs.includes("escapeHtml(nft.name || 'NFT')"), 'CC-6  renderNFTDetail image alt escaped');

// item.js renderProperties uses escapeHtml
assert(itemJs.includes("escapeHtml(prop.trait_type || prop.key || 'Unknown')"), 'CC-7  renderProperties trait_type escaped');
assert(itemJs.includes("escapeHtml(prop.value || '-')"), 'CC-8  renderProperties value escaped');

// All 5 JS files have escapeHtml
assert(marketplaceJs.includes('function escapeHtml('), 'CC-9   marketplace.js has escapeHtml');
assert(browseJs.includes('function escapeHtml('), 'CC-10  browse.js has escapeHtml');
assert(itemJs.includes('function escapeHtml('), 'CC-11  item.js has escapeHtml');
assert(createJs.includes('function escapeHtml('), 'CC-12  create.js has escapeHtml');
assert(profileJs.includes('function escapeHtml('), 'CC-13  profile.js has escapeHtml');

// ════════════════════════════════════════════════════════════
// Summary
// ════════════════════════════════════════════════════════════
console.log(`\n${'═'.repeat(50)}`);
console.log(`Phase 15 Marketplace Audit: ${passed} passed, ${failed} failed (${passed + failed} total)`);
console.log(`${'═'.repeat(50)}`);
process.exit(failed > 0 ? 1 : 0);
