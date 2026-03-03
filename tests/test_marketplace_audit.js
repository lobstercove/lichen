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
const rpcRs = fs.readFileSync(path.join(__dirname, '..', 'rpc', 'src', 'lib.rs'), 'utf8');
const marketplaceConfigJs = fs.readFileSync(path.join(__dirname, '..', 'marketplace', 'js', 'marketplace-config.js'), 'utf8');

// ════════════════════════════════════════════════════════════
// M-1: XSS in marketplace.js — loadFeaturedCollections, loadTopCreators, loadRecentSales
// ════════════════════════════════════════════════════════════
console.log('\n── M-1: marketplace.js XSS fix ──');

assert(marketplaceJs.includes('escapeHtml('), 'M-1.1  escapeHtml helper is used');

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
assert(mkEsc !== null || marketplaceJs.includes('escapeHtml('), 'M-1.17 escapeHtml function parses or shared helper used');
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

assert(profileJs.includes('escapeHtml('), 'M-2.1  escapeHtml helper used');
assert(profileJs.includes('function safeImageUrl('), 'M-2.2  safeImageUrl function added');

// renderNFTGrid escaping — nft.id in hidden onclick uses encodeURIComponent (URL context)
assert(profileJs.includes("encodeURIComponent(nft.id || nft.token || '')"), 'M-2.3  nft.id escaped in onclick');
assert(profileJs.includes("escapeHtml(nft.collection || nft.collection_name || 'Unknown')"), 'M-2.4  nft.collection escaped');
assert(profileJs.includes("escapeHtml(name)"), 'M-2.5  nft name escaped');
assert(profileJs.includes("escapeHtml(price)"), 'M-2.6  nft price escaped');

// loadActivity escaping
assert(profileJs.includes("escapeHtml(String(tokenRef || '-'))"), 'M-2.7  token reference escaped in activity row');
assert(profileJs.includes('escapeHtml(eventItem)'), 'M-2.8  activity item label escaped');
assert(profileJs.includes('escapeHtml(type)'), 'M-2.9  activity type escaped');
assert(profileJs.includes('formatHash(eventFrom, 8)'), 'M-2.10 activity from uses seller/buyer mapping');
assert(profileJs.includes('formatHash(eventTo, 8)'), 'M-2.11 activity to uses seller/buyer mapping');

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

assert(browseJs.includes('escapeHtml('), 'M-3.1  browse.js uses escapeHtml');

// List view escaping — search in the full browse.js source (nft.id in onclick uses escapeJsAttr)
assert(browseJs.includes("escapeJsAttr(nft.id)") || browseJs.includes("encodeURIComponent(nft.id)"), 'M-3.2  list view nft.id escaped in onclick');
assert(browseJs.includes("escapeHtml(nft.collection || 'Unknown')"), 'M-3.3  list view nft.collection escaped');
assert(browseJs.includes("escapeHtml(nft.name || 'NFT #'"), 'M-3.4  list view nft.name escaped (NFT # fallback)');
assert(browseJs.includes("escapeHtml(priceInMolt)"), 'M-3.5  list view price escaped');
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

assert(itemJs.includes('escapeHtml('), 'M-4.1  item.js uses escapeHtml');
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

assert(createJs.includes('escapeHtml('), 'M-5.1  create.js uses escapeHtml');

// showFilePreview escapes file.name
assert(createJs.includes("escapeHtml(file.name)"), 'M-5.2  file.name escaped in preview');
assert(!createJs.includes("+ file.name + ' ('"), 'M-5.3  raw file.name no longer used');

// renderProperties escapes trait values
assert(createJs.includes("escapeHtml(prop.trait_type || '')"), 'M-5.4  trait_type escaped in value attribute');
assert(createJs.includes("escapeHtml(prop.value || '')"), 'M-5.5  prop.value escaped in value attribute');

// Validate escapeHtml handles attribute injection
const createEsc = buildEscapeHtml(createJs);
assert(createEsc !== null || createJs.includes('escapeHtml('), 'M-5.6  escapeHtml available (local or shared)');
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
assert(mkTrendingSection.includes("escapeJsAttr(nft.id)") || mkTrendingSection.includes("escapeHtml(nft.id)"), 'CC-1  loadTrendingNFTs nft.id is escaped');
assert(mkTrendingSection.includes("escapeHtml(nft.collection)"), 'CC-2  loadTrendingNFTs nft.collection is escaped');

// browse.js grid view (already fixed in prior audit) still works
const gridViewSection = browseJs.slice(browseJs.indexOf("currentView === 'grid'"));
assert(gridViewSection.includes("escapeJsAttr(nft.id)") || gridViewSection.includes("escapeHtml(nft.id)"), 'CC-3  grid view nft.id is escaped');
assert(gridViewSection.includes("escapeHtml(nft.collection || 'Unknown')"), 'CC-4  grid view nft.collection is escaped');

// item.js renderNFTDetail uses escapeHtml for image src
assert(itemJs.includes("escapeHtml(imageUrl)"), 'CC-5  renderNFTDetail image src escaped');
assert(itemJs.includes("escapeHtml(nft.name || 'NFT')"), 'CC-6  renderNFTDetail image alt escaped');

// item.js renderProperties uses escapeHtml
assert(itemJs.includes("escapeHtml(prop.trait_type || prop.key || 'Unknown')"), 'CC-7  renderProperties trait_type escaped');
assert(itemJs.includes("escapeHtml(prop.value || '-')"), 'CC-8  renderProperties value escaped');

// All 5 JS files have escapeHtml
assert(marketplaceJs.includes('escapeHtml('), 'CC-9   marketplace.js uses escapeHtml');
assert(browseJs.includes('escapeHtml('), 'CC-10  browse.js uses escapeHtml');
assert(itemJs.includes('escapeHtml('), 'CC-11  item.js uses escapeHtml');
assert(createJs.includes('escapeHtml('), 'CC-12  create.js uses escapeHtml');
assert(profileJs.includes('escapeHtml('), 'CC-13  profile.js uses escapeHtml');

// ════════════════════════════════════════════════════════════
// M-8: Multi-criteria filters — browse.js wiring
// ════════════════════════════════════════════════════════════
console.log('\n── M-8: Multi-criteria filter wiring ──');

