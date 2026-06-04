#!/usr/bin/env node
'use strict';

const fs = require('fs');
const path = require('path');

const repoRoot = path.join(__dirname, '..', '..');
const dexHtml = fs.readFileSync(path.join(repoRoot, 'dex', 'index.html'), 'utf8');
const dexCss = fs.readFileSync(path.join(repoRoot, 'dex', 'dex.css'), 'utf8');
const dexJs = fs.readFileSync(path.join(repoRoot, 'dex', 'dex.js'), 'utf8');
const ciWorkflow = fs.readFileSync(path.join(repoRoot, '.github', 'workflows', 'ci.yml'), 'utf8');

let passed = 0;
let failed = 0;

function assert(condition, label) {
    if (condition) {
        passed++;
        console.log(`PASS ${label}`);
    } else {
        failed++;
        console.log(`FAIL ${label}`);
    }
}

function extractFunctionBody(source, functionName) {
    const marker = `function ${functionName}(`;
    const start = source.indexOf(marker);
    if (start === -1) return '';
    const open = source.indexOf('{', start);
    if (open === -1) return '';
    let depth = 0;
    for (let i = open; i < source.length; i++) {
        const ch = source[i];
        if (ch === '{') depth++;
        if (ch === '}') {
            depth--;
            if (depth === 0) return source.slice(open + 1, i);
        }
    }
    return '';
}

function extractFunctionDeclaration(source, functionName) {
    const marker = `function ${functionName}(`;
    const start = source.indexOf(marker);
    if (start === -1) return '';
    const open = source.indexOf('{', start);
    if (open === -1) return '';
    let depth = 0;
    for (let i = open; i < source.length; i++) {
        const ch = source[i];
        if (ch === '{') depth++;
        if (ch === '}') {
            depth--;
            if (depth === 0) return source.slice(start, i + 1);
        }
    }
    return '';
}

function countMatches(source, pattern) {
    return (source.match(pattern) || []).length;
}

function buildDexSanitizerHarness() {
    const negativeIds = dexJs.match(/const DEX_NEGATIVE_ALLOWED_INPUT_IDS = new Set\(\[[\s\S]*?\]\);/)?.[0] || '';
    const positiveIds = dexJs.match(/const DEX_STRICT_POSITIVE_INPUT_IDS = new Set\(\[[\s\S]*?\]\);/)?.[0] || '';
    const allowsNegative = extractFunctionDeclaration(dexJs, 'dexInputAllowsNegative');
    const allowsDecimal = extractFunctionDeclaration(dexJs, 'dexInputAllowsDecimal');
    const sanitizer = extractFunctionDeclaration(dexJs, 'sanitizeDexNumberInput');
    if (!negativeIds || !positiveIds || !allowsNegative || !allowsDecimal || !sanitizer) return null;
    return new Function(`${negativeIds}\n${positiveIds}\n${allowsNegative}\n${allowsDecimal}\n${sanitizer}\nreturn { sanitizeDexNumberInput };`)();
}

function makeDexInput(id, value, dataset = {}) {
    return {
        id,
        value,
        dataset: { dexNumeric: 'true', ...dataset },
        min: dataset.min ?? '',
        max: dataset.max ?? '',
        step: dataset.step ?? '',
    };
}

console.log('\nDEX UI Readiness');

const requiredStaticNumericIds = [
    'orderPrice',
    'stopPrice',
    'orderAmount',
    'orderTotal',
    'slippageCustom',
    'predictAmount',
    'predictInitLiq',
    'liqMinPrice',
    'liqMaxPrice',
    'liqAmountA',
    'liqAmountB',
    'propMakerFee',
    'propTakerFee',
    'propParamValue',
    'launchAmountInput',
];

assert(!dexHtml.includes('type="number"') && !dexJs.includes('type="number"'), 'DEX has no native number inputs');
assert(requiredStaticNumericIds.every((id) => new RegExp(`id="${id}"[^>]*data-dex-numeric="true"`).test(dexHtml)), 'static DEX numeric inputs use guarded text controls');
const walletGateInputRule = dexCss.match(/\.wallet-gated-disabled input,[\s\S]*?\.wallet-gated-disabled textarea \{[\s\S]*?\}/)?.[0] || '';
const walletGateModeRule = dexCss.match(/\.wallet-gated-disabled \.preset-btn,[\s\S]*?\.wallet-gated-disabled \.predict-toggle-btn \{[\s\S]*?\}/)?.[0] || '';
assert(
    walletGateInputRule.includes('pointer-events: auto')
        && !walletGateInputRule.includes('pointer-events: none')
        && walletGateModeRule.includes('pointer-events: auto')
        && !walletGateModeRule.includes('pointer-events: none'),
    'wallet gate keeps editable DEX controls interactive while signing actions stay gated'
);

