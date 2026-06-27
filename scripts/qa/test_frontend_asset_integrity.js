#!/usr/bin/env node
'use strict';

const { spawnSync } = require('child_process');
const fs = require('fs');
const path = require('path');

const repoRoot = path.join(__dirname, '..', '..');

function definePortal(name, excludedRoots = [], requiredStagePaths = []) {
    return {
        name,
        excludedRoots: new Set(excludedRoots),
        requiredStagePaths: new Set(requiredStagePaths),
    };
}

const portals = [
    definePortal('website'),
    definePortal('explorer'),
    definePortal('wallet', ['extension']),
    definePortal('dex', ['loadtest', 'market-maker', 'sdk'], ['charting_library/']),
    definePortal('marketplace'),
    definePortal('programs'),
    definePortal('developers'),
    definePortal('monitoring'),
    definePortal('faucet', ['src']),
];

const CSP_CONNECT_ALLOWLIST = [
    'https://rpc.lichen.network',
    'https://testnet-rpc.lichen.network',
    'wss://rpc.lichen.network',
    'wss://testnet-rpc.lichen.network',
    'https://custody.lichen.network',
    'https://testnet-custody.lichen.network',
    'https://custody-testnet.lichen.network',
    'https://explorer.lichen.network',
    'https://wallet.lichen.network',
    'https://marketplace.lichen.network',
    'https://dex.lichen.network',
    'https://lichen.network',
    'https://developers.lichen.network',
    'https://programs.lichen.network',
    'https://faucet.lichen.network',
    'https://cloudflareinsights.com',
    'https://static.cloudflareinsights.com',
];
const DEX_CRITICAL_ASSET_VERSION = '20260602';

let passed = 0;
let failed = 0;
const gitIgnoreCache = new Map();

function assert(condition, label) {
    if (condition) {
        passed++;
        console.log(`  ✅ ${label}`);
    } else {
        failed++;
        console.log(`  ❌ ${label}`);
    }
}

function toPosix(value) {
    return value.split(path.sep).join('/');
}

function isGitIgnored(absolutePath) {
    const relative = toPosix(path.relative(repoRoot, absolutePath));
    if (!relative || relative.startsWith('..')) {
        return false;
    }

    if (gitIgnoreCache.has(relative)) {
        return gitIgnoreCache.get(relative);
    }

    const result = spawnSync('git', ['check-ignore', relative], {
        cwd: repoRoot,
        encoding: 'utf8',
    });
    const ignored = result.status === 0;
    gitIgnoreCache.set(relative, ignored);
    return ignored;
}

function stripQueryAndHash(ref) {
    return String(ref || '').split('#')[0].split('?')[0];
}

function isExternalRef(ref) {
    return /^(?:[a-z]+:)?\/\//i.test(ref)
        || ref.startsWith('data:')
        || ref.startsWith('blob:')
        || ref.startsWith('mailto:')
        || ref.startsWith('tel:')
        || ref.startsWith('javascript:');
}

function collectHtmlFiles(portal, currentDir, relativeDir, htmlFiles) {
    const entries = fs.readdirSync(currentDir, { withFileTypes: true });
    for (const entry of entries) {
        if (entry.name.startsWith('.')) {
            continue;
        }

        const relativePath = relativeDir ? `${relativeDir}/${entry.name}` : entry.name;
        const topLevel = relativePath.split('/')[0];
        if (portal.name === 'dex' && topLevel === 'charting_library') {
            continue;
        }
        if (portal.excludedRoots.has(topLevel)) {
            continue;
        }

        const absolutePath = path.join(currentDir, entry.name);
        if (entry.isDirectory()) {
            collectHtmlFiles(portal, absolutePath, relativePath, htmlFiles);
            continue;
        }

        if (entry.isFile() && entry.name.endsWith('.html')) {
            htmlFiles.push(absolutePath);
        }
    }
}

function getPortalHtmlFiles(portal) {
    const portalRoot = path.join(repoRoot, portal.name);
    const htmlFiles = [];
    collectHtmlFiles(portal, portalRoot, '', htmlFiles);
    return htmlFiles.sort();
}

function collectPortalSourceFiles(portal, currentDir, relativeDir, sourceFiles) {
    const entries = fs.readdirSync(currentDir, { withFileTypes: true });
    for (const entry of entries) {
        if (entry.name.startsWith('.')) {
            continue;
        }

        const relativePath = relativeDir ? `${relativeDir}/${entry.name}` : entry.name;
        const topLevel = relativePath.split('/')[0];
        if (portal.name === 'dex' && topLevel === 'charting_library') {
            continue;
        }
        if (portal.excludedRoots.has(topLevel)) {
            continue;
        }

        const absolutePath = path.join(currentDir, entry.name);
        if (entry.isDirectory()) {
            if (entry.name === 'node_modules' || entry.name === '__pycache__') continue;
            collectPortalSourceFiles(portal, absolutePath, relativePath, sourceFiles);
            continue;
        }

        if (entry.isFile() && /\.(?:html|js|css)$/.test(entry.name)) {
            sourceFiles.push(absolutePath);
        }
    }
}

function getPortalSourceFiles(portal) {
    const portalRoot = path.join(repoRoot, portal.name);
    const sourceFiles = [];
    collectPortalSourceFiles(portal, portalRoot, '', sourceFiles);
    return sourceFiles.sort();
}