// buildFilterParams passes price range, sort_by, collection, rarity to RPC
assert(browseJs.includes('function buildFilterParams()'), 'M-8.1  buildFilterParams function exists');
assert(browseJs.includes('params.price_min'), 'M-8.2  price_min passed to RPC');
assert(browseJs.includes('params.price_max'), 'M-8.3  price_max passed to RPC');
assert(browseJs.includes('params.sort_by'), 'M-8.4  sort_by passed to RPC');
assert(browseJs.includes('params.collection'), 'M-8.5  collection passed to RPC');
assert(browseJs.includes('params.rarity'), 'M-8.6  rarity passed to RPC');

// loadListings calls buildFilterParams
assert(browseJs.includes('buildFilterParams()'), 'M-8.7  loadListings uses buildFilterParams');
assert(browseJs.includes('dataSource.getAllListings(params)'), 'M-8.8  filter params forwarded to data source');

// View toggle wiring
assert(browseJs.includes('function setView(view)'), 'M-8.9  setView function exists');
assert(browseJs.includes("querySelectorAll('.view-btn')"), 'M-8.10 view-btn click listeners registered');
assert(browseJs.includes("getAttribute('data-view')"), 'M-8.11 data-view attribute read');
assert(browseJs.includes("classList.add('list-view')"), 'M-8.12 list-view class toggled on grid container');

// Price range wiring
assert(browseJs.includes("getElementById('applyPriceBtn')"), 'M-8.13 applyPriceBtn listener');
assert(browseJs.includes("getElementById('minPrice')"), 'M-8.14 minPrice input read');
assert(browseJs.includes("getElementById('maxPrice')"), 'M-8.15 maxPrice input read');

// Rarity filter wiring
assert(browseJs.includes("querySelectorAll('.rarityFilter')"), 'M-8.16 rarity checkbox listeners');
assert(browseJs.includes('selectedRarities'), 'M-8.17 selectedRarities tracked');

// Sort maps to RPC sort_by values
assert(browseJs.includes("params.sort_by = 'price_asc'"), 'M-8.18 price_low maps to price_asc');
assert(browseJs.includes("params.sort_by = 'price_desc'"), 'M-8.19 price_high maps to price_desc');
assert(browseJs.includes("params.sort_by = 'oldest'"), 'M-8.20 oldest sort supported');
assert(browseJs.includes("params.sort_by = 'newest'"), 'M-8.21 newest is default sort');

// "Not Listed" removed — browse only shows active listings
assert(!browseJs.includes('filterNotListed'), 'M-8.22 filterNotListed removed (nonsensical for browse)');

// ════════════════════════════════════════════════════════════
// M-9: Browse filter/pagination/task wiring
// ════════════════════════════════════════════════════════════
console.log('\n── M-9: browse filter and pagination task wiring ──');

assert(browseJs.includes('window.clearFilters = function () {'), 'M-9.1  clearFilters is defined globally for inline button wiring');
assert(browseJs.includes('statusHasOffers = !!hasOffersBox.checked;'), 'M-9.2  has-offers checkbox updates status flag');
assert(browseJs.includes('if (statusHasOffers) params.has_offers = true;'), 'M-9.3  has-offers forwarded to listing RPC params');
assert(browseJs.includes('if (filterMode === \'featured\' || filterMode === \'creators\')'), 'M-9.4  URL filter params featured/creators are parsed');
assert(browseJs.includes("<span class=\"pagination-ellipsis\">…</span>"), 'M-9.5  pagination renders ellipsis for large page ranges');

// ════════════════════════════════════════════════════════════
// M-10: marketplace-data limit argument wiring
// ════════════════════════════════════════════════════════════
console.log('\n── M-10: marketplace-data limit argument wiring ──');

const marketplaceDataJs = fs.readFileSync(path.join(__dirname, '..', 'marketplace', 'js', 'marketplace-data.js'), 'utf8');
assert(marketplaceDataJs.includes('async function getFeaturedCollections(limit)'), 'M-10.1 getFeaturedCollections accepts limit arg');
assert(marketplaceDataJs.includes('async function getTrendingNFTs(limit, period)'), 'M-10.2 getTrendingNFTs accepts limit/period args');
assert(marketplaceDataJs.includes('async function getTopCreators(limit)'), 'M-10.3 getTopCreators accepts limit arg');
assert(marketplaceDataJs.includes('async function getRecentSales(limit)'), 'M-10.4 getRecentSales accepts limit arg');
assert(marketplaceDataJs.includes('Math.max(1, Number(limit) || 6)'), 'M-10.5 featured collections enforces caller limit');
assert(marketplaceDataJs.includes('Math.max(1, Number(limit) || 12)'), 'M-10.6 trending NFTs enforces caller limit');
assert(marketplaceDataJs.includes('Math.max(1, Number(limit) || 8)'), 'M-10.7 top creators enforces caller limit');
assert(marketplaceDataJs.includes('Math.max(1, Number(limit) || 10)'), 'M-10.8 recent sales enforces caller limit');

// ════════════════════════════════════════════════════════════
// M-11: Footer link and royalty cap alignment
// ════════════════════════════════════════════════════════════
console.log('\n── M-11: footer links and royalty cap alignment ──');

const createHtmlForCap = fs.readFileSync(path.join(__dirname, '..', 'marketplace', 'create.html'), 'utf8');
const browseHtmlForLinks = fs.readFileSync(path.join(__dirname, '..', 'marketplace', 'browse.html'), 'utf8');
const profileHtmlForLinks = fs.readFileSync(path.join(__dirname, '..', 'marketplace', 'profile.html'), 'utf8');

assert(createHtmlForCap.includes('id="nftRoyalty"') && createHtmlForCap.includes('max="10"'), 'M-11.1 create royalty input max aligns to 10% cap');
assert(createHtmlForCap.includes('max 10%'), 'M-11.2 create royalty hint text reflects 10% cap');
assert(createJs.includes('royalty < 0 || royalty > 10'), 'M-11.3 create.js enforces 0–10 royalty validation');

assert(!browseHtmlForLinks.includes('href="#docs"') && !browseHtmlForLinks.includes('href="#api"') && !browseHtmlForLinks.includes('href="#help"') && !browseHtmlForLinks.includes('href="#terms"'),
    'M-11.4 browse footer resource placeholders removed');