const sanitizerHarness = buildDexSanitizerHarness();
const sanitizerCases = [
    ['orderPrice', '0.14', false, '0.14', {}],
    ['orderAmount', '.12', false, '0.12', {}],
    ['orderAmount', '0.1.2', false, '0.12', {}],
    ['orderPrice', '1e-+4', false, '14', {}],
    ['propMakerFee', '-1', true, '-1', { min: '-100', max: '100', decimal: 'false' }],
    ['propTakerFee', '1.5', false, '15', { min: '0', max: '100', decimal: 'false' }],
    ['launchAmountInput', '0', true, '', {}],
];
assert(
    !!sanitizerHarness && sanitizerCases.every(([id, raw, finalize, expected, dataset]) => {
        const input = makeDexInput(id, raw, dataset);
        sanitizerHarness.sanitizeDexNumberInput(input, finalize);
        return input.value === expected;
    }),
    'DEX numeric sanitizer preserves valid decimals and rejects invalid characters'
);
let transientResetRestoresLastValid = false;
let intentionalStrictPositiveZeroClears = false;
if (sanitizerHarness) {
    const transientInput = makeDexInput('orderPrice', '0.15', { userEdited: '1' });
    sanitizerHarness.sanitizeDexNumberInput(transientInput, false);
    transientInput.value = '0';
    sanitizerHarness.sanitizeDexNumberInput(transientInput, true);
    transientResetRestoresLastValid = transientInput.value === '0.15';

    const zeroInput = makeDexInput('orderPrice', '0.15', { userEdited: '1' });
    sanitizerHarness.sanitizeDexNumberInput(zeroInput, false);
    zeroInput.value = '0';
    sanitizerHarness.sanitizeDexNumberInput(zeroInput, false);
    sanitizerHarness.sanitizeDexNumberInput(zeroInput, true);
    intentionalStrictPositiveZeroClears = zeroInput.value === '';
}
assert(transientResetRestoresLastValid && intentionalStrictPositiveZeroClears, 'DEX strict-positive sanitizer distinguishes transient resets from intentional zero input');

const dynamicNumericClasses = [
    'edit-price-input',
    'edit-qty-input',
    'sltp-sl-input',
    'sltp-tp-input',
    'pclose-custom-input',
    'margin-adjust-input',
];

assert(
    dynamicNumericClasses.every((klass) => new RegExp(`data-dex-numeric="true"[^>]*class="${klass}"|class="${klass}"[^>]*data-dex-numeric="true"`).test(dexJs)),
    'dynamic order and margin numeric inputs use guarded text controls'
);
assert(dexJs.includes('id="marginCloseLimitPriceInput" type="text"') && dexJs.includes('id="marginCloseQtyInput" type="text"'), 'margin close modal numeric controls are guarded text inputs');

