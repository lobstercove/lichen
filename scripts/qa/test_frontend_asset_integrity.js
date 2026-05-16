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
    const bodyStart = source.indexOf('{', signatureIndex);
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

console.log('\n── Frontend Asset Integrity ──');

for (const portal of portals) {
    const htmlFiles = getPortalHtmlFiles(portal);
    assert(htmlFiles.length > 0, `${portal.name} contributes deployed HTML pages to the asset scan`);
    validateRequiredStagePaths(portal);

    for (const pagePath of htmlFiles) {
        const html = fs.readFileSync(pagePath, 'utf8');
        analyzeAssetRefs(portal, pagePath, extractScriptRefs(html), 'script');
        analyzeAssetRefs(portal, pagePath, extractLinkRefs(html), 'link');
    }
}

validateMonitoringIncidentControls();
validateMonitoringRiskConsole();

console.log(`\nFrontend asset integrity: ${passed} passed, ${failed} failed`);
if (failed > 0) {
    process.exit(1);
}