assert(!createHtmlForCap.includes('href="#docs"') && !createHtmlForCap.includes('href="#api"') && !createHtmlForCap.includes('href="#help"') && !createHtmlForCap.includes('href="#terms"'),
    'M-11.5 create footer resource placeholders removed');
assert(!profileHtmlForLinks.includes('href="#docs"') && !profileHtmlForLinks.includes('href="#api"') && !profileHtmlForLinks.includes('href="#help"') && !profileHtmlForLinks.includes('href="#terms"'),
    'M-11.6 profile footer resource placeholders removed');

assert(createHtmlForCap.includes('Max 50MB'), 'M-11.7 create upload hint shows 50MB max to match validator');
assert(createJs.includes('file.size > 50 * 1024 * 1024'), 'M-11.8 create file upload enforces 50MB max');

// List view outputs proper structure with escapeHtml
assert(browseJs.includes('nft-list-header'), 'M-8.23 list view has table header');
assert(browseJs.includes('nft-list-item'), 'M-8.24 list view uses nft-list-item');
assert(browseJs.includes('nft-list-thumb'), 'M-8.25 list view has thumbnail');
assert(browseJs.includes('nft-list-col-price'), 'M-8.26 list view has price column');

// Collection search wiring
assert(browseJs.includes("getElementById('collectionSearch')"), 'M-8.27 collection search filter wired');

// ════════════════════════════════════════════════════════════
// M-9: marketplace-data.js filter param forwarding
// ════════════════════════════════════════════════════════════
console.log('\n── M-9: marketplace-data.js filter forwarding ──');

const dataJs = fs.readFileSync(path.join(__dirname, '..', 'marketplace', 'js', 'marketplace-data.js'), 'utf8');
assert(dataJs.includes('async function getAllListings(limitOrOpts)'), 'M-9.1  getAllListings accepts object param');
assert(dataJs.includes("typeof limitOrOpts === 'object'"), 'M-9.2  object vs number param detection');
assert(dataJs.includes("rpcCall('getMarketListings', [params])"), 'M-9.3  params forwarded to RPC call');

// ════════════════════════════════════════════════════════════
// M-10: browse.html structure tests
// ════════════════════════════════════════════════════════════
console.log('\n── M-10: browse.html structure ──');

const browseHtml = fs.readFileSync(path.join(__dirname, '..', 'marketplace', 'browse.html'), 'utf8');
assert(!browseHtml.includes('filterNotListed'), 'M-10.1 Not Listed checkbox removed from HTML');
assert(browseHtml.includes('filterHasOffers'), 'M-10.2 Has Offers checkbox present');
assert(browseHtml.includes('rarityFilter'), 'M-10.3 rarity checkboxes present');
assert(browseHtml.includes('data-view="grid"'), 'M-10.4 grid view toggle present');
assert(browseHtml.includes('data-view="list"'), 'M-10.5 list view toggle present');
assert(browseHtml.includes('value="oldest"'), 'M-10.6 oldest sort option present');
assert(browseHtml.includes('minPrice'), 'M-10.7 min price input present');
assert(browseHtml.includes('maxPrice'), 'M-10.8 max price input present');
assert(browseHtml.includes('applyPriceBtn'), 'M-10.9 apply price button present');

// ════════════════════════════════════════════════════════════
// M-11: CSS consistency — DEX button styles ported
// ════════════════════════════════════════════════════════════
console.log('\n── M-11: CSS consistency ──');

const marketCss = fs.readFileSync(path.join(__dirname, '..', 'marketplace', 'css', 'marketplace.css'), 'utf8');
assert(marketCss.includes('background: var(--orange-primary)'), 'M-11.1 btn-primary uses flat orange (no gradient)');
assert(marketCss.includes('background: var(--orange-dark)'), 'M-11.2 btn-primary hover uses orange-dark');
assert(marketCss.includes('box-shadow: var(--shadow-glow)'), 'M-11.3 btn-primary hover has glow');
assert(marketCss.includes('background: var(--bg-hover)'), 'M-11.4 btn-secondary uses bg-hover');
assert(marketCss.includes('border-color: var(--orange-primary)'), 'M-11.5 btn-secondary hover uses orange border');
assert(!marketCss.includes('.btn-small.btn-primary'), 'M-11.6 separate btn-small.btn-primary removed (uses base .btn-primary now)');
assert(marketCss.includes('.nft-list-item'), 'M-11.7 list view CSS exists');
assert(marketCss.includes('.nft-list-header'), 'M-11.8 list header CSS exists');
assert(marketCss.includes('.nft-list-thumb'), 'M-11.9 list thumbnail CSS exists');
assert(marketCss.includes('.nfts-grid.list-view'), 'M-11.10 list-view variant exists');
assert(marketCss.includes('.rarity.legendary'), 'M-11.11 rarity legend CSS exists');
assert(marketCss.includes('.browse-empty'), 'M-11.12 browse empty state CSS exists');

// ════════════════════════════════════════════════════════════
// M-12: Gas removal from create page (BUG-1)
// ════════════════════════════════════════════════════════════
console.log('\n── M-12: Gas removal from create page ──');

assert(!createJs.includes('GAS_ESTIMATE'), 'M-12.1 GAS_ESTIMATE constant removed');
assert(!createJs.includes('gasEstimate'), 'M-12.2 No gasEstimate references remain');
assert(!createJs.includes('Gas Estimate'), 'M-12.3 No "Gas Estimate" label in JS');

const createHtml = fs.readFileSync(path.join(__dirname, '..', 'marketplace', 'create.html'), 'utf8');
assert(!createHtml.includes('Gas Estimate'), 'M-12.4 No "Gas Estimate" in create.html');
assert(!createHtml.includes('~0.001 MOLT'), 'M-12.5 No hardcoded gas amount in HTML');
assert(createHtml.includes('0.500 MOLT'), 'M-12.6 Total cost is 0.500 MOLT (no gas)');

// ════════════════════════════════════════════════════════════
// M-13: Listing price on create page (BUG-3)
// ════════════════════════════════════════════════════════════
console.log('\n── M-13: Listing price on create page ──');

assert(createHtml.includes('nftListingPrice'), 'M-13.1 Listing price input exists in HTML');
assert(createHtml.includes('Set a price to list for sale'), 'M-13.2 Listing price hint text present');
assert(createHtml.includes('previewPrice'), 'M-13.3 Preview price element exists');
assert(createHtml.includes('Not for sale'), 'M-13.4 Default preview says "Not for sale"');

