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
    const signatureIndex = source.indexOf(`function ${functionName}(`);
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

console.log(`\nFrontend asset integrity: ${passed} passed, ${failed} failed`);
if (failed > 0) {
    process.exit(1);
}