const updateSubmitBtn = extractFunctionBody(dexJs, 'updateSubmitBtn');
const syncOrderTypeUi = extractFunctionBody(dexJs, 'syncOrderTypeUi');
const updateMarginInfo = extractFunctionBody(dexJs, 'updateMarginInfo');
const readWalletProviderSnapshot = extractFunctionBody(dexJs, 'readWalletProviderSnapshot');
assert(
    updateSubmitBtn.includes('walletCanSign()')
        && updateSubmitBtn.includes('walletSigningGateMessage()')
        && dexJs.includes('Reconnect wallet to sign')
        && dexJs.includes('Import web wallet to sign'),
    'trade submit button gates read-only wallet state'
);
assert(
    dexJs.includes('function walletIsConnected()')
        && updateSubmitBtn.includes('walletIsConnected()')
        && dexJs.includes("if (!walletIsConnected()) return { ok: false, error: 'Connect wallet first', code: 'NO_WALLET' };")
        && dexJs.includes('await syncDexWithExtensionState({ timeoutMs: 400 });'),
    'trade submit preflight syncs provider state and uses shared wallet connection readiness'
);
assert(
    dexJs.includes('function ensureWalletSigningReady(options = {})')
        && dexJs.includes('function requestWalletProviderConnection(providerType = null)')
        && updateSubmitBtn.includes('const recoverableWalletGate = connected && !canSign')
        && updateSubmitBtn.includes('submitBtn.disabled = Boolean(disabledReason) && !recoverableWalletGate')
        && dexJs.includes('const ready = await ensureWalletSigningReady({ notify: true, timeoutMs: 400 });'),
    'read-only connected trade submit stays clickable and reacquires signer approval before preflight'
);
assert(
    readWalletProviderSnapshot.includes('livePopupSession')
        && readWalletProviderSnapshot.includes('exposedAccounts')
        && readWalletProviderSnapshot.includes("providerType !== 'web-wallet' || windowOpen"),
    'web wallet signing readiness requires a live popup session, not cached popup state'
);
assert(
    dexJs.includes('function webWalletNeedsWalletSetup(providerState = lastExtensionProviderState)')
        && readWalletProviderSnapshot.includes("Object.prototype.hasOwnProperty.call(providerState, 'hasWallet')")
        && dexJs.includes('Import web wallet to sign')
        && dexJs.includes('Web Wallet Account Missing')
        && dexJs.includes('A saved DEX address is not enough to sign.'),
    'web wallet reconnect distinguishes an empty popup from a locked or inactive signer'
);
assert(
    dexJs.includes('restoreWalletConnectionState(currentAddress, reconnectProviderType)')
        && dexJs.includes('const needsConnectionReconcile = !walletIsConnected()')
        && dexJs.includes('await connectWalletTo(currentAddress, shortWalletAddress(currentAddress),'),
    'provider sync repairs stale DEX connected state before showing signing-ready actions'
);
assert(
    dexJs.includes("providerState.hasWallet === false")
        && dexJs.includes("providerType: 'extension'")
        && dexJs.includes('Switched away from the empty web-wallet popup.'),
    'provider sync falls back to a ready matching extension when saved web-wallet state is empty'
);
assert(
    dexJs.includes('function setOrderPriceFromMarket(')
        && dexJs.includes('inputIsBeingEdited(priceInput)')
        && dexJs.includes("priceInput.dataset.userEdited = '1'")
        && dexJs.includes('selectPair(pair, { userInitiated: true })'),
    'market refreshes do not overwrite active trade price edits'
);
assert(
    dexJs.includes('function getAvailableBalance(')
        && updateSubmitBtn.includes('getAvailableBalance(quoteSymbol)')
        && updateSubmitBtn.includes('getAvailableBalance(baseSymbol)')
        && dexJs.includes('const available = getAvailableBalance(neededToken)')
        && dexJs.includes("return getAvailableBalance('lUSD')")
        && dexJs.includes("getAvailableBalance('LICN')"),
    'trade, prediction, pool, and launch validations use normalized balances'
);
assert(
    dexJs.includes('function buildOpenPositionLimitArgs(')
        && dexJs.includes('function buildOpenPositionArgs(')
        && dexJs.includes("if (mode === 'margin') state.orderType = 'limit';")
        && !syncOrderTypeUi.includes("state.tradeMode === 'margin' && state.orderType !== 'limit'")
        && !syncOrderTypeUi.includes("state.tradeMode === 'margin' && btn.dataset.type !== 'limit'")
        && syncOrderTypeUi.includes('btn.hidden = false')
        && syncOrderTypeUi.includes("btn.style.display = ''")
        && dexJs.includes('buildOpenPositionLimitArgs(wallet.address')
        && dexJs.includes('buildOpenPositionArgs(wallet.address')
        && dexJs.includes("effectiveOrderType === 'market'")
        && !dexJs.includes('Margin entries are market-only')
        && !syncOrderTypeUi.includes('marginMarketOnly'),
    'margin ticket keeps spot-style order type tabs with limit as the default'
);
assert(
    updateMarginInfo.includes('marginEntry')
        && updateMarginInfo.includes('marginCollateral')
        && updateMarginInfo.includes('marginLiqPrice')
        && updateMarginInfo.includes('marginRatio')
        && updateMarginInfo.includes('referencePrice')
        && updateMarginInfo.includes('notional'),
    'margin risk summary is driven by amount, price, collateral, and liquidation fields'
);
assert(!dexJs.includes('localStorage.dexPortfolioCache'), 'portfolio summary does not show fake local 24h deltas');

const applyWalletGateAll = extractFunctionBody(dexJs, 'applyWalletGateAll');
assert(applyWalletGateAll.includes('.vote-btn, .finalize-btn, .execute-btn'), 'governance lifecycle buttons share wallet signing gate');
assert(
    dexHtml.includes('id="propVotingPeriod"')
        && dexHtml.includes('id="govDefaultVotingPeriod"')
        && dexHtml.includes('data-current="100" data-unit="x"')
        && dexJs.includes('DEFAULT_PROTOCOL_PARAMS')
        && dexJs.includes('syncGovernanceProtocolUi()')
        && dexJs.includes('currentProposalVotingSlots()')
        && dexJs.includes('new ArrayBuffer(105)')
        && dexJs.includes('writeU64LE(v, 97, currentProposalVotingSlots())')
        && dexJs.includes('new ArrayBuffer(53)')
        && dexJs.includes('writeU64LE(v, 45, currentProposalVotingSlots())'),
    'governance defaults are data-synced and proposal voting period is encoded on-chain'
);
assert(applyWalletGateAll.includes('.btn-predict-resolve, .btn-predict-challenge, .btn-predict-finalize, .btn-predict-claim, .btn-predict-claim-pos'), 'prediction lifecycle buttons share wallet signing gate');
assert(applyWalletGateAll.includes("document.querySelectorAll('.margin-close-btn, .cancel-btn')") && applyWalletGateAll.includes('btn.disabled = !canSign'), 'cancel and margin close actions require signing readiness');
assert(applyWalletGateAll.includes('.launch-quick-buy, .launch-quick-sell') && applyWalletGateAll.includes('updateLaunchTradeButton()') && applyWalletGateAll.includes('updateLaunchCreateButton()'), 'launchpad quick and primary actions share launch validation');