assert(createJs.includes('nftListingPrice'), 'M-13.5 create.js reads listing price input');
assert(createJs.includes("buildContractCallData('list_nft'"), 'M-13.6 Auto-list_nft called after mint');
assert(createJs.includes('previewPrice'), 'M-13.7 Live preview updates listing price');

// ════════════════════════════════════════════════════════════
// M-14: Contract — Royalty enforcement (BUG-4)
// ════════════════════════════════════════════════════════════
console.log('\n── M-14: Contract royalty enforcement ──');

const contractRs = fs.readFileSync(path.join(__dirname, '..', 'contracts', 'moltmarket', 'src', 'lib.rs'), 'utf8');
assert(contractRs.includes('royalty_bps'), 'M-14.1 royalty_bps field in contract');
assert(contractRs.includes('LISTING_SIZE: usize = 147'), 'M-14.2 Listing layout is 147 bytes');
assert(contractRs.includes('royalty_amount'), 'M-14.3 Royalty amount computed in buy_nft');
assert(contractRs.includes('seller_amount = price - fee_amount - royalty_amount'), 'M-14.4 3-way split in buy_nft');
assert(contractRs.includes('list_nft_with_royalty'), 'M-14.5 list_nft_with_royalty function exists');

// ════════════════════════════════════════════════════════════
// M-15: Contract — Auction system
// ════════════════════════════════════════════════════════════
console.log('\n── M-15: Contract auction system ──');

assert(contractRs.includes('AUCTION_SIZE'), 'M-15.1 AUCTION_SIZE constant defined');
assert(contractRs.includes('create_auction'), 'M-15.2 create_auction function exists');
assert(contractRs.includes('place_bid'), 'M-15.3 place_bid function exists');
assert(contractRs.includes('settle_auction'), 'M-15.4 settle_auction function exists');
assert(contractRs.includes('cancel_auction'), 'M-15.5 cancel_auction function exists');
assert(contractRs.includes('get_auction'), 'M-15.6 get_auction function exists');
assert(contractRs.includes('Anti-sniping') || contractRs.includes('anti-sniping') || contractRs.includes('ANTI_SNIPE') || contractRs.includes('anti_snipe'), 'M-15.7 Anti-sniping protection');
assert(contractRs.includes('reserve_price'), 'M-15.8 Reserve price support');

// ════════════════════════════════════════════════════════════
// M-16: Contract — Collection offers
// ════════════════════════════════════════════════════════════
console.log('\n── M-16: Contract collection offers ──');

assert(contractRs.includes('COLLECTION_OFFER_SIZE'), 'M-16.1 COLLECTION_OFFER_SIZE constant defined');
assert(contractRs.includes('make_collection_offer'), 'M-16.2 make_collection_offer function exists');
assert(contractRs.includes('accept_collection_offer'), 'M-16.3 accept_collection_offer function exists');
assert(contractRs.includes('cancel_collection_offer'), 'M-16.4 cancel_collection_offer function exists');

// ════════════════════════════════════════════════════════════
// M-17: Contract — Offer expiry
// ════════════════════════════════════════════════════════════
console.log('\n── M-17: Contract offer expiry ──');

assert(contractRs.includes('OFFER_EXPIRY_SIZE'), 'M-17.1 OFFER_EXPIRY_SIZE constant defined');
assert(contractRs.includes('make_offer_with_expiry'), 'M-17.2 make_offer_with_expiry function exists');
assert(contractRs.includes('expiry'), 'M-17.3 expiry field used in offers');

// ════════════════════════════════════════════════════════════
// M-18: Core — MarketActivityKind expansion
// ════════════════════════════════════════════════════════════
console.log('\n── M-18: Core activity kinds ──');

const coreMarketplace = fs.readFileSync(path.join(__dirname, '..', 'core', 'src', 'marketplace.rs'), 'utf8');
assert(coreMarketplace.includes('AuctionCreated'), 'M-18.1 AuctionCreated kind exists');
assert(coreMarketplace.includes('AuctionBid'), 'M-18.2 AuctionBid kind exists');
assert(coreMarketplace.includes('AuctionSettled'), 'M-18.3 AuctionSettled kind exists');
assert(coreMarketplace.includes('AuctionCancelled'), 'M-18.4 AuctionCancelled kind exists');
assert(coreMarketplace.includes('CollectionOffer'), 'M-18.5 CollectionOffer kind exists');
assert(coreMarketplace.includes('CollectionOfferAccepted'), 'M-18.6 CollectionOfferAccepted kind exists');
assert(coreMarketplace.includes('OfferAccepted'), 'M-18.7 OfferAccepted kind exists');
assert(coreMarketplace.includes('OfferCancelled'), 'M-18.8 OfferCancelled kind exists');
assert(coreMarketplace.includes('PriceUpdate'), 'M-18.9 PriceUpdate kind exists');
assert(coreMarketplace.includes('Transfer'), 'M-18.10 Transfer kind exists');

// ════════════════════════════════════════════════════════════
// M-19: RPC — New endpoints
// ════════════════════════════════════════════════════════════
console.log('\n── M-19: RPC new endpoints ──');

const rpcLib = fs.readFileSync(path.join(__dirname, '..', 'rpc', 'src', 'lib.rs'), 'utf8');
assert(rpcLib.includes('getMarketOffers'), 'M-19.1 getMarketOffers endpoint registered');
assert(rpcLib.includes('getMarketAuctions'), 'M-19.2 getMarketAuctions endpoint registered');
assert(rpcLib.includes('handle_get_market_offers'), 'M-19.3 handle_get_market_offers handler exists');
assert(rpcLib.includes('handle_get_market_auctions'), 'M-19.4 handle_get_market_auctions handler exists');

// Activity kind string mapping in RPC
assert(rpcLib.includes('"auction_created"') || rpcLib.includes('"AuctionCreated"'), 'M-19.5 AuctionCreated kind mapped in RPC');
assert(rpcLib.includes('"auction_bid"') || rpcLib.includes('"AuctionBid"'), 'M-19.6 AuctionBid kind mapped in RPC');
assert(rpcLib.includes('"offer"') || rpcLib.includes('"Offer"'), 'M-19.7 Offer kind mapped in RPC');