function extractScriptRefs(html) {
    return Array.from(html.matchAll(/<script\b[^>]*\bsrc=(['"])([^'"]+)\1[^>]*>/gi), (match) => ({
        tag: match[0],
        ref: match[2],
    }));
}

function extractLinkRefs(html) {
    const results = [];
    const linkTags = html.match(/<link\b[^>]*>/gi) || [];
    for (const tag of linkTags) {
        const hrefMatch = tag.match(/\bhref=(['"])([^'"]+)\1/i);
        if (!hrefMatch) {
            continue;
        }

        const relMatch = tag.match(/\brel=(['"])([^'"]+)\1/i);
        const relValue = (relMatch ? relMatch[2] : '').toLowerCase();
        const assetLikeRel = relValue.includes('stylesheet')
            || relValue.includes('icon')
            || relValue.includes('manifest')
            || relValue.includes('modulepreload')
            || relValue.includes('preload');

        if (!assetLikeRel) {
            continue;
        }

        results.push({
            tag,
            ref: hrefMatch[2],
        });
    }

    return results;
}

function resolvePortalAsset(portalRoot, pageDir, ref) {
    const cleanRef = stripQueryAndHash(ref);
    if (cleanRef.startsWith('/')) {
        return path.join(portalRoot, cleanRef.slice(1));
    }
    return path.resolve(pageDir, cleanRef);
}

function getPortalRelativeAssetPath(portalRoot, absolutePath) {
    const relative = path.relative(portalRoot, absolutePath);
    return toPosix(relative);
}

function isCoveredByRequiredStagePath(portal, relativeAsset) {
    for (const stagePath of portal.requiredStagePaths) {
        const normalized = toPosix(stagePath);
        if (normalized.endsWith('/')) {
            if (relativeAsset.startsWith(normalized)) {
                return true;
            }
            continue;
        }

        if (relativeAsset === normalized || relativeAsset.startsWith(`${normalized}/`)) {
            return true;
        }
    }

    return false;
}

function validateRequiredStagePaths(portal) {
    if (portal.requiredStagePaths.size === 0) {
        return;
    }

    const portalRoot = path.join(repoRoot, portal.name);
    const missing = [];

    for (const requiredPath of portal.requiredStagePaths) {
        if (!fs.existsSync(path.join(portalRoot, requiredPath))) {
            missing.push(requiredPath);
        }
    }

    assert(missing.length === 0, `${portal.name} staged Pages assets exist locally`);
}

function validateProductionHeaders(portal) {
    const headersPath = path.join(repoRoot, portal.name, '_headers');
    if (!fs.existsSync(headersPath)) {
        return;
    }

    const headers = fs.readFileSync(headersPath, 'utf8');
    const connectSrcDirectives = [...headers.matchAll(/connect-src\s+([^;\n]+)/g)].map(match => match[1]);
    assert(
        !/connect-src[^\n]*(?:http:\/\/localhost|ws:\/\/localhost)/.test(headers),
        `${portal.name}/_headers production connect-src excludes localhost origins`
    );
    assert(
        connectSrcDirectives.length > 0 &&
            connectSrcDirectives.every(directive => {
                const tokens = directive.trim().split(/\s+/);
                return !tokens.includes('https:') && !tokens.includes('wss:');
            }),
        `${portal.name}/_headers production connect-src excludes broad https/wss schemes`
    );
    assert(
        connectSrcDirectives.length > 0 &&
            connectSrcDirectives.every(directive => {
                const tokens = new Set(directive.trim().split(/\s+/));
                return CSP_CONNECT_ALLOWLIST.every(origin => tokens.has(origin));
            }),
        `${portal.name}/_headers production connect-src includes explicit RPC, custody, app, and analytics origins`
    );
    assert(
        !/frame-ancestors[^\n]*http:\/\/localhost/.test(headers),
        `${portal.name}/_headers production frame-ancestors excludes localhost origins`
    );
}

function validateDexCriticalAssetCaching() {
    const dexHtml = fs.readFileSync(path.join(repoRoot, 'dex', 'index.html'), 'utf8');
    const dexHeaders = fs.readFileSync(path.join(repoRoot, 'dex', '_headers'), 'utf8');
    const scriptRefs = extractScriptRefs(dexHtml);
    const criticalScripts = ['shared/utils.js', 'shared-config.js', 'dex.js'];
    const versions = [];

    for (const asset of criticalScripts) {
        const ref = scriptRefs.find(({ ref }) => stripQueryAndHash(ref) === asset)?.ref || '';
        const version = ref.match(/[?&]v=([0-9]{8})\b/)?.[1] || '';
        if (version) versions.push(version);
        assert(
            version >= DEX_CRITICAL_ASSET_VERSION,
            `DEX ${asset} uses a current cache-busting version token`
        );
        assert(
            dexHeaders.includes(`\n/${asset}\n  Cache-Control: no-cache, max-age=0, must-revalidate`),
            `DEX ${asset} has an exact no-cache Pages header`
        );
    }

    assert(
        versions.length === criticalScripts.length && new Set(versions).size === 1,
        'DEX metadata-critical assets share one cache-busting version token'
    );
}

function analyzeAssetRefs(portal, pagePath, refs, kind) {
    const portalRoot = path.join(repoRoot, portal.name);
    const pageDir = path.dirname(pagePath);
    const relativePage = toPosix(path.relative(repoRoot, pagePath));
    const localRefs = refs.filter(({ ref }) => ref && !isExternalRef(ref));
    const seen = new Map();
    const duplicates = [];
    const invalidAssets = [];
    const uncoveredIgnoredAssets = [];

    for (const { ref, tag } of localRefs) {
        const normalizedRef = toPosix(stripQueryAndHash(ref));
        if (seen.has(normalizedRef)) {
            duplicates.push(normalizedRef);
        } else {
            seen.set(normalizedRef, tag);
        }

        const resolved = resolvePortalAsset(portalRoot, pageDir, ref);
        const relativeAsset = getPortalRelativeAssetPath(portalRoot, resolved);
        const topLevel = relativeAsset.split('/')[0];
        const staysInsidePortal = relativeAsset !== '' && !relativeAsset.startsWith('..');
        const pointsToDeployableRoot = staysInsidePortal && !portal.excludedRoots.has(topLevel);
        const assetExists = fs.existsSync(resolved);

        if (!pointsToDeployableRoot || !assetExists) {
            invalidAssets.push(ref);
        }

        if (pointsToDeployableRoot && assetExists && isGitIgnored(resolved) && !isCoveredByRequiredStagePath(portal, relativeAsset)) {
            uncoveredIgnoredAssets.push(relativeAsset);
        }

        if (normalizedRef.endsWith('shared/pq.js')) {
            const isModuleScript = /\btype=(['"])module\1/i.test(tag);
            if (isModuleScript) {
                invalidAssets.push(`${ref} [browser-pq-module-load]`);
            }
        }

        if (normalizedRef.endsWith('.mjs')) {
            const isModuleScript = /\btype=(['"])module\1/i.test(tag);
            if (!isModuleScript) {
                invalidAssets.push(`${ref} [module-script-required]`);
            }
        }
    }

    assert(duplicates.length === 0, `${relativePage} has no duplicate local ${kind} references`);
    assert(invalidAssets.length === 0, `${relativePage} local ${kind} references resolve to deployable assets`);
    assert(uncoveredIgnoredAssets.length === 0, `${relativePage} has no undeclared gitignored local ${kind} refs`);
}

function extractFunctionBody(source, functionName) {
    const signatures = [`function ${functionName}(`, `async function ${functionName}(`];
    const signatureIndex = signatures
        .map((signature) => source.indexOf(signature))
        .filter((index) => index >= 0)
        .sort((a, b) => a - b)[0] ?? -1;
    if (signatureIndex === -1) return '';
    const paramsStart = source.indexOf('(', signatureIndex);
    if (paramsStart === -1) return '';
    let paramsDepth = 0;
    let paramsEnd = -1;
    for (let i = paramsStart; i < source.length; i++) {
        if (source[i] === '(') paramsDepth++;
        if (source[i] === ')') paramsDepth--;
        if (paramsDepth === 0) {
            paramsEnd = i;
            break;
        }
    }
    if (paramsEnd === -1) return '';
    const bodyStart = source.indexOf('{', paramsEnd);
    if (bodyStart === -1) return '';

    let depth = 0;
    for (let i = bodyStart; i < source.length; i++) {
        if (source[i] === '{') depth++;
        if (source[i] === '}') depth--;
        if (depth === 0) {
            return source.slice(bodyStart + 1, i);
        }
    }
    return '';
}

function validateMonitoringIncidentControls() {
    const monitoringRoot = path.join(repoRoot, 'monitoring');
    const html = fs.readFileSync(path.join(monitoringRoot, 'index.html'), 'utf8');
    const js = fs.readFileSync(path.join(monitoringRoot, 'js', 'monitoring.js'), 'utf8');
    const css = fs.readFileSync(path.join(monitoringRoot, 'css', 'monitoring.css'), 'utf8');
    const sharedUtils = fs.readFileSync(path.join(monitoringRoot, 'shared', 'utils.js'), 'utf8');

    const fakeControlHtmlTokens = [
        'killswitch',
        'banList',
        'Operator Actions',
        'killswitchBanIpBtn',
        'killswitchEmergencyShutdownBtn',
    ];
    assert(
        fakeControlHtmlTokens.every((token) => !html.includes(token)),
        'monitoring incident panel exposes no fake browser control buttons'
    );

    const fakeControlJsTokens = [
        'killswitchBanIP',
        'killswitchRateLimit',
        'killswitchBlockMethod',
        'killswitchFreezeAccount',
        'killswitchEmergencyShutdown',
        'killswitchDenyAll',
        'showIncidentControlUnavailable',
        'promptAdminToken',
        'quickBan',
        'quickThrottle',
        'activeBans',
        'addBan',
        'removeBan',
        'renderBans',
        'data-remove-ban',
    ];
    assert(
        fakeControlJsTokens.every((token) => !js.includes(token)),
        'monitoring JavaScript has no placeholder incident mutations or local ban state'
    );

    const fakeControlCssTokens = [
        '.killswitch',
        '.ban-item',
        '.ban-type',
        '.ban-target',
        '.attack-actions',
        '.btn-xs',
    ];
    assert(
        fakeControlCssTokens.every((token) => !css.includes(token)),
        'monitoring CSS has no stale fake incident-control styles'
    );

    assert(
        html.includes('incidentAuthorityGrid') && js.includes('updateIncidentAuthorityBoard'),
        'monitoring incident panel is backed by live incident authority state'
    );

    const recentBlocksBody = extractFunctionBody(js, 'updateRecentBlocks');
    assert(
        recentBlocksBody.includes("rpc('getRecentBlocks'") && !/rpc\(['"]getBlock['"]/.test(recentBlocksBody),
        'monitoring recent blocks use indexed getRecentBlocks instead of per-slot getBlock fanout'
    );

    const oracleBridgeBody = extractFunctionBody(js, 'updateOracleBridgeHealthBoard');
    assert(
        js.includes('NEOGASRWD') && oracleBridgeBody.includes("rpc('getNeoGasRewardsStats'"),
        'monitoring tracks Neo GAS rewards vault registry and health stats'
    );
    assert(
        html.includes('id="ecoWbtcSupply"') &&
            html.includes('id="btcRouteStatus"') &&
            html.includes('id="bridgeWbtcSupply"') &&
            html.includes('id="wbtcReserveAttested"') &&
            html.includes('id="wgasReserveAttested"') &&
            html.includes('id="wneoReserveAttested"') &&
            html.includes('id="btcReserveMatch"') &&
            js.includes("'WBTC'") &&
            oracleBridgeBody.includes("rpc('getWbtcStats'") &&
            oracleBridgeBody.includes("rpc('getBridgeRouteRestrictionStatus', ['bitcoin', 'btc'])") &&
            oracleBridgeBody.includes('wbtcSupply = Number(wbtc?.total_supply ?? wbtc?.supply ?? 0)') &&
            oracleBridgeBody.includes('btcRoute.route_ready !== false') &&
            oracleBridgeBody.includes(": 'CHECK'"),
        'monitoring tracks WBTC contract inventory plus Bitcoin route and reserve health'
    );
    assert(
        sharedUtils.includes('function getSignedMetadataManifestOrNull(') &&
            sharedUtils.includes('function mergeSignedAndLiveRegistryEntries(') &&
            sharedUtils.includes('function getSignedRegistryEntryOrFallback(') &&
            sharedUtils.includes('var liveEntries = await getLiveRegistryEntries(method, fallbackRpcCall)') &&
            sharedUtils.includes('return fallbackRpcCall(method, params || [])'),
        'monitoring signed metadata registry falls back to live registry for newly shipped contracts'
    );
}

function validateMonitoringRiskConsole() {
    const monitoringRoot = path.join(repoRoot, 'monitoring');
    const html = fs.readFileSync(path.join(monitoringRoot, 'index.html'), 'utf8');
    const js = fs.readFileSync(path.join(monitoringRoot, 'js', 'monitoring.js'), 'utf8');
    const css = fs.readFileSync(path.join(monitoringRoot, 'css', 'monitoring.css'), 'utf8');

    const requiredHtmlTokens = [
        'id="riskConsoleForm"',
        'id="riskActionForm"',
        'id="riskTargetType"',
        'id="riskAuthorityType"',
        'id="riskReasonCode"',
        'id="riskRestrictionMode"',
        'id="riskEvidenceHash"',
        'id="riskEvidenceUriHash"',
        'id="riskTtlPolicy"',
        'id="riskCustomTtlSlots"',
        'id="riskSignerForm"',
        'id="riskProposerValue"',
        'id="riskGovernanceAuthorityValue"',
        'id="riskConnectSignerBtn"',
        'id="riskBuildPreviewBtn"',
        'id="riskSignPreviewBtn"',
        'id="riskPreviewPanel"',
        'id="riskSignerNote"',
        'shared/wallet-connect.js',
        'value="account_asset"',
        'value="code_hash"',
        'value="bridge_route"',
        'value="protocol_module"',
        'value="incident_guardian"',
        'value="testnet_drill"',
        'value="guardian_72h"',
        'value="outgoing_only"',
        'value="bidirectional"',
        'id="riskStatusGrid"',
        'id="riskValidationGrid"',
        'id="riskRestrictionList"',
        'id="riskConsoleBadge"',
        'id="riskActionNote"',
    ];
    assert(
        requiredHtmlTokens.every((token) => html.includes(token)),
        'monitoring Risk Console renders target search and status panel controls'
    );

    const riskBody = extractFunctionBody(js, 'fetchRiskConsoleStatus');
    const requiredReadMethods = [
        'listActiveRestrictions',
        'getAccountRestrictionStatus',
        'getAccountAssetRestrictionStatus',
        'getAssetRestrictionStatus',
        'getContractLifecycleStatus',
        'getCodeHashRestrictionStatus',
        'getBridgeRouteRestrictionStatus',
        'getRestrictionStatus',
        'canSend',
        'canReceive',
    ];
    assert(
        requiredReadMethods.every((method) => riskBody.includes(method)),
        'monitoring Risk Console queries shipped read-only restriction RPC methods'
    );

    const forbiddenMutationTokens = [
        'buildRestrict',
        'buildUnrestrict',
        'buildSuspend',
        'buildResume',
        'buildQuarantine',
        'buildTerminate',
        'buildBanCodeHash',
        'buildUnbanCodeHash',
        'buildPauseBridgeRoute',
        'buildResumeBridgeRoute',
        'buildLiftRestriction',
        'buildExtendRestriction',
        'admin_token',
        'LICHEN_ADMIN_TOKEN',
    ];
    assert(
        forbiddenMutationTokens.every((token) => !riskBody.includes(token)),
        'monitoring Risk Console has no unsigned-builder or admin-token mutation path'
    );

    assert(
        js.includes("document.getElementById('riskConsoleForm')?.addEventListener('submit'") &&
            js.includes("document.getElementById('riskTargetType')?.addEventListener('change'") &&
            js.includes("document.getElementById('riskActionForm')?.addEventListener('submit'") &&
            js.includes("document.getElementById('riskConnectSignerBtn')?.addEventListener('click'") &&
            js.includes("document.getElementById('riskBuildPreviewBtn')?.addEventListener('click'") &&
            js.includes("document.getElementById('riskSignPreviewBtn')?.addEventListener('click'") &&
            js.includes("document.getElementById('riskSubmitSignedTxBtn')?.addEventListener('click'") &&
            js.includes("document.getElementById('riskApproveProposalBtn')?.addEventListener('click'") &&
            js.includes("document.getElementById('riskExecuteProposalBtn')?.addEventListener('click'"),
        'monitoring Risk Console binds controls without inline handlers'
    );

    const requiredValidationTokens = [
        'RISK_GUARDIAN_MAX_TTL_SLOTS = 648_000',
        'function riskAuthorityPolicy(',
        'function riskEvidenceStatus(',
        'function riskTtlStatus(',
        'function riskModePolicy(',
        'function riskBuilderInputStatus(',
        'function validateRiskActionContext(',
        'riskAuthorityCheck',
        'riskEvidenceCheck',
        'riskExpiryPreview',
        'riskPolicyCheck',
    ];
    assert(
        requiredValidationTokens.every((token) => js.includes(token)),
        'monitoring Risk Console validates authority, evidence, TTL, and policy context'
    );

    const validationBody = extractFunctionBody(js, 'validateRiskActionContext');
    assert(
        !/rpc\(|fetch\(|build[A-Z]|admin_token|LICHEN_ADMIN_TOKEN/.test(validationBody),
        'monitoring Risk Console validation remains local and non-mutating'
    );

    const previewBody = extractFunctionBody(js, 'riskBuilderPreviewRequest');
    const requiredBuilderMethods = [
        'buildRestrictAccountTx',
        'buildRestrictAccountAssetTx',
        'buildSetFrozenAssetAmountTx',
        'buildSuspendContractTx',
        'buildQuarantineContractTx',
        'buildTerminateContractTx',
        'buildBanCodeHashTx',
        'buildPauseBridgeRouteTx',
        'buildUnrestrictAccountTx',
        'buildUnrestrictAccountAssetTx',
        'buildResumeContractTx',
        'buildUnbanCodeHashTx',
        'buildResumeBridgeRouteTx',
        'buildLiftRestrictionTx',
        'buildExtendRestrictionTx',
    ];
    assert(
        requiredBuilderMethods.every((method) => previewBody.includes(method)),
        'monitoring Risk Console preview routes every shipped create-style restriction builder'
    );

    const buildPreviewBody = extractFunctionBody(js, 'buildRiskTransactionPreview');
    assert(
        buildPreviewBody.includes('rpc(request.method, [request.params])') &&
            !/sendTransaction|sendRawTransaction|licn_sendTransaction|admin_token|LICHEN_ADMIN_TOKEN/.test(buildPreviewBody),
        'monitoring Risk Console builds unsigned previews without submission or admin-token paths'
    );

    const signPreviewBody = extractFunctionBody(js, 'signRiskTransactionPreview');
    assert(
        signPreviewBody.includes('provider.signTransaction(lastRiskPreviewTx.transaction_base64)') &&
            !/sendTransaction|sendRawTransaction|licn_sendTransaction|admin_token|LICHEN_ADMIN_TOKEN/.test(signPreviewBody),
        'monitoring Risk Console signs builder wire previews locally without submitting them'
    );

    const submitPreviewBody = extractFunctionBody(js, 'submitRiskSignedPreview');
    const submitTxBody = extractFunctionBody(js, 'submitRiskSignedTransaction');
    assert(
        submitPreviewBody.includes('lastRiskSignedPreview?.signedTransactionBase64') &&
            submitTxBody.includes("rpc('sendTransaction', [signedBase64])") &&
            !/admin_token|LICHEN_ADMIN_TOKEN/.test(submitPreviewBody + submitTxBody),
        'monitoring Risk Console submits only signed transaction payloads through public RPC'
    );

    const controlBody = extractFunctionBody(js, 'buildRiskGovernanceControlTransaction');
    const runControlBody = extractFunctionBody(js, 'runRiskGovernanceControlAction');
    assert(
        js.includes('const RISK_PROPOSAL_APPROVE_IX = 35') &&
            js.includes('const RISK_PROPOSAL_EXECUTE_IX = 36') &&
            controlBody.includes('data: [instructionType, ...riskU64LeBytes(proposalId)]') &&
            runControlBody.includes('provider.signTransaction(controlTx)') &&
            runControlBody.includes('submitRiskSignedTransaction'),
        'monitoring Risk Console builds signed approval and execution lifecycle transactions'
    );

    assert(
        css.includes('.risk-console-form') &&
            css.includes('.risk-action-form') &&
            css.includes('.risk-status-grid') &&
            css.includes('.risk-validation-grid') &&
            css.includes('.risk-signer-form') &&
            css.includes('.risk-lifecycle-form') &&
            css.includes('.risk-action-button') &&
            css.includes('.risk-preview-panel') &&
            css.includes('.risk-lifecycle-panel') &&
            css.includes('.risk-restriction-card'),
        'monitoring Risk Console has deployable status-panel styling'
    );
}

function validateDexChartPricePrecision() {
    const dexJsPath = path.join(repoRoot, 'dex', 'dex.js');
    const js = fs.readFileSync(dexJsPath, 'utf8');
    const scaleBody = extractFunctionBody(js, 'chartPriceScaleForPair');

    assert(
        scaleBody.includes('isDisplayInvertedPair(pair)) return 100000000') &&
            scaleBody.includes('if (absPrice >= 1000) return 100') &&
            scaleBody.includes('if (absPrice >= 1) return 10000') &&
            scaleBody.includes('if (absPrice >= 0.001) return 1000000'),
        'DEX TradingView price scale matches DEX display precision tiers'
    );

    assert(
        js.includes('resolveSymbol: (name, ok, err) => {') &&
            js.includes('const ps = chartPriceScaleForPair(p);') &&
            !js.includes('p.price < 1 ? 10000 : 100'),
        'DEX TradingView resolveSymbol uses shared chart price-scale helper'
    );

    const syncSizeBody = extractFunctionBody(js, 'syncTradingChartSize');
    const observerBody = extractFunctionBody(js, 'installTradingChartResizeObserver');
    assert(
        js.includes('let tvChartResizeObserver = null') &&
            js.includes('function scheduleTradingChartResize()') &&
            observerBody.includes("typeof ResizeObserver === 'function'") &&
            observerBody.includes('tvChartResizeObserver.observe(el);') &&
            syncSizeBody.includes("tvWidget.resize(width, height)") &&
            syncSizeBody.includes("el.querySelector('iframe')") &&
            js.includes('syncMarginAvailabilityUi();\n        scheduleTradingChartResize();') &&
            !js.includes("window.dispatchEvent(new Event('resize'))") &&
            !js.includes('setTimeout(initTradingView, 200)'),
        'DEX TradingView resizes through container ResizeObserver instead of synthetic window resize workarounds'
    );
}

function validateDexWalletAndPairState() {
    const dexJsPath = path.join(repoRoot, 'dex', 'dex.js');
    const walletConnectPath = path.join(repoRoot, 'dex', 'shared', 'wallet-connect.js');
    const js = fs.readFileSync(dexJsPath, 'utf8');
    const walletConnect = fs.readFileSync(walletConnectPath, 'utf8');

    const nameBody = extractFunctionBody(js, 'formatLichenNameLabel');
    assert(
        nameBody.includes("replace(/(?:\\.lichen)+$/i, '')") &&
            js.includes('state.lichenName = formatLichenNameLabel(reverseResult.name)') &&
            js.includes('const label = formatLichenNameLabel(name); if (label) nameMap[addr] = label;'),
        'DEX normalizes reverse .lichen names without duplicating the suffix'
    );

    const selectPairBody = extractFunctionBody(js, 'selectPair');
    const guardedPriceBody = extractFunctionBody(js, 'setOrderPriceFromMarket');
    assert(
        selectPairBody.includes('setOrderPriceFromMarket(state.lastPrice, { force: options.userInitiated === true });') &&
            guardedPriceBody.includes('inputIsBeingEdited(priceInput)') &&
            guardedPriceBody.includes('formatPriceRaw(price)') &&
            selectPairBody.includes('updateOrderFormPairLabels(pair);') &&
            selectPairBody.includes('calcTotal();') &&
            selectPairBody.includes('updateSubmitBtn();'),
        'DEX pair switch refreshes order price, units, totals, and submit button labels'
    );

    const orderLabelsBody = extractFunctionBody(js, 'updateOrderFormPairLabels');
    const dexHtml = fs.readFileSync(path.join(repoRoot, 'dex', 'index.html'), 'utf8');
    assert(
        orderLabelsBody.includes("setTextById('orderPriceUnit', quote)") &&
            orderLabelsBody.includes("setTextById('orderAmountUnit', base)") &&
            dexHtml.includes('id="orderPriceUnit"') &&
            dexHtml.includes('id="orderAmountUnit"'),
        'DEX order form asset units are data-driven by the active pair'
    );

    assert(
        js.includes('Open P&L: ${pnlSign}$') &&
            !js.includes('dexPortfolioCache') &&
            !js.includes('24h: ${changeSign}$'),
        'DEX portfolio summary avoids fake local 24h deltas'
    );

    const calcTotalBody = extractFunctionBody(js, 'calcTotal');
    assert(
        calcTotalBody.includes('const quotePair = state.activePair;') &&
            calcTotalBody.includes('const quotePairId = state.activePairId;') &&
            calcTotalBody.includes('if (state.activePairId !== quotePairId || state.orderSide !== quoteSide) return;') &&
            calcTotalBody.includes('quotePair.quote ||'),
        'DEX router quote debounce guards against stale pair writes'
    );

    const providerStart = walletConnect.indexOf('PopupLichenProvider.prototype._handlePopupClosed = function ()');
    const providerEnd = walletConnect.indexOf('PopupLichenProvider.prototype._startWindowMonitor', providerStart);
    const providerCloseBody = providerStart >= 0 && providerEnd > providerStart
        ? walletConnect.slice(providerStart, providerEnd)
        : '';
    assert(
        providerCloseBody.includes('pending.reject(new Error(\'Web wallet window closed before the request completed\'))') &&
            providerCloseBody.includes('this._setDisconnected();'),
        'DEX web-wallet provider clears live signing state when the popup is closed'
    );
}

function validateWalletConnectionOriginGuards() {
    for (const relativePath of ['dex/shared/wallet-connect.js', 'programs/shared/wallet-connect.js', 'marketplace/shared/wallet-connect.js']) {
        const js = fs.readFileSync(path.join(repoRoot, relativePath), 'utf8');
        assert(
            js.includes('function isLocalDevelopmentOrigin()') &&
                js.includes('function isLocalWalletOverrideUrl(value)') &&
                js.includes('isLocalDevelopmentOrigin() &&') &&
                js.includes("window.localStorage.getItem('lichen_app_url_wallet')") &&
                js.includes('overrideUrl = isLocalWalletOverrideUrl(candidate) ? candidate : null;'),
            `${relativePath} only honors wallet origin overrides for local development`
        );
    }
}

function validateMarketplaceWalletBridgeParity() {
    const walletConnect = fs.readFileSync(path.join(repoRoot, 'marketplace', 'shared', 'wallet-connect.js'), 'utf8');
    const htmlFiles = ['index.html', 'browse.html', 'create.html', 'item.html', 'profile.html']
        .map((name) => fs.readFileSync(path.join(repoRoot, 'marketplace', name), 'utf8'));

    assert(
        walletConnect.includes('function PopupLichenProvider(') &&
            walletConnect.includes('function getPopupLichenProvider()') &&
            walletConnect.includes("url.searchParams.set('source', getWalletConnectSource());") &&
            walletConnect.includes("url.searchParams.set('network', getSelectedWalletNetwork());"),
        'Marketplace shared wallet utility uses the same popup-backed wallet bridge as DEX'
    );

    assert(
        walletConnect.includes('LichenWallet.prototype.sendTransaction = async function (instructions)') &&
            walletConnect.includes('normalizeRpcInstruction') &&
            walletConnect.includes('provider.sendTransaction'),
        'Marketplace retains its shared NFT transaction signer while delegating approval to the connected provider'
    );

    assert(
        htmlFiles.every((html) => html.includes('data-lichen-source="marketplace"') &&
            html.includes('data-lichen-network-storage-key="lichenmarket_network"')),
        'Marketplace pages load wallet bridge with Marketplace popup source and network context'
    );
}

function validateProgramsWalletBridgeParity() {
    const html = fs.readFileSync(path.join(repoRoot, 'programs', 'playground.html'), 'utf8');
    const js = fs.readFileSync(path.join(repoRoot, 'programs', 'js', 'playground-complete.js'), 'utf8');
    const sdk = fs.readFileSync(path.join(repoRoot, 'programs', 'js', 'lichen-sdk.js'), 'utf8');
    const dexJs = fs.readFileSync(path.join(repoRoot, 'dex', 'dex.js'), 'utf8');

    const helperIndex = html.indexOf('src="shared/wallet-connect.js"');
    const sdkIndex = html.indexOf('src="js/lichen-sdk.js"');
    assert(
        helperIndex !== -1 &&
            sdkIndex !== -1 &&
            helperIndex < sdkIndex &&
            html.includes('data-lichen-source="programs"') &&
            html.includes('data-lichen-network-storage-key="playground_network"'),
        'Programs playground loads the shared DEX wallet bridge before the SDK with Programs popup context'
    );

    assert(
        js.includes("LICHEN_CONFIG.initNetworkSelector(selector, PLAYGROUND_NETWORK_STORAGE_KEY") &&
            js.includes('networkSelectorManagedByConfig = true') &&
            js.includes('this.network = normalizeExplorerNetwork(selector.value || this.network);'),
        'Programs playground network selector is populated from shared network config'
    );

    assert(
        js.includes("await this.connectWalletProvider('extension');") &&
            js.includes("await this.connectWalletProvider('web-wallet');") &&
            js.includes('const response = await connector.connect({ provider: providerType });') &&
            js.includes('this.syncWalletProviderState({ notify: true })'),
        'Programs playground supports DEX-style extension and web-wallet provider actions'
    );

    const dexWalletStateCopy = [
        'Create or Import Web Wallet',
        'Web Wallet Locked',
        'Web Wallet Connected',
        'Reconnect Web Wallet Session',
        'Connect with Web Wallet',
        'Extension Not Detected',
        'Extension Has No Wallet Loaded',
        'Set Up Extension Wallet',
        'Extension Locked',
        'Extension Connected',
        'Reconnect Extension Session',
        'Connect Wallet Extension',
    ];
    assert(
        dexWalletStateCopy.every((text) => dexJs.includes(text) && js.includes(text)) &&
            js.includes("const livePopupSession = providerType !== 'web-wallet' || windowOpen;") &&
            js.includes('const exposedAccounts = livePopupSession ? accounts : [];') &&
            js.includes("const usingWebWallet = providerState.providerType === 'web-wallet';") &&
            js.includes("const activeStateLabel = this.walletCanSign() ? 'Active' : 'Read-only';") &&
            !js.includes('Programs transactions'),
        'Programs wallet modal uses the same DEX provider states, closed-popup handling, and read-only wallet labeling'
    );

    const staleProgramsWalletCopy = [
        'to trade\n                        securely',
        'saved in the DEX',
        'access to this DEX',
        'A saved DEX address',
        'sign orders, approvals, and cancellations',
        'approve orders, approvals, and cancellations',
        'outside the DEX',
        'switch the DEX signer',
        'approve trading requests',
        'approve the DEX connection',
        'The DEX refreshes automatically',
        'the DEX will follow it automatically',
        'trade securely',
    ];
    assert(
        html.includes('deploy\n                        and test programs securely') &&
            js.includes('sign deployments, calls, and metadata updates') &&
            js.includes('approve program requests') &&
            staleProgramsWalletCopy.every((text) => !html.includes(text) && !js.includes(text)),
        'Programs wallet modal copy is program-developer specific while preserving the shared DEX flow'
    );

    assert(
        sdk.includes("wallet?.providerType === 'web-wallet'") &&
            sdk.includes('const popupProvider = getPopupLichenProvider();') &&
            sdk.includes('wallet?.provider && await matchesWalletAddress(wallet.provider)'),
        'Programs SDK resolves popup and injected wallet providers for transaction approval'
    );

    assert(
        sdk.includes('simulation.error || simulation.logs || simulation.returnCode || simulation.return_code'),
        'Programs SDK preflight diagnostics must read simulateTransaction returnCode'
    );

    assert(
        sdk.includes('faucetBaseUrl()') &&
            sdk.includes('getFaucetConfig()') &&
            sdk.includes('`${faucetBase}/faucet/request`') &&
            sdk.includes('payload?.error || payload?.message || response.statusText') &&
            !sdk.includes("this.config.rpc.includes('/rpc')") &&
            js.includes("this.network === 'mainnet' || this.network === 'local-mainnet'") &&
            js.includes('getFaucetConfig().catch(() => null)') &&
            js.includes('max_per_request'),
        'Programs faucet uses the configured faucet service, surfaces JSON errors, respects faucet max, and rejects local mainnet'
    );
}

function validateFrontendInputGuards() {
    const dexJs = fs.readFileSync(path.join(repoRoot, 'dex', 'dex.js'), 'utf8');
    const dexHtml = fs.readFileSync(path.join(repoRoot, 'dex', 'index.html'), 'utf8');
    const programsJs = fs.readFileSync(path.join(repoRoot, 'programs', 'js', 'playground-complete.js'), 'utf8');
    const programsHtml = fs.readFileSync(path.join(repoRoot, 'programs', 'playground.html'), 'utf8');
    const explorerAddressJs = fs.readFileSync(path.join(repoRoot, 'explorer', 'js', 'address.js'), 'utf8');
    const explorerAddressHtml = fs.readFileSync(path.join(repoRoot, 'explorer', 'address.html'), 'utf8');
    const explorerBlocksJs = fs.readFileSync(path.join(repoRoot, 'explorer', 'js', 'blocks.js'), 'utf8');
    const explorerBlocksHtml = fs.readFileSync(path.join(repoRoot, 'explorer', 'blocks.html'), 'utf8');
    const explorerJs = fs.readFileSync(path.join(repoRoot, 'explorer', 'js', 'explorer.js'), 'utf8');
    const faucetJs = fs.readFileSync(path.join(repoRoot, 'faucet', 'faucet.js'), 'utf8');
    const faucetHtml = fs.readFileSync(path.join(repoRoot, 'faucet', 'index.html'), 'utf8');
    const marketplaceBrowseJs = fs.readFileSync(path.join(repoRoot, 'marketplace', 'js', 'browse.js'), 'utf8');
    const marketplaceBrowseHtml = fs.readFileSync(path.join(repoRoot, 'marketplace', 'browse.html'), 'utf8');
    const marketplaceCreateJs = fs.readFileSync(path.join(repoRoot, 'marketplace', 'js', 'create.js'), 'utf8');
    const marketplaceCreateHtml = fs.readFileSync(path.join(repoRoot, 'marketplace', 'create.html'), 'utf8');
    const marketplaceItemJs = fs.readFileSync(path.join(repoRoot, 'marketplace', 'js', 'item.js'), 'utf8');
    const marketplaceProfileJs = fs.readFileSync(path.join(repoRoot, 'marketplace', 'js', 'profile.js'), 'utf8');
    const monitoringJs = fs.readFileSync(path.join(repoRoot, 'monitoring', 'js', 'monitoring.js'), 'utf8');
    const monitoringHtml = fs.readFileSync(path.join(repoRoot, 'monitoring', 'index.html'), 'utf8');
    const websiteJs = fs.readFileSync(path.join(repoRoot, 'website', 'script.js'), 'utf8');
    const walletJs = fs.readFileSync(path.join(repoRoot, 'wallet', 'js', 'wallet.js'), 'utf8');
    const extensionPopupJs = fs.readFileSync(path.join(repoRoot, 'wallet', 'extension', 'src', 'popup', 'popup.js'), 'utf8');
    const extensionFullJs = fs.readFileSync(path.join(repoRoot, 'wallet', 'extension', 'src', 'pages', 'full.js'), 'utf8');

    const nativeNumberHits = portals.flatMap((portal) => (
        getPortalSourceFiles(portal)
            .filter((file) => /type=(["'])number\1|input\[type=(["'])number\2\]/.test(fs.readFileSync(file, 'utf8')))
            .map((file) => toPosix(path.relative(repoRoot, file)))
    ));
    assert(
        nativeNumberHits.length === 0,
        `deployed static frontends avoid native number controls (${nativeNumberHits.join(', ') || 'none found'})`
    );

    const dexNumericGuardBody = extractFunctionBody(dexJs, 'applyDexNumericInputGuards');
    assert(
        dexJs.includes('function sanitizeDexNumberInput(') &&
            !dexHtml.includes('type="number"') &&
            !dexJs.includes('type="number"') &&
            dexHtml.includes('id="orderPrice" placeholder="0.0000" inputmode="decimal"') &&
            dexHtml.includes('id="orderAmount" placeholder="0.00" inputmode="decimal"') &&
            dexHtml.includes('data-dex-numeric="true"') &&
            dexNumericGuardBody.includes('input[data-dex-numeric="true"]') &&
            dexNumericGuardBody.includes("event.key === 'e' || event.key === 'E' || event.key === '+'") &&
            dexNumericGuardBody.includes("event.key === '-' && !dexInputAllowsNegative(input)") &&
            dexNumericGuardBody.includes("event.key === '.' && !dexInputAllowsDecimal(input)") &&
            dexNumericGuardBody.includes("input.addEventListener('paste'"),
        'DEX numeric inputs reject exponent/sign junk and sanitize pasted values'
    );

    assert(
        dexHtml.includes('id="predictTradeHint"') &&
            dexHtml.includes('id="predictCreateHint"') &&
            dexHtml.includes('id="addLiqHint"') &&
            dexHtml.includes('id="proposalSubmitHint"') &&
            dexHtml.includes('id="launchTradeHint"') &&
            dexHtml.includes('id="launchCreateHint"') &&
            dexHtml.includes('id="rewardClaimAllHint"') &&
            dexHtml.includes('id="rewardClaimTradingHint"') &&
            dexHtml.includes('id="rewardClaimLpHint"') &&
            dexHtml.includes('id="predictStatusFilter"') &&
            dexHtml.includes('id="predictPagination"') &&
            dexJs.includes('function getPredictCreateValidation()') &&
            dexJs.includes('function isPredictMarketOpen(') &&
            dexJs.includes('function applyPredictionMarketSort()') &&
            dexJs.includes('function getFilteredPredictionMarkets()') &&
            dexJs.includes('function renderPredictPagination(') &&
            dexJs.includes('function getAddLiquidityValidation()') &&
            dexJs.includes('function updateAddLiquidityButton()') &&
            dexJs.includes('function updateProposalSubmitButton()') &&
            dexJs.includes('function updateRewardsClaimButtons()') &&
            dexJs.includes('function updateLaunchTradeButton()') &&
            dexJs.includes('.btn-predict-resolve, .btn-predict-challenge, .btn-predict-finalize, .btn-predict-claim, .btn-predict-claim-pos') &&
            dexJs.includes("document.querySelectorAll('.margin-close-btn, .cancel-btn')") &&
            dexJs.includes("document.querySelectorAll('.lp-collect-btn, .lp-remove-btn, .lp-add-btn')") &&
            dexJs.includes('No LP positions yet'),
        'DEX prediction, pool, launch, rewards, and governance actions expose validation-driven disabled states'
    );

    const updateSubmitBody = extractFunctionBody(dexJs, 'updateSubmitBtn');
    const syncOrderTypeBody = extractFunctionBody(dexJs, 'syncOrderTypeUi');
    assert(
        dexHtml.includes('id="orderSubmitHint"') &&
            dexHtml.includes('id="marginCollateral"') &&
            updateSubmitBody.includes('walletSigningGateMessage()') &&
            dexJs.includes('Reconnect wallet to sign') &&
            dexJs.includes('Import web wallet to sign') &&
            updateSubmitBody.includes('Margin stop-limit entries are not live yet') &&
            dexJs.includes("if (mode === 'margin') state.orderType = 'limit';") &&
            !syncOrderTypeBody.includes('marginOnlyHidden') &&
            syncOrderTypeBody.includes('btn.hidden = false') &&
            syncOrderTypeBody.includes("btn.style.display = ''") &&
            dexJs.includes("const neededToken = tradeMode === 'margin'") &&
            dexJs.includes("const neededAmount = tradeMode === 'margin'"),
        'DEX order ticket exposes clear wallet gating and keeps margin order tabs visible'
    );

    const dexObserverBody = extractFunctionBody(dexJs, 'observeDexNumericInputGuards');
    assert(
        dexObserverBody.includes('new MutationObserver') &&
            dexObserverBody.includes('applyDexNumericInputGuards(node)') &&
            dexJs.includes('observeDexNumericInputGuards();'),
        'DEX applies numeric guards to dynamically inserted inputs'
    );

    const programsNumericGuardBody = extractFunctionBody(programsJs, 'applyProgramsInputGuards');
    const programsDynamicNumericInputIds = [
        'tokenDecimalsInput',
        'tokenSupplyInput',
        'nftMaxSupplyInput',
        'nftRoyaltyInput',
        'lendCollateralInput',
        'lendLiqThreshInput',
        'lendLiqBonusInput',
        'launchPlatformFeeInput',
        'launchGradMcapInput',
        'vaultPerfFeeInput',
        'vaultMaxStratInput',
        'idInitRepInput',
        'idMaxRepInput',
        'idVouchCostInput',
        'idVouchRewardInput',
        'mktFeeBpsInput',
        'auctDurationInput',
        'auctMinBidInput',
    ];
    assert(
        programsJs.includes('function sanitizeProgramsNumberInput(') &&
            !programsHtml.includes('type="number"') &&
            !programsJs.includes('type="number"') &&
            programsHtml.includes('id="initialFunding"') &&
            programsHtml.includes('id="transferAmount"') &&
            programsHtml.includes('data-programs-numeric="true"') &&
            programsNumericGuardBody.includes('input[data-programs-numeric="true"]') &&
            programsNumericGuardBody.includes("event.key === 'e' || event.key === 'E' || event.key === '+'") &&
            programsNumericGuardBody.includes("event.key === '-' && !programsInputAllowsNegative(input)") &&
            programsNumericGuardBody.includes("event.key === '.' && !programsInputAllowsDecimal(input)") &&
            programsNumericGuardBody.includes("input.addEventListener('paste'") &&
            programsJs.includes('observeProgramsInputGuards();') &&
            programsJs.includes('parseProgramsDecimal(fundingInput)') &&
            programsJs.includes('parseProgramsDecimal(transferAmountInput)') &&
            programsDynamicNumericInputIds.every(id => {
                const idIndex = programsJs.indexOf(`id="${id}"`);
                const guardIndex = programsJs.indexOf('data-programs-numeric="true"', idIndex);
                return idIndex !== -1 && guardIndex !== -1 && guardIndex - idIndex < 220;
            }),
        'Programs numeric inputs reject exponent/sign junk and sanitize pasted values'
    );

    const faucetGuardBody = extractFunctionBody(faucetJs, 'applyFaucetInputGuards');
    assert(
        faucetJs.includes('function sanitizeFaucetBase58(') &&
            faucetHtml.includes('data-address-input="base58"') &&
            faucetGuardBody.includes('sanitizeFaucetBase58(addressInput.value)') &&
            faucetGuardBody.includes("addressInput.addEventListener('paste'"),
        'faucet address input is constrained to base58 characters'
    );

    assert(
        faucetJs.includes('function sanitizeFaucetInteger(') &&
            !faucetHtml.includes('type="number"') &&
            faucetJs.includes('const captchaValue = String') &&
            faucetHtml.includes('inputmode="numeric"') &&
            faucetGuardBody.includes("event.key === 'e' || event.key === 'E' || event.key === '+' || event.key === '-' || event.key === '.'") &&
            faucetGuardBody.includes('sanitizeFaucetInteger(captchaInput.value)'),
        'faucet captcha input accepts integer digits only'
    );

    assert(
        marketplaceCreateJs.includes('var _mintingFeeLoaded = false;') &&
            !marketplaceCreateJs.includes('init\\n    var _mintingFeeLoaded') &&
            marketplaceCreateHtml.includes('data-market-numeric="true"') &&
            marketplaceCreateJs.includes('function applyMarketNumericInputGuards(') &&
            marketplaceCreateJs.includes('parseMarketIntegerRange(supplyInput ? supplyInput.value') &&
            marketplaceCreateJs.includes('listingPriceSporesBig = parseMarketOptionalLicnSpores') &&
            marketplaceCreateJs.includes('marketSporesJsonNumber(listingPriceSporesBig') &&
            !marketplaceCreateHtml.includes('type="number"') &&
            !marketplaceCreateJs.includes('Math.round(listingPrice * 1e9)'),
        'marketplace create page validates mint/list numeric values before signed actions'
    );

    assert(
        marketplaceItemJs.includes('function parseMarketLicnSpores(') &&
            marketplaceItemJs.includes('marketSporesJsonNumber(priceSporesBig') &&
            marketplaceItemJs.includes('listingPriceToSpores(currentListing') &&
            marketplaceItemJs.includes('parseMarketOptionalInteger(expiryHoursText') &&
            marketplaceItemJs.includes('applyMarketNumericInputGuards(actionContainer)') &&
            !marketplaceItemJs.includes('type="number"') &&
            !marketplaceItemJs.includes('Math.round(price * 1e9)') &&
            !marketplaceItemJs.includes('Math.round(parseFloat'),
        'marketplace item page uses exact spores for listing, buying, offers, auctions, and bids'
    );

    assert(
        marketplaceProfileJs.includes('function parseMarketLicnSpores(') &&
            marketplaceProfileJs.includes('listNFTForSale(nft, priceSpores)') &&
            marketplaceProfileJs.includes('marketSporesJsonNumber(amountSpores') &&
            marketplaceProfileJs.includes('updateListingPrice(nft, nextSpores)') &&
            !marketplaceProfileJs.includes('Math.round(parseFloat') &&
            !marketplaceProfileJs.includes('isNaN(parseFloat'),
        'marketplace profile page uses exact spores for listing, offers, auctions, and bids'
    );

    assert(
        marketplaceBrowseHtml.includes('data-market-numeric="true"') &&
            marketplaceBrowseJs.includes('function readPriceFilterValue(') &&
            marketplaceBrowseJs.includes('parseDecimalBaseUnits(raw, 9, label)') &&
            marketplaceBrowseJs.includes('applyMarketNumericInputGuards(document)') &&
            !marketplaceBrowseHtml.includes('type="number"'),
        'marketplace browse price filters reject exponent and malformed decimal input'
    );

    assert(
        explorerAddressJs.includes('function normalizeContractCallArgs(functionName, args, callerAddress)') &&
            explorerAddressJs.includes("case 'register_identity'") &&
            explorerAddressJs.includes("case 'register_name'") &&
            explorerAddressJs.includes("case 'attest_skill'") &&
            explorerAddressJs.includes('utf8ByteLength(name)') &&
            explorerAddressJs.includes('const callArgs = JSON.stringify(normalizeContractCallArgs(functionName, args, callerAddress));'),
        'explorer LichenID actions encode ordered WASM ABI args'
    );

    assert(
        explorerAddressJs.includes("rpcCall('getRecentBlockhash', [])") &&
            explorerAddressJs.includes('Recent blockhash unavailable') &&
            explorerAddressJs.includes('if (endpoint !== currentEndpoint)') &&
            explorerAddressJs.includes('Endpoint cannot be cleared by the current LichenID contract') &&
            explorerAddressJs.includes("throw new Error('No changes to save')"),
        'explorer LichenID profile updates use fresh blockhashes and avoid doomed no-op writes'
    );

    assert(
        explorerAddressJs.includes('function parseExplorerDecimalBaseUnits(') &&
            explorerAddressJs.includes('function buildTransferInstruction(fromAddress, toAddress, amountSpores)') &&
            explorerAddressJs.includes('parseExplorerPositiveSpores(') &&
            explorerAddressJs.includes('explorerJsonSafeNumber(rateSporesBig') &&
            explorerAddressJs.includes("{ value: 10, label: 'Personal' }") &&
            explorerAddressJs.includes('applyExplorerNumericInputGuards(modal)') &&
            !explorerAddressJs.includes('type="number"') &&
            !explorerAddressJs.includes('Math.floor(Number(amountLicn) * 1_000_000_000)'),
        'explorer signed action modals use exact base-unit parsing and full LichenID agent types'
    );

    assert(
        explorerAddressJs.includes('const EXPLORER_NO_BOOTSTRAP_INDEX') &&
            explorerAddressJs.includes('function isSelfFundedBootstrapIndex(value)') &&
            explorerAddressJs.includes('numeric >= EXPLORER_NO_BOOTSTRAP_INDEX_ROUNDED') &&
            explorerAddressJs.includes('function buildBootstrapRecoveryDisplay(rewards, stakingStatus = null)') &&
            explorerAddressJs.includes('hasRecoverySchedule = debt > 0 || earned > 0 || hasGraduationSlot') &&
            explorerAddressJs.includes('const isSelfFunded = isSelfFundedBootstrapIndex(stakingStatus?.bootstrap_index)') &&
            explorerAddressJs.includes('if (isSelfFunded || !hasRecoverySchedule)') &&
            explorerAddressJs.includes("label: '0.0%'") &&
            explorerAddressHtml.includes('id="bootstrapRecoverySection"') &&
            explorerAddressJs.includes("document.getElementById('bootstrapRecoverySection')") &&
            explorerAddressJs.includes("recoverySection.style.display = recovery.hasRecoverySchedule ? 'block' : 'none'") &&
            explorerAddressJs.includes("rpcCall('getStakingStatus', [address]).catch(() => null)") &&
            explorerAddressJs.includes("document.getElementById('rewardsDebt').textContent = formatLicn(debt)") &&
            !explorerAddressJs.includes("'No bootstrap grant'") &&
            explorerAddressJs.includes("document.getElementById('rewardsVestingText').textContent = recovery.label"),
        'explorer validator bootstrap recovery keeps numeric debt rows, hides true self-funded validators, and does not render zero recovery as 100% graduated'
    );

    assert(
        explorerBlocksHtml.includes('data-explorer-integer="true"') &&
            explorerBlocksJs.includes('function applyBlockIntegerInputGuards(') &&
            explorerBlocksJs.includes("['e', 'E', '+', '-', '.'].includes(event.key)") &&
            !explorerBlocksHtml.includes('type="number"'),
        'explorer block filters use guarded integer text inputs'
    );

    assert(
        monitoringHtml.includes('data-monitoring-integer="true"') &&
            monitoringJs.includes('function applyMonitoringIntegerInputGuards(') &&
            monitoringJs.includes("['e', 'E', '+', '-', '.'].includes(event.key)") &&
            monitoringJs.includes("if (!/^\\d+$/.test(raw))") &&
            monitoringJs.includes("return { error: 'Amount must be a non-negative integer.' };") &&
            !monitoringHtml.includes('type="number"'),
        'monitoring risk console rejects decimal/exponent truncation in integer blockchain inputs'
    );

    assert(
        explorerJs.includes('async submitTransaction(txData)') &&
            explorerJs.includes('const simulation = await this.simulateTransaction(txData);') &&
            websiteJs.includes('async submitTransaction(txData)') &&
            websiteJs.includes('const simulation = await this.simulateTransaction(txData);'),
        'website and explorer RPC clients preflight before transaction submission'
    );

    assert(
        explorerAddressJs.includes("rpcCall('getStakingPosition', [address])") &&
            explorerAddressJs.includes("rpcCall('getUnstakingQueue', [address])") &&
            explorerAddressHtml.includes('Estimated Total Value (LICN)') &&
            explorerAddressHtml.includes('MossStake Redeemable Value') &&
            explorerAddressHtml.includes('Pending MossStake Unstake') &&
            explorerAddressHtml.includes('stLICN Balance') &&
            explorerAddressJs.includes('`${formatLicnExact(stLicn)} stLICN`') &&
            explorerAddressJs.includes('totalAccountValueLicn') &&
            explorerAddressJs.includes('pendingUnstakeLicn') &&
            explorerAddressJs.includes('mossValue'),
        'explorer account summary shows native balance, total account value, MossStake redeemable value, stLICN receipt balance, and pending unstake separately'
    );

    assert(
        walletJs.includes("'ProposeGovernedTransfer': 'Governance Proposal'") &&
            walletJs.includes('`Pending ${amount} LICN`') &&
            walletJs.includes("'ApproveGovernedTransfer': 'Governance Approval'") &&
            extensionPopupJs.includes("'ProposeGovernedTransfer': 'Governance Proposal'") &&
            extensionPopupJs.includes('`Pending ${amt} LICN`') &&
            extensionFullJs.includes("'ProposeGovernedTransfer': 'Governance Proposal'") &&
            extensionFullJs.includes('`Pending ${amt} LICN`') &&
            explorerAddressJs.includes("'ProposeGovernedTransfer': 'Gov. Proposal'") &&
            explorerAddressJs.includes('isGovernanceTransferProposal') &&
            explorerAddressJs.includes('`Pending ${formatLicnExact(txAmount)} LICN`'),
        'wallet, extension, and explorer render governed transfer proposals as pending governance activity, not received funds'
    );
}

console.log('\n── Frontend Asset Integrity ──');

for (const portal of portals) {
    const htmlFiles = getPortalHtmlFiles(portal);
    assert(htmlFiles.length > 0, `${portal.name} contributes deployed HTML pages to the asset scan`);
    validateRequiredStagePaths(portal);
    validateProductionHeaders(portal);

    for (const pagePath of htmlFiles) {
        const html = fs.readFileSync(pagePath, 'utf8');
        analyzeAssetRefs(portal, pagePath, extractScriptRefs(html), 'script');
        analyzeAssetRefs(portal, pagePath, extractLinkRefs(html), 'link');
    }
}

validateMonitoringIncidentControls();
validateMonitoringRiskConsole();
validateDexChartPricePrecision();
validateDexCriticalAssetCaching();
validateDexWalletAndPairState();
validateWalletConnectionOriginGuards();
validateMarketplaceWalletBridgeParity();
validateProgramsWalletBridgeParity();
validateFrontendInputGuards();

console.log(`\nFrontend asset integrity: ${passed} passed, ${failed} failed`);
if (failed > 0) {
    process.exit(1);
}
