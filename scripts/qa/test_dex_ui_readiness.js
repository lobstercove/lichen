#!/usr/bin/env node
'use strict';

const fs = require('fs');
const path = require('path');

const repoRoot = path.join(__dirname, '..', '..');
const dexHtml = fs.readFileSync(path.join(repoRoot, 'dex', 'index.html'), 'utf8');
const dexJs = fs.readFileSync(path.join(repoRoot, 'dex', 'dex.js'), 'utf8');

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
assert(updateSubmitBtn.includes('walletCanSign()') && updateSubmitBtn.includes('Reconnect wallet to sign'), 'trade submit button gates read-only wallet state');
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

if (failed > 0) {
    console.error(`\nDEX UI readiness: ${passed} passed, ${failed} failed`);
    process.exit(1);
}

console.log(`\nDEX UI readiness: ${passed} passed, 0 failed`);