// ════════════════════════════════════════════════════════════
// M-20: Validator — Function-to-kind mapping
// ════════════════════════════════════════════════════════════
console.log('\n── M-20: Validator function mapping ──');

const validatorMain = fs.readFileSync(path.join(__dirname, '..', 'validator', 'src', 'main.rs'), 'utf8');
assert(validatorMain.includes('create_auction'), 'M-20.1 create_auction mapped in validator');
assert(validatorMain.includes('place_bid'), 'M-20.2 place_bid mapped in validator');
assert(validatorMain.includes('settle_auction'), 'M-20.3 settle_auction mapped in validator');
assert(validatorMain.includes('cancel_auction'), 'M-20.4 cancel_auction mapped in validator');
assert(validatorMain.includes('make_collection_offer'), 'M-20.5 make_collection_offer mapped in validator');
assert(validatorMain.includes('accept_collection_offer'), 'M-20.6 accept_collection_offer mapped in validator');
assert(validatorMain.includes('cancel_collection_offer'), 'M-20.7 cancel_collection_offer mapped in validator');
assert(validatorMain.includes('make_offer_with_expiry'), 'M-20.8 make_offer_with_expiry mapped in validator');
assert(validatorMain.includes('list_nft_with_royalty'), 'M-20.9 list_nft_with_royalty mapped in validator');
assert(validatorMain.includes('update_listing_price'), 'M-20.10 update_listing_price mapped in validator');

// ════════════════════════════════════════════════════════════
// M-21: Item page offers display
// ════════════════════════════════════════════════════════════
console.log('\n── M-21: Item page offers ──');

assert(itemJs.includes('loadOffers'), 'M-21.1 loadOffers function exists in item.js');
assert(itemJs.includes('offersList'), 'M-21.2 offersList container referenced');
assert(itemJs.includes('_itemAcceptOffer'), 'M-21.3 Accept offer handler exists');
assert(itemJs.includes("rpcCall('getMarketOffers'"), 'M-21.4 Calls getMarketOffers RPC');

const itemHtml = fs.readFileSync(path.join(__dirname, '..', 'marketplace', 'item.html'), 'utf8');
assert(itemHtml.includes('offersList'), 'M-21.5 Offers panel in item.html');
assert(itemHtml.includes('fa-hand-holding-usd'), 'M-21.6 Offers icon present');

// ════════════════════════════════════════════════════════════
// M-22: Profile page offers tab
// ════════════════════════════════════════════════════════════
console.log('\n── M-22: Profile page offers tab ──');

const profileHtml = fs.readFileSync(path.join(__dirname, '..', 'marketplace', 'profile.html'), 'utf8');
assert(profileHtml.includes('data-tab="offers"'), 'M-22.1 Offers tab button in profile.html');
assert(profileHtml.includes('id="offers-tab"'), 'M-22.2 Offers tab content panel exists');
assert(profileHtml.includes('offersTable'), 'M-22.3 offersTable element exists');

assert(profileJs.includes('loadProfileOffers'), 'M-22.4 loadProfileOffers function exists');
assert(profileJs.includes('_profileAcceptOffer'), 'M-22.5 Accept offer handler in profile');
assert(profileJs.includes('_profileCancelOffer'), 'M-22.6 Cancel offer handler in profile');
assert(profileJs.includes("rpcCall('getMarketOffers'"), 'M-22.7 Profile calls getMarketOffers RPC');
assert(profileJs.includes('window.location.href = nextUrl') || profileJs.includes('window.location.href = \"profile.html?id='), 'M-22.8 Profile wallet switch redirects to active wallet profile');
assert(profileJs.includes('loadProfile();'), 'M-22.9 Profile reloads content on wallet connect/disconnect');

// ════════════════════════════════════════════════════════════
// M-23: Contract — Core marketplace CRUD functions
// ════════════════════════════════════════════════════════════
console.log('\n── M-23: Contract core CRUD ──');

assert(contractRs.includes('pub extern "C" fn initialize('), 'M-23.1  initialize function exported');
assert(contractRs.includes('pub extern "C" fn list_nft('), 'M-23.2  list_nft function exported');
assert(contractRs.includes('pub extern "C" fn buy_nft('), 'M-23.3  buy_nft function exported');
assert(contractRs.includes('pub extern "C" fn cancel_listing('), 'M-23.4  cancel_listing function exported');
assert(contractRs.includes('pub extern "C" fn get_listing('), 'M-23.5  get_listing function exported');
assert(contractRs.includes('pub extern "C" fn make_offer('), 'M-23.6  make_offer function exported');
assert(contractRs.includes('pub extern "C" fn cancel_offer('), 'M-23.7  cancel_offer function exported');
assert(contractRs.includes('pub extern "C" fn accept_offer('), 'M-23.8  accept_offer function exported');

// ════════════════════════════════════════════════════════════
// M-24: Contract — Admin & query functions
// ════════════════════════════════════════════════════════════
console.log('\n── M-24: Contract admin & query ──');

assert(contractRs.includes('pub extern "C" fn set_marketplace_fee('), 'M-24.1  set_marketplace_fee exported');
assert(contractRs.includes('pub extern "C" fn get_marketplace_stats('), 'M-24.2  get_marketplace_stats exported');
assert(contractRs.includes('pub extern "C" fn set_nft_attributes('), 'M-24.3  set_nft_attributes exported');
assert(contractRs.includes('pub extern "C" fn get_nft_attributes('), 'M-24.4  get_nft_attributes exported');
assert(contractRs.includes('pub extern "C" fn get_offer_count('), 'M-24.5  get_offer_count exported');
assert(contractRs.includes('pub extern "C" fn update_listing_price('), 'M-24.6  update_listing_price exported');
assert(contractRs.includes('pub extern "C" fn mm_pause('), 'M-24.7  mm_pause exported');
assert(contractRs.includes('pub extern "C" fn mm_unpause('), 'M-24.8  mm_unpause exported');

// Reentrancy guard used
assert(contractRs.includes('reentrancy_enter()'), 'M-24.9  reentrancy guard enter used');
assert(contractRs.includes('reentrancy_exit()'), 'M-24.10 reentrancy guard exit used');

// Pause check in critical functions
assert(contractRs.includes('is_mm_paused()'), 'M-24.11 pause check used in functions');