const validationFunctions = [
    'getPredictCreateValidation',
    'updatePredictSubmitButton',
    'getAddLiquidityValidation',
    'updateAddLiquidityButton',
    'getProposalSubmitValidation',
    'updateProposalSubmitButton',
    'getLaunchTradeValidation',
    'getLaunchCreateValidation',
    'updateRewardsClaimButtons',
];
assert(validationFunctions.every((fn) => dexJs.includes(`function ${fn}(`)), 'all DEX action groups have validation/update functions');

const predictionSort = extractFunctionBody(dexJs, 'applyPredictionMarketSort');
assert(
    predictionSort.includes('isPredictMarketOpen')
        && predictionSort.includes('activeRank')
        && predictionSort.includes('statusDelta')
        && predictionSort.includes('return statusDelta'),
    'prediction market sort keeps open markets first'
);
assert(
    dexHtml.includes('id="predictStatusFilter"')
        && dexHtml.includes('id="predictPagination"')
        && dexHtml.includes('id="predictPrevPage"')
        && dexHtml.includes('id="predictNextPage"')
        && dexJs.includes('function getFilteredPredictionMarkets(')
        && dexJs.includes('function renderPredictPagination(')
        && dexJs.includes("statusFilter === 'open'")
        && dexJs.includes('predictState.marketPage = 1'),
    'prediction market list exposes status filtering and pagination'
);
assert(dexJs.includes('data-market-open="${canTradeMarket ?') && applyWalletGateAll.includes("btn.dataset.marketOpen !== '0'"), 'closed prediction market buy buttons stay disabled after wallet gating');
assert(
    dexJs.includes('No LP positions yet')
        && dexJs.includes('Unable to load LP positions')
        && applyWalletGateAll.includes('.lp-collect-btn, .lp-remove-btn, .lp-add-btn')
        && dexJs.includes('const canAdd = walletCanSign()'),
    'pool positions distinguish connected empty/load-failure states and require signing for LP actions'
);
assert(dexHtml.includes('rewardClaimAllHint') && dexHtml.includes('rewardClaimTradingHint') && dexHtml.includes('rewardClaimLpHint'), 'reward claim actions expose per-button reasons');

assert(countMatches(dexHtml, /class="order-submit-hint"/g) >= 9, 'DEX exposes inline action hints across trade, predict, pool, governance, launch, and rewards');
assert(
    !/\bapi\.post\((?!['"]\/router\/quote['"])/.test(dexJs)
        && !/\bapi\.(?:del|delete|put|patch)\(/.test(dexJs)
        && !dexJs.includes("api.post('/router/swap'")
        && !dexJs.includes('api.post("/router/swap"'),
    'DEX frontend uses REST only for read/quote paths, not mutating writes'
);
assert(
    dexJs.includes('prepareTokenPull(spotEscrowSymbol(state.orderSide, state.activePair), contracts.dex_core, escrowRaw)')
        && dexJs.includes('prepareTokenPull(MARGIN_COLLATERAL_SYMBOL, contracts.dex_margin, marginDeposit)')
        && dexJs.includes('prepareTokenPull(tokenA, contracts.dex_amm, rawA)')
        && dexJs.includes('prepareTokenPull(tokenB, contracts.dex_amm, rawB)')
        && dexJs.includes("prepareTokenPull('lUSD', contracts.prediction_market, tradeAmountRaw)")
        && dexJs.includes("namedCallIx(contracts.sporepump, 'buy'")
        && dexJs.includes("namedCallIx(contracts.sporepump, 'sell'")
        && dexJs.includes("namedCallIx(contracts.sporepump, 'create_token'"),
    'DEX frontend pulls native/token collateral before signed trade, margin, pool, prediction, and launch actions'
);
assert(
    ciWorkflow.includes('node scripts/qa/test_dex_ui_readiness.js')
        && ciWorkflow.includes('node scripts/qa/audit_frontend_rpc_parity.js'),
    'CI runs DEX frontend wiring and RPC parity audits'
);

if (failed > 0) {
    console.error(`\nDEX UI readiness: ${passed} passed, ${failed} failed`);
    process.exit(1);
}

console.log(`\nDEX UI readiness: ${passed} passed, 0 failed`);