// Constants
assert(contractRs.includes('LISTING_SIZE'), 'M-24.12 LISTING_SIZE constant defined');
assert(contractRs.includes('OFFER_SIZE'), 'M-24.13 OFFER_SIZE constant defined');
assert(contractRs.includes('COLLECTION_OFFER_SIZE'), 'M-24.14 COLLECTION_OFFER_SIZE constant defined');
assert(contractRs.includes('AUCTION_SIZE'), 'M-24.15 AUCTION_SIZE constant defined');
assert(contractRs.includes('OFFER_EXPIRY_SIZE'), 'M-24.16 OFFER_EXPIRY_SIZE constant defined');

// No v4 versioning references (pre-launch, everything is v1)
assert(!contractRs.includes('v4:'), 'M-24.17 No v4 section headers in contract');
assert(!contractRs.includes('OFFER_V4'), 'M-24.18 No OFFER_V4 naming in contract');

// ════════════════════════════════════════════════════════════
// M-25: Profile page — deterministic avatar & banner (no editable uploads)
// ════════════════════════════════════════════════════════════
console.log('\n── M-25: Profile avatar & banner ──');

assert(profileJs.includes('generateIdenticon'), 'M-25.1  generateIdenticon function used');
assert(profileJs.includes('bannerGradientFromHash'), 'M-25.2  bannerGradientFromHash for multi-stop banner');
assert(profileJs.includes('<svg'), 'M-25.3  SVG identicon generated');
assert(profileJs.includes('border-radius:50%'), 'M-25.4  Identicon rendered as circle');
assert(profileJs.includes('profileAvatar'), 'M-25.5  profileAvatar element populated');
assert(profileJs.includes('bannerImage'), 'M-25.6  bannerImage element populated');
assert(profileHtml.includes('profile-avatar'), 'M-25.7  profile-avatar in HTML');
assert(profileHtml.includes('profile-banner'), 'M-25.8  profile-banner in HTML');

// No fake edit buttons (no on-chain storage for profile images)
assert(!profileHtml.includes('editBanner'), 'M-25.9  No fake editBanner button');
assert(!profileHtml.includes('editAvatar'), 'M-25.10 No fake editAvatar button');
assert(!profileHtml.includes('editProfileBtn'), 'M-25.11 No fake editProfile button');
assert(!profileJs.includes('editBanner'), 'M-25.12 No editBanner JS handler');
assert(!profileJs.includes('editAvatar'), 'M-25.13 No editAvatar JS handler');
assert(!profileJs.includes('editProfileBtn'), 'M-25.14 No editProfileBtn JS handler');

// ════════════════════════════════════════════════════════════
// M-26: Item.js — Core action handlers
// ════════════════════════════════════════════════════════════
console.log('\n── M-26: Item page action handlers ──');

assert(itemJs.includes('handleListForSale') || itemJs.includes('listForSale') || itemJs.includes('_itemListForSale'), 'M-26.1  List for sale handler exists');
assert(itemJs.includes('handleCancelListing') || itemJs.includes('cancelListing') || itemJs.includes('_itemCancelListing'), 'M-26.2  Cancel listing handler exists');
assert(itemJs.includes('handleBuy') || itemJs.includes('buyNFT') || itemJs.includes('_itemBuy'), 'M-26.3  Buy handler exists');
assert(itemJs.includes('handleMakeOffer') || itemJs.includes('makeOffer') || itemJs.includes('_itemMakeOffer'), 'M-26.4  Make offer handler exists');
assert(itemJs.includes('loadActivity'), 'M-26.5  loadActivity function exists');
assert(itemJs.includes('loadMoreFromCollection'), 'M-26.6  loadMoreFromCollection function exists');
assert(itemJs.includes('checkListingStatus') || itemJs.includes('loadListingStatus'), 'M-26.7  Listing status check exists');
assert(itemJs.includes('loadOffers();'), 'M-26.8  Wallet switch refreshes offers on item page');

// Browse wallet switch should fully reload listings
assert(browseJs.includes('loadListings(); // reload + re-render to refresh wallet-dependent content') || browseJs.includes('loadListings();'), 'M-26.9  Browse wallet switch reloads listings');
assert(itemJs.includes("rpcCall('getMarketListings', [{ collection: collectionId, limit: 100 }])"), 'M-26.10 Listing status uses scoped listing query (collection + limit 100)');
assert(!itemJs.includes("rpcCall('getMarketSales', [{ limit: 500 }])"), 'M-26.11 Listing status avoids full 500-sales scan per item load');
assert(profileJs.includes('sortBy === \'sales\'') && profileJs.includes('sales_count || a.sales || a.sale_count || a.total_sales || 0'), 'M-26.12 Most Sales sort uses sales-count fields, not price');

// ════════════════════════════════════════════════════════════
// M-27: Create.js — Mint flow
// ════════════════════════════════════════════════════════════
console.log('\n── M-27: Create page mint flow ──');

assert(createJs.includes('mintNFT') || createJs.includes('handleMint'), 'M-27.1  Mint function exists');
assert(createJs.includes('MINTING_FEE'), 'M-27.2  MINTING_FEE constant defined');
assert(!createJs.includes('GAS_ESTIMATE'), 'M-27.3  No GAS_ESTIMATE (no gas on MoltChain)');
assert(createJs.includes('resetForm') || createJs.includes('resetMintForm'), 'M-27.4  Reset form function exists');
assert(createJs.includes('updatePriceBreakdown'), 'M-27.5  updatePriceBreakdown function exists');
assert(createJs.includes('setupLivePreview'), 'M-27.6  setupLivePreview function exists');
assert(!createJs.includes("rpcCall('createCollection'"), 'M-27.7  createCollection RPC call removed (nonexistent endpoint)');
assert(!createJs.includes("deploy_nft_collection"), 'M-27.8  deploy_nft_collection fallback removed');
assert(createJs.includes('CREATE_COLLECTION_OPCODE = 6'), 'M-27.9  Uses CreateCollection opcode-6 system instruction');
assert(createJs.includes('MINT_NFT_OPCODE = 7'), 'M-27.10 Uses MintNft opcode-7 system instruction');
assert(createJs.includes('buildCreateCollectionInstructionData'), 'M-27.11 Collection payload uses bincode-compatible encoding');
assert(createJs.includes('buildMintInstructionData'), 'M-27.12 Mint payload uses bincode-compatible encoding');
assert(createJs.includes('storeMetadataOnReef'), 'M-27.13 Metadata storage routed through REEF helper');
assert(createJs.includes("buildContractCallData('store_data'"), 'M-27.14 REEF store_data contract call used for metadata persistence');
assert(!createJs.includes('data:application/json;base64,'), 'M-27.15 Inline data URI metadata removed from mint flow');

// ════════════════════════════════════════════════════════════
// M-28: Program routing — remove [0xFF;32] placeholders
// ════════════════════════════════════════════════════════════
console.log('\n── M-28: Program routing (MKT-C05) ──');

assert(!itemJs.includes('new Uint8Array(32).fill(0xFF)'), 'M-28.1  item.js no hardcoded [0xFF;32] program placeholder');
assert(!profileJs.includes('new Uint8Array(32).fill(0xFF)'), 'M-28.2  profile.js no hardcoded [0xFF;32] program placeholder');
assert(!createJs.includes('new Uint8Array(32).fill(0xFF)'), 'M-28.3  create.js no hardcoded [0xFF;32] program placeholder');
assert(itemJs.includes('CONTRACT_PROGRAM_ID = marketplaceProgram'), 'M-28.4  item.js routes transactions via resolved marketplace program');
assert(profileJs.includes('CONTRACT_PROGRAM_ID = marketplaceProgram'), 'M-28.5  profile.js routes transactions via resolved marketplace program');
assert(createJs.includes('resolveMarketplaceProgram'), 'M-28.6  create.js resolves marketplace program from symbol registry');
assert(createJs.includes('program_id: mp'), 'M-28.7  create.js list_nft path uses resolved marketplace program id');
assert(createJs.includes('program_id: reefProgram'), 'M-28.8  create.js reef storage call uses resolved REEF program id');

// ════════════════════════════════════════════════════════════
// M-29: Offer arg order + royalty listing wiring
// ════════════════════════════════════════════════════════════
console.log('\n── M-29: Offer arg order + royalty listing ──');

assert(profileJs.includes("buildContractCallData('accept_offer', ["), 'M-29.1  profile accept_offer call exists');
assert(profileJs.includes('currentWallet.address,\n                nftContract,\n                tokenId,\n                offerer'), 'M-29.2  profile accept_offer arg order is seller,nft_contract,token_id,offerer');
assert(itemJs.includes("buildContractCallData('list_nft_with_royalty'"), 'M-29.3  item list flow uses list_nft_with_royalty path');
assert(profileJs.includes("buildContractCallData('list_nft_with_royalty'"), 'M-29.4  profile list flow uses list_nft_with_royalty path');
assert(createJs.includes("buildContractCallData('list_nft_with_royalty'"), 'M-29.5  create auto-list uses list_nft_with_royalty path');
assert(itemJs.includes('royaltyBps'), 'M-29.6  item computes royalty basis points for listing');
assert(profileJs.includes('royaltyBps'), 'M-29.7  profile computes royalty basis points for listing');
assert(createJs.includes('royaltyBps'), 'M-29.8  create computes royalty basis points for auto-listing');

// ════════════════════════════════════════════════════════════
// M-30: Offer expiry + backend offer filtering
// ════════════════════════════════════════════════════════════
console.log('\n── M-30: Offer expiry + backend filtering ──');

assert(itemJs.includes('offerExpiryHours'), 'M-30.1  item page wires offer expiry input');
assert(itemJs.includes("buildContractCallData('make_offer_with_expiry'"), 'M-30.2  make_offer_with_expiry is used for offers');
assert(itemJs.includes('expiryTs'), 'M-30.3  expiry timestamp is computed and passed');
assert(itemJs.includes('token_id: currentNFT.token_id'), 'M-30.4  item offers request includes backend token_id filter');
assert(!itemJs.includes('offerItems = offerItems.filter'), 'M-30.5  client-side per-token offer filtering removed');
assert(rpcRs.includes('token_id_filter'), 'M-30.6  RPC getMarketOffers parses token_id filter');
assert(rpcRs.includes('token_filter'), 'M-30.7  RPC getMarketOffers parses token pubkey filter');
assert(rpcRs.includes('filtered.truncate(limit);'), 'M-30.8  RPC truncates filtered offers to requested limit');

// ════════════════════════════════════════════════════════════
// M-31: Update listing price UI wiring
// ════════════════════════════════════════════════════════════
console.log('\n── M-31: Update listing price wiring ──');

assert(itemJs.includes('handleUpdatePrice'), 'M-31.1  item.js has update price handler');
assert(itemJs.includes("buildContractCallData('update_listing_price'"), 'M-31.2  item.js calls update_listing_price');
assert(itemJs.includes('_profileUpdatePrice') || profileJs.includes('_profileUpdatePrice'), 'M-31.3  profile update price action exposed');
assert(profileJs.includes("buildContractCallData('update_listing_price'"), 'M-31.4  profile.js calls update_listing_price');

// ════════════════════════════════════════════════════════════
// M-32: Chain status bar DOM presence
// ════════════════════════════════════════════════════════════
console.log('\n── M-32: Chain status bar DOM ──');

assert(browseHtml.includes('id="chainBlockHeight"'), 'M-32.1  browse.html has chainBlockHeight');
assert(itemHtml.includes('id="chainBlockHeight"'), 'M-32.2  item.html has chainBlockHeight');
assert(createHtml.includes('id="chainBlockHeight"'), 'M-32.3  create.html has chainBlockHeight');
assert(profileHtml.includes('id="chainBlockHeight"'), 'M-32.4  profile.html has chainBlockHeight');

// ════════════════════════════════════════════════════════════
// M-33: Marketplace wsUrl production config
// ════════════════════════════════════════════════════════════
console.log('\n── M-33: Marketplace ws config ──');

assert(marketplaceConfigJs.includes("ws: 'wss://rpc.moltchain.network/ws'"), 'M-33.1  mainnet wsUrl is configured');
assert(marketplaceConfigJs.includes("ws: 'wss://testnet-rpc.moltchain.network/ws'"), 'M-33.2  testnet wsUrl is configured');

// ════════════════════════════════════════════════════════════
// M-34: Backend aggregated marketplace stats
// ════════════════════════════════════════════════════════════
console.log('\n── M-34: Aggregated stats endpoint usage ──');

assert(marketplaceDataJs.includes("rpcCall('getMoltMarketStats'"), 'M-34.1  getStats uses backend getMoltMarketStats');
assert(!marketplaceDataJs.includes("getMarketSales', [{ limit: 500 }]"), 'M-34.2  getStats no longer scans 500 market sales in-browser');
assert(marketplaceDataJs.includes('marketStats.sale_volume'), 'M-34.3  totalVolume sourced from backend aggregated sale_volume');

// ════════════════════════════════════════════════════════════
// M-35: Collection offers + favorites wiring
// ════════════════════════════════════════════════════════════
console.log('\n── M-35: Collection offers + favorites wiring ──');

assert(itemJs.includes("buildContractCallData('make_collection_offer'"), 'M-35.1  item.js wires make_collection_offer call');
assert(itemJs.includes("buildContractCallData('cancel_collection_offer'"), 'M-35.2  item.js wires cancel_collection_offer call');
assert(itemJs.includes("buildContractCallData('accept_collection_offer'"), 'M-35.3  item.js wires accept_collection_offer call');
assert(profileJs.includes("buildContractCallData('make_collection_offer'"), 'M-35.4  profile.js wires make_collection_offer call');
assert(itemJs.includes('include_collection_offers: true'), 'M-35.5  item offers query requests collection offers');
assert(profileJs.includes('include_collection_offers: true'), 'M-35.6  profile offers query requests collection offers');
assert(rpcRs.includes('include_collection_offers'), 'M-35.7  RPC getMarketOffers supports include_collection_offers flag');
assert(itemJs.includes('moltmarket_favorites_v1'), 'M-35.8  item.js persists favorites in wallet-scoped storage');
assert(itemHtml.includes('favoriteToggleBtn'), 'M-35.9  item.html renders favorite toggle button');
assert(profileJs.includes('loadFavoritedNFTs') && profileJs.includes('renderFavoritedGrid'), 'M-35.10 profile favorites tab is implemented with dynamic loading/rendering');

// ════════════════════════════════════════════════════════════
// M-36: Mint authorization + derivation parity hardening
// ════════════════════════════════════════════════════════════
console.log('\n── M-36: Mint authorization + derivation parity ──');

const moltpunksRs = fs.readFileSync(path.join(__dirname, '..', 'contracts', 'moltpunks', 'src', 'lib.rs'), 'utf8');
assert(moltpunksRs.includes('caller.0 == minter.0 || caller.0 == to.0'), 'M-36.1  moltpunks mint allows minter or self-mint caller');
assert(createJs.includes('Invalid collection address for token derivation'), 'M-36.2  create.js validates collection address length before derivation');
assert(createJs.includes('var digest = await sha256Bytes(preimage);'), 'M-36.3  create.js uses shared SHA-256 helper for token account derivation');
assert(createJs.includes("rpcCall('getNFT', [collectionAddress, tokenId])"), 'M-36.4  create.js performs post-mint runtime derivation consistency check');

// ════════════════════════════════════════════════════════════
// M-37: Offer guardrails + collection-offer fee safety
// ════════════════════════════════════════════════════════════
console.log('\n── M-37: Offer guardrails + fee safety ──');

assert(contractRs.includes('MIN_OFFER_PRICE'), 'M-37.1  moltmarket defines minimum offer floor constant');
assert(contractRs.includes('MAX_ACTIVE_OFFERS_PER_WALLET'), 'M-37.2  moltmarket defines per-wallet active offer limit constant');
assert(contractRs.includes('offerer_active_count_key') && contractRs.includes('reserve_offer_slot_if_needed'), 'M-37.3  moltmarket tracks/reserves active offer slots per wallet');
assert(contractRs.includes('Offer price below minimum floor'), 'M-37.4  make_offer paths enforce minimum offer floor');
assert(contractRs.includes('Per-wallet active offer limit reached'), 'M-37.5  make_offer paths enforce per-wallet active offer cap');
assert(contractRs.includes('call_token_transfer(payment_token, offerer, marketplace_addr, price)'), 'M-37.6  accept_collection_offer escrows full payment once in marketplace');
assert(contractRs.includes('call_token_transfer(payment_token, marketplace_addr, offerer, price)'), 'M-37.7  accept_collection_offer refunds escrow if NFT transfer fails');

// ════════════════════════════════════════════════════════════
// M-38: Auction lifecycle wiring + royalty fallback
// ════════════════════════════════════════════════════════════
console.log('\n── M-38: Auction lifecycle wiring + royalty fallback ──');

assert(itemHtml.includes('id="auctionPanel"'), 'M-38.1  item.html includes auction panel container');
assert(itemJs.includes('handleCreateAuction') && itemJs.includes('handlePlaceBid') && itemJs.includes('handleSettleAuction') && itemJs.includes('handleCancelAuction'), 'M-38.2  item.js wires full auction action handlers');
assert(itemJs.includes("buildContractCallData('create_auction'") && itemJs.includes("buildContractCallData('place_bid'") && itemJs.includes("buildContractCallData('settle_auction'") && itemJs.includes("buildContractCallData('cancel_auction'"), 'M-38.3  item.js builds all auction contract calls');
assert(profileJs.includes('_profileCreateAuction') && profileJs.includes('_profilePlaceBid'), 'M-38.4  profile.js exposes auction create/bid actions');
assert(profileJs.includes("buildContractCallData('create_auction'") && profileJs.includes("buildContractCallData('place_bid'"), 'M-38.5  profile.js builds auction create/bid contract calls');
assert(contractRs.includes('Auction royalty transfer failed; paying fallback to seller'), 'M-38.6  settle_auction logs royalty-failure fallback path');
assert(contractRs.includes('call_token_transfer(payment_token, marketplace_addr, seller, royalty_amount)'), 'M-38.7  settle_auction credits seller on royalty transfer failure');

// ════════════════════════════════════════════════════════════
// Summary
// ════════════════════════════════════════════════════════════
console.log(`\n${'═'.repeat(50)}`);
console.log(`Marketplace Audit: ${passed} passed, ${failed} failed (${passed + failed} total)`);
console.log(`${'═'.repeat(50)}`);
process.exit(failed > 0 ? 1 : 0);
