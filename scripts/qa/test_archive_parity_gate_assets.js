#!/usr/bin/env node
'use strict';

const fs = require('fs');
const path = require('path');
const { spawnSync } = require('child_process');
const { builtinModules } = require('module');

const repoRoot = path.join(__dirname, '..', '..');
const harnessPath = 'tests/local-multi-validator-test.sh';
const entrypoints = [
    'tests/e2e-volume.js',
    'tests/e2e-launchpad.js',
];
const supportPaths = [
    'tests/archive-v2-https-source.py',
];
const policyPaths = [
    'docs/deployment/ARCHIVE_PARITY_REPAIR_PLAN_2026-07-09.md',
    'docs/deployment/ARCHIVE_V2_ACTIVATION_CADENCE_AND_VALIDATOR_LIVENESS_PLAN_2026-08-18.md',
    'docs/deployment/TESTNET_STATE_AND_SYNC_POLICY.md',
];

let passed = 0;
let failed = 0;

function assert(condition, label) {
    if (condition) {
        passed += 1;
        console.log(`  PASS ${label}`);
    } else {
        failed += 1;
        console.error(`  FAIL ${label}`);
    }
}

function repoPath(relativePath) {
    return path.join(repoRoot, relativePath);
}

function isFile(relativePath) {
    try {
        return fs.statSync(repoPath(relativePath)).isFile();
    } catch {
        return false;
    }
}

function isTracked(relativePath) {
    return spawnSync(
        'git',
        ['ls-files', '--error-unmatch', '--', relativePath],
        { cwd: repoRoot, stdio: 'ignore' },
    ).status === 0;
}

function isIgnored(relativePath) {
    return spawnSync(
        'git',
        ['check-ignore', '--no-index', '-q', '--', relativePath],
        { cwd: repoRoot, stdio: 'ignore' },
    ).status === 0;
}

function verifySource(relativePath) {
    assert(isFile(relativePath), `${relativePath} exists`);
    assert(isTracked(relativePath), `${relativePath} is Git-tracked`);
    assert(!isIgnored(relativePath), `${relativePath} is not ignored`);
}

function resolveLocalRequire(fromPath, request) {
    const base = path.posix.normalize(path.posix.join(path.posix.dirname(fromPath), request));
    for (const candidate of [base, `${base}.js`, path.posix.join(base, 'index.js')]) {
        if (isFile(candidate)) {
            return candidate;
        }
    }
    return null;
}

function packageNameFromRequest(request) {
    if (request.startsWith('@')) {
        return request.split('/').slice(0, 2).join('/');
    }
    return request.split('/')[0];
}

function collectLocalDependencies(entrypoint) {
    const pending = [entrypoint];
    const visited = new Set();
    const missing = [];
    const packages = new Map();
    const builtins = new Set([
        ...builtinModules,
        ...builtinModules.map((moduleName) => `node:${moduleName}`),
    ]);

    while (pending.length > 0) {
        const relativePath = pending.pop();
        if (visited.has(relativePath)) {
            continue;
        }
        visited.add(relativePath);

        const source = fs.readFileSync(repoPath(relativePath), 'utf8');
        const executableSource = source
            .replace(/\/\*[\s\S]*?\*\//g, '')
            .replace(/^\s*\/\/.*$/gm, '');
        for (const match of executableSource.matchAll(/(?:require|import)\(\s*['"]([^'"]+)['"]\s*\)/g)) {
            const request = match[1];
            if (request.startsWith('./') || request.startsWith('../')) {
                const resolved = resolveLocalRequire(relativePath, request);
                if (!resolved) {
                    missing.push(`${relativePath}: ${request}`);
                } else if (!visited.has(resolved)) {
                    pending.push(resolved);
                }
            } else if (!builtins.has(request)) {
                const packageName = packageNameFromRequest(request);
                const importers = packages.get(packageName) || [];
                importers.push(`${relativePath}: ${request}`);
                packages.set(packageName, importers);
            }
        }
    }

    return { dependencies: Array.from(visited).sort(), missing, packages };
}

function extractWorkflowJob(workflow, jobName) {
    const marker = `  ${jobName}:`;
    const start = workflow.indexOf(marker);
    if (start === -1) {
        return '';
    }
    const remainder = workflow.slice(start + marker.length);
    const nextJob = remainder.search(/\n  [a-zA-Z0-9_-]+:\s*\n/);
    return nextJob === -1
        ? workflow.slice(start)
        : workflow.slice(start, start + marker.length + nextJob);
}

const harness = fs.readFileSync(repoPath(harnessPath), 'utf8');
verifySource(harnessPath);
const finalizedSpreadStart = harness.indexOf('wait_for_cluster_finalized_spread() {');
const finalizedSpreadEnd = harness.indexOf('\nget_validator_count() {', finalizedSpreadStart);
const finalizedSpreadFunction = finalizedSpreadStart >= 0 && finalizedSpreadEnd > finalizedSpreadStart
    ? harness.slice(finalizedSpreadStart, finalizedSpreadEnd)
    : '';
const finalizedSpreadCalls = harness.match(/^\s{4}wait_for_cluster_finalized_spread /gm) || [];
assert(
    harness.includes('rpc_query_params "$1" "getSlot" \'["finalized"]\'')
        && harness.includes('get_health_frontier_with_retry() {')
        && finalizedSpreadFunction.includes('get_health_frontier_with_retry')
        && finalizedSpreadFunction.includes('probe_pids[validator_num]=$!')
        && finalizedSpreadFunction.includes('finalized_spread <= max_spread')
        && finalizedSpreadFunction.includes('maximum_lag <= max_spread')
        && finalizedSpreadCalls.length >= 4,
    'every Archive V2 stop path concurrently converges health-published authoritative finalized frontiers and tip lag',
);
assert(
    harness.includes('managed process ${managed_pid} exited while waiting for cluster readiness')
        && harness.includes('[[ -n "$managed_pid" ]] && ! kill -0 "$managed_pid" 2>/dev/null')
        && harness.includes('local validator_num managed_pid'),
    'cluster readiness fails immediately when a validator managed by the gate exits',
);
const freshJoinStart = harness.indexOf('prepare_archive_v2_fresh_join_roots() {');
const freshJoinEnd = harness.indexOf('\narchive_v2_health_role() {', freshJoinStart);
const freshJoinFunction = freshJoinStart >= 0 && freshJoinEnd > freshJoinStart
    ? harness.slice(freshJoinStart, freshJoinEnd)
    : '';
const hotCheckpointProfileStart = harness.indexOf('verify_archive_v2_hot_checkpoint_profile() {');
const hotCheckpointProfileEnd = harness.indexOf('\nverify_public_history_manifest_parity() {', hotCheckpointProfileStart);
const hotCheckpointProfileFunction = hotCheckpointProfileStart >= 0
    && hotCheckpointProfileEnd > hotCheckpointProfileStart
    ? harness.slice(hotCheckpointProfileStart, hotCheckpointProfileEnd)
    : '';
assert(freshJoinFunction.length > 0, 'four-validator harness defines Archive V2 fresh-join preparation');
assert(
    freshJoinFunction.includes('ARCHIVE_V2_FRESH_JOIN_SOURCE_ROOT:-/tmp/lichen-testnet/archive-v2-v1')
        && freshJoinFunction.includes('ARCHIVE_V2_FRESH_JOIN_REPLICA_ROOT:-/tmp/lichen-testnet/archive-v2-replica-v4')
        && freshJoinFunction.includes('--source "fresh-source:local-a:${replica_root}"')
        && !freshJoinFunction.includes('local source_root="$(db_path')
        && !freshJoinFunction.includes('--source "fresh-source:local-a:$(db_path'),
    'fresh roles restore an explicitly verified catalog source and independent replica, never a state directory',
);
const checkpointCatalogRebuildStart = harness.indexOf('rebuild_archive_v2_checkpoint_catalog_evidence() {');
const checkpointCatalogRebuildEnd = harness.indexOf('\narchive_v2_source_finalized_slot() {', checkpointCatalogRebuildStart);
const checkpointCatalogRebuild = checkpointCatalogRebuildStart >= 0
    && checkpointCatalogRebuildEnd > checkpointCatalogRebuildStart
    ? harness.slice(checkpointCatalogRebuildStart, checkpointCatalogRebuildEnd)
    : '';
assert(
    checkpointCatalogRebuild.includes('for V_NUM in $(seq 1 "$MAX_VALIDATORS")')
        && checkpointCatalogRebuild.includes('--end-slot "$end_slot"')
        && checkpointCatalogRebuild.includes('--history-start-slot "$history_start_slot"')
        && checkpointCatalogRebuild.includes('[[ "$handoff_root" == "$expected_handoff_root" ]]')
        && checkpointCatalogRebuild.includes('reconstructed checkpoint catalog ${catalog_root} differs from ${baseline_catalog_root}')
        && checkpointCatalogRebuild.includes('ARCHIVE_V2_FRESH_JOIN_SOURCE_ROOT=')
        && checkpointCatalogRebuild.includes('ARCHIVE_V2_FRESH_JOIN_REPLICA_ROOT=')
        && checkpointCatalogRebuild.includes('ARCHIVE_V2_CHECKPOINT_BOUND_CATALOGS=1')
        && hotCheckpointProfileFunction.includes('if [[ "$ARCHIVE_V2_CHECKPOINT_BOUND_CATALOGS" == "1" ]]')
        && hotCheckpointProfileFunction.includes('archive-v2-checkpoint-bound-v${V_NUM}'),
    'superseded checkpoint catalogs are reconstructed independently from all four authoritative states and rebound by exact handoff root',
);
const sourcePeerStart = harness.indexOf('activate_checkpoint_bound_source_peer() {');
const sourcePeerEnd = harness.indexOf('\nverify_fresh_archive_v2_role_rejoins() {', sourcePeerStart);
const sourcePeerFunctions = sourcePeerStart >= 0 && sourcePeerEnd > sourcePeerStart
    ? harness.slice(sourcePeerStart, sourcePeerEnd)
    : '';
const freshRoleVerifyStart = harness.indexOf('verify_fresh_archive_v2_role_rejoins() {');
const freshRoleVerifyEnd = harness.indexOf('\narchive_v2_rpc_block_hash() {', freshRoleVerifyStart);
const freshRoleVerifyFunction = freshRoleVerifyStart >= 0 && freshRoleVerifyEnd > freshRoleVerifyStart
    ? harness.slice(freshRoleVerifyStart, freshRoleVerifyEnd)
    : '';
assert(
    sourcePeerFunctions.includes('for validator_num in 1 4')
        && sourcePeerFunctions.includes('bound_root="$ARCHIVE_V2_FRESH_JOIN_SOURCE_ROOT"')
        && sourcePeerFunctions.includes('LICHEN_LOCAL_ARCHIVE_V2_RECENT_HISTORY_SLOTS_V1="$ARCHIVE_V2_PUBLIC_MIN_RECENT_HISTORY_SLOTS"')
        && sourcePeerFunctions.includes('bound_root="/tmp/lichen-testnet/archive-v2-checkpoint-bound-v4"')
        && sourcePeerFunctions.includes('LICHEN_LOCAL_ARCHIVE_V2_RECENT_HISTORY_SLOTS_V4="$ARCHIVE_V2_PUBLIC_MIN_RECENT_HISTORY_SLOTS"')
        && sourcePeerFunctions.includes('V${validator_num} is serving the exact catalog binding required by checkpoint')
        && sourcePeerFunctions.includes('unset')
        && sourcePeerFunctions.includes('LICHEN_LOCAL_ARCHIVE_V2_ROOT_V1')
        && sourcePeerFunctions.includes('LICHEN_LOCAL_ARCHIVE_V2_ROOT_V4')
        && sourcePeerFunctions.includes('V${validator_num} restored its current append-complete Archive V2 catalog')
        && freshRoleVerifyFunction.indexOf('activate_checkpoint_bound_source_peer')
            < freshRoleVerifyFunction.indexOf('reset_fresh_role_state_with_identity')
        && freshRoleVerifyFunction.indexOf('restore_current_archive_v2_source_peer')
            < freshRoleVerifyFunction.indexOf('after fresh Archive V2 role rejoin matrix'),
    'fresh checkpoint transfer uses two exact independently bound full-archive source peers and restores both current catalogs before success',
);
const checkpointResumeStart = harness.indexOf('elif [[ "$RESUME_AFTER_ARCHIVE_V2_CHECKPOINT" == "1" ]]');
const checkpointResumeEnd = harness.indexOf('elif [[ "$RESUME_AFTER_PUBLIC_PARITY" == "1" ]]', checkpointResumeStart);
const checkpointResume = checkpointResumeStart >= 0 && checkpointResumeEnd > checkpointResumeStart
    ? harness.slice(checkpointResumeStart, checkpointResumeEnd)
    : '';
assert(
    checkpointResume.includes('manifest.get("snapshot_profile") != profile')
        && checkpointResume.includes('[[ "$profile_root" != "$handoff_root" ]]')
        && checkpointResume.includes('checkpoint_catalog_rebuild_required=1')
        && checkpointResume.includes('rebuild_archive_v2_checkpoint_catalog_evidence')
        && checkpointResume.includes('[[ "$handoff_root" == "$baseline_handoff_root" ]]')
        && checkpointResume.includes('profile_start > required_profile_start')
        && checkpointResume.includes('insufficient_profile_count > 0')
        && checkpointResume.includes('COMMON_CHECKPOINT_SLOT="$RESUME_PUBLIC_PARITY_CHECKPOINT"')
        && checkpointResume.includes('start_archive_v2_validator "$validator_num" "$resume_log"')
        && checkpointResume.includes('if [[ "$resume_role" == "consensus" ]]')
        && checkpointResume.includes('resume_error" == *"consensus"*')
        && checkpointResume.includes('prepare_archive_v2_fresh_join_roots')
        && checkpointResume.indexOf('prepare_archive_v2_fresh_join_roots')
            < checkpointResume.indexOf('verify_fresh_archive_v2_role_rejoins')
        && checkpointResume.includes('run_requested_user_journeys_and_post_parity'),
    'diagnostic fresh-role resume revalidates exact checkpoint/catalog evidence and requested journeys before succeeding',
);
const strictJourneysStart = harness.indexOf('run_requested_user_journeys_and_post_parity() {');
const strictJourneysEnd = harness.indexOf('\nif [[ "$REUSE_EXISTING_CLUSTER" == "1" ]]', strictJourneysStart);
const strictJourneysFunction = strictJourneysStart >= 0 && strictJourneysEnd > strictJourneysStart
    ? harness.slice(strictJourneysStart, strictJourneysEnd)
    : '';
const strictJourneyCalls = harness.match(/^\s{4}run_requested_user_journeys_and_post_parity$/gm) || [];
assert(
    strictJourneysFunction.includes('node "$REPO_ROOT/tests/e2e-volume.js"')
        && strictJourneysFunction.includes('node "$REPO_ROOT/tests/e2e-launchpad.js"')
        && strictJourneysFunction.includes('after post-activity own-state restart')
        && strictJourneysFunction.includes('wait_for_common_checkpoint "post-journey parity"')
        && strictJourneysFunction.includes('verify_public_history_manifest_parity offline "$POST_JOURNEY_CHECKPOINT_SLOT"')
        && strictJourneyCalls.length === 5,
    'normal and exact-resume gates share volume, launchpad, own-state restart, and post-journey parity enforcement',
);
assert(
    harness.includes('LOCAL_GATE_SUCCESS=0')
        && harness.includes('gate_exit_status" -eq 0 && "$LOCAL_GATE_SUCCESS" -ne 1')
        && harness.includes('Local multi-validator gate exited without its success marker')
        && harness.includes('LOCAL_GATE_SUCCESS=1'),
    'cleanup cannot report a shell-aborted local gate as successful',
);
const reconcileStart = harness.indexOf('reconcile_archive_v2_checkpoint_catalogs() {');
const reconcileEnd = harness.indexOf('\nwait_for_common_checkpoint() {', reconcileStart);
const reconcileFunction = reconcileStart >= 0 && reconcileEnd > reconcileStart
    ? harness.slice(reconcileStart, reconcileEnd)
    : '';
const establishedBootstrapStart = harness.indexOf('bootstrap_established_archive_v2_role() {');
const establishedBootstrapEnd = harness.indexOf('\nreconcile_archive_v2_checkpoint_catalogs() {', establishedBootstrapStart);
const establishedBootstrapFunction = establishedBootstrapStart >= 0
    && establishedBootstrapEnd > establishedBootstrapStart
    ? harness.slice(establishedBootstrapStart, establishedBootstrapEnd)
    : '';
assert(
    establishedBootstrapFunction.includes('role-bootstrap')
        && establishedBootstrapFunction.includes('--dry-run')
        && establishedBootstrapFunction.includes('--allow-local-dev-short-history')
        && establishedBootstrapFunction.includes('state_admission_persisted')
        && establishedBootstrapFunction.includes('state_admission_created'),
    'established Archive V2 roles exercise explicit local-only stopped-state dry-run/publish admission evidence',
);
const publicManifestStart = harness.indexOf('public_history_manifest_root() {');
const publicManifestEnd = harness.indexOf('\nverify_archive_v2_hot_checkpoint_profile() {', publicManifestStart);
const publicManifestFunction = publicManifestStart >= 0 && publicManifestEnd > publicManifestStart
    ? harness.slice(publicManifestStart, publicManifestEnd)
    : '';
const logicalArchiveV2StateSource = fs.readFileSync(repoPath('core/src/state/archive_v2_state.rs'), 'utf8');
const logicalArchiveV2CliSource = fs.readFileSync(repoPath('core/src/bin/lichen-archive-v2.rs'), 'utf8');
assert(
    publicManifestFunction.includes('public-history-manifest')
        && publicManifestFunction.includes('profile_catalog_bound="0"')
        && publicManifestFunction.includes('"$profile_catalog_bound" == "1"')
        && publicManifestFunction.includes('--state-dir "$manifest_db_path"')
        && publicManifestFunction.includes('--root "$archive_root"')
        && publicManifestFunction.includes('--source-dir "$source_root"')
        && publicManifestFunction.includes('composed Archive V2 plus hot-checkpoint manifest failed')
        && logicalArchiveV2CliSource.includes('compute_archive_v2_checkpoint_public_history_manifest(')
        && logicalArchiveV2StateSource.includes('checkpoint_handoff_root(history_start_slot)')
        && logicalArchiveV2StateSource.includes('archive_v2_declared_gap_covers')
        && logicalArchiveV2StateSource.includes('logical_checkpoint_manifest_is_independent_of_archive_handoff')
        && logicalArchiveV2StateSource.includes('assert_eq!(manifest_a, full_manifest);'),
    'checkpoint parity composes authenticated Archive V2 prefixes with hot suffixes independently of physical handoff',
);
assert(
    reconcileFunction.includes('--state-dir "$(db_path "$V_NUM")"')
        && !reconcileFunction.includes('--state-dir "$(db_path "$V_NUM")/checkpoints/'),
    'common Archive V2 catalogs build from stopped authoritative state plus cold history',
);
assert(
    reconcileFunction.includes('finality_depth=$((LICHEN_COLD_RETENTION_SLOTS - ARCHIVE_V2_FRESH_JOIN_CATALOG_HEADROOM_SLOTS))')
        && reconcileFunction.includes('common_end=$((minimum_finalized_slot - finality_depth))')
        && reconcileFunction.includes('minimum_checkpoint_slot=$((common_end + ARCHIVE_V2_FRESH_JOIN_RECENT_HISTORY_SLOTS))')
        && reconcileFunction.includes('planned_checkpoint_slot=$(( ((minimum_checkpoint_slot + CHECKPOINT_INTERVAL_SLOTS - 1) / CHECKPOINT_INTERVAL_SLOTS) * CHECKPOINT_INTERVAL_SLOTS ))')
        && !reconcileFunction.includes('maximum_catalog_end_for_fresh_window')
        && !reconcileFunction.includes('common_end=$((maximum_finalized_slot - LICHEN_COLD_RETENTION_SLOTS))'),
    'common Archive V2 catalog preserves established-role coverage and plans a later checkpoint with enough fresh hot suffix',
);
assert(
    reconcileFunction.indexOf('bootstrap_established_archive_v2_role "$V_NUM"')
        < reconcileFunction.indexOf('start_archive_v2_validator "$V_NUM" "$ROLE_LOG"')
        && reconcileFunction.includes('did not restore its stopped-state Archive V2 admission'),
    'common catalog restart requires a durable stopped-state role admission on every validator',
);
const finalReconcileCall = harness.lastIndexOf('    reconcile_archive_v2_checkpoint_catalogs ');
const finalHotCheckpointCall = harness.lastIndexOf('    verify_archive_v2_hot_checkpoint_profile');
const finalPrepareFreshCall = harness.lastIndexOf('    prepare_archive_v2_fresh_join_roots');
const finalVerifyFreshCall = harness.lastIndexOf('    verify_fresh_archive_v2_role_rejoins');
assert(
    finalReconcileCall >= 0
        && finalHotCheckpointCall > finalReconcileCall
        && finalPrepareFreshCall > finalHotCheckpointCall
        && finalVerifyFreshCall > finalPrepareFreshCall,
    'fresh Archive V2 role joins run only after common catalog admission and a catalog-bound checkpoint',
);
assert(
    freshJoinFunction.includes('expected_profile_start=$((COMMON_CHECKPOINT_SLOT - ARCHIVE_V2_FRESH_JOIN_RECENT_HISTORY_SLOTS + 1))')
        && freshJoinFunction.includes('source_catalog_end >= checkpoint_profile_start - 1')
        && freshJoinFunction.includes('checkpoint_profile_start <= expected_profile_start'),
    'fresh joins require at least the configured hot suffix and a catalog-covered checkpoint handoff',
);
const checkpointWaitStart = harness.indexOf('wait_for_common_checkpoint() {');
const checkpointWaitEnd = harness.indexOf('\nwait_for_archive_v2_retention_boundary() {', checkpointWaitStart);
const checkpointWaitFunction = checkpointWaitStart >= 0 && checkpointWaitEnd > checkpointWaitStart
    ? harness.slice(checkpointWaitStart, checkpointWaitEnd)
    : '';
assert(
    checkpointWaitFunction.includes('minimum_target="$ARCHIVE_V2_FRESH_JOIN_RECENT_HISTORY_SLOTS"')
        && checkpointWaitFunction.includes('ARCHIVE_V2_CHECKPOINT_CATALOG_END + ARCHIVE_V2_FRESH_JOIN_RECENT_HISTORY_SLOTS')
        && checkpointWaitFunction.includes('target_slot < minimum_target')
        && checkpointWaitFunction.includes('minimum_target + CHECKPOINT_INTERVAL_SLOTS - 1')
        && checkpointWaitFunction.includes('ARCHIVE_V2_PLANNED_CHECKPOINT_SLOT'),
    'checkpoint selection cannot choose a slot below the catalog-relative fresh-join history window',
);
assert(
    checkpointWaitFunction.includes('remaining_slots=$((target_slot - current_slot))')
        && checkpointWaitFunction.includes('slot_budget_secs=$(((remaining_slots + 3) / 4))')
        && checkpointWaitFunction.includes('deadline=$((SECONDS + slot_budget_secs + 600))'),
    'production-cadence checkpoint timeout scales with the actual remaining slot distance',
);
assert(
    checkpointWaitFunction.includes('! kill -0 "$validator_pid"')
        && checkpointWaitFunction.includes('Skipping checkpoint at slot ${target_slot}')
        && checkpointWaitFunction.includes('Failed to create checkpoint at slot ${target_slot}:')
        && checkpointWaitFunction.includes('terminally paused.*slot=${target_slot}')
        && checkpointWaitFunction.includes('checkpoint_log_offsets'),
    'checkpoint wait fails promptly on new exact-slot skips, failures, pauses, or validator exits',
);
assert(
    harness.includes('restore_interrupted_fresh_role_state() {')
        && harness.includes('arm_fresh_role_restore "$validator_num" "$original_state" "$original_cold"')
        && harness.includes('discard_fresh_role_candidate_state "$state_dir" "$cold_dir"')
        && harness.includes('"${state_dir}.snapshot-live-rollback"')
        && harness.includes('"${state_dir}.snapshot-live-rollback.json"')
        && harness.indexOf('discard_fresh_role_candidate_state "$state_dir" "$cold_dir"')
            < harness.indexOf('mv "$FRESH_ROLE_ORIGINAL_STATE" "$state_dir"')
        && harness.includes('assert_fresh_role_original_has_no_snapshot_transaction')
        && harness.includes('Original fresh-role state remains in its explicit backup path for manual recovery')
        && harness.includes('disarm_fresh_role_restore'),
    'interrupted fresh-role verification discards snapshot sidecars before restoring validator-owned state',
);
const genesisHashStart = harness.indexOf('archive_v2_genesis_hash() {');
const genesisHashEnd = harness.indexOf('\narchive_v2_source_finalized_slot() {', genesisHashStart);
const genesisHashFunction = genesisHashStart >= 0 && genesisHashEnd > genesisHashStart
    ? harness.slice(genesisHashStart, genesisHashEnd)
    : '';
assert(
    genesisHashFunction.includes('archive_v2_rpc_block_hash_with_retry "$V1_RPC" 0')
        && !genesisHashFunction.includes('rpc_query_params "$V1_RPC" getBlock "[0]"'),
    'post-checkpoint genesis capture retries bounded deep-history RPC readiness',
);
const runtimeRoleStart = harness.indexOf('verify_archive_v2_runtime_role_matrix() {');
const runtimeRoleEnd = harness.indexOf('\nreconcile_archive_v2_checkpoint_catalogs() {', runtimeRoleStart);
const runtimeRoleFunction = runtimeRoleStart >= 0 && runtimeRoleEnd > runtimeRoleStart
    ? harness.slice(runtimeRoleStart, runtimeRoleEnd)
    : '';
assert(
    runtimeRoleFunction.includes('for candidate in $(seq 1 256); do')
        && runtimeRoleFunction.includes('archive_v2_rpc_block_hash "$(rpc_port 1)" "$candidate"')
        && runtimeRoleFunction.includes('archive_v2_rpc_error_message')
        && runtimeRoleFunction.includes('consensus_migrated_deep_slot="$candidate"')
        && !runtimeRoleFunction.includes('LICHEN_COLD_RETENTION_SLOTS / 2'),
    'consensus-role denial uses a full-archive-proven migrated slot instead of assuming a retention fraction',
);
const validatorSource = fs.readFileSync(repoPath('validator/src/main.rs'), 'utf8');
const websocketFanoutStart = validatorSource.indexOf('fn emit_dex_events(');
const websocketFanoutEnd = validatorSource.indexOf('\nstruct SnapshotSync', websocketFanoutStart);
const websocketFanout = websocketFanoutStart >= 0 && websocketFanoutEnd > websocketFanoutStart
    ? validatorSource.slice(websocketFanoutStart, websocketFanoutEnd)
    : '';
assert(
    websocketFanout.includes('get_events_by_slot(block.header.slot, usize::MAX)')
        && !websocketFanout.includes('get_contract_logs(')
        && websocketFanout.includes('fn emit_new_canonical_dex_events(')
        && websocketFanout.includes('emit_new_canonical_dex_events(state, dex_broadcaster, dex_event_cursor);')
        && validatorSource.includes('lichen_rpc::ws_event_broadcasters();')
        && validatorSource.includes('start_ws_server_with_broadcasters(')
        && validatorSource.includes('canonical_dex_websocket_cursor_emits_once_with_stored_trade_slot')
        && validatorSource.includes('block_receiver_chainable_sync_path_applies_post_hooks_once_after_store')
        && validatorSource.includes('block_receiver_pending_path_fans_out_once_after_post_hooks')
        && validatorSource.includes('block_receiver_fork_adoption_fans_out_once_after_post_hooks')
        && validatorSource.includes('bft_commit_path_applies_shared_post_hooks_once_after_store'),
    'canonical WebSocket fanout uses bounded slot reads and one shared, once-only DEX projection across every apply path',
);
assert(
    validatorSource.includes('fn materialize_block_commit_for_sync(')
        && validatorSource.includes('certificate.verify_child_metadata(&child.tx_fees_paid, &child.oracle_prices)?;')
        && validatorSource.includes('certificate.verify_parent(&block, chain_id, min_validator_stake)?;')
        && validatorSource.includes('block.commit_signatures = certificate.signatures;')
        && validatorSource.includes('materialize_block_commit_for_sync(\n                            &state_for_block_requests,')
        && validatorSource.includes('Refusing to serve unverifiable canonical block')
        && validatorSource.includes('sync_block_materializes_verified_commit_from_canonical_child')
        && validatorSource.includes('sync_block_refuses_missing_local_and_canonical_child_commit'),
    'P2P catch-up materializes verified canonical-child commits and fails closed without finality evidence',
);
assert(
    validatorSource.includes('static CHECKPOINT_CREATION_ACTIVE: AtomicBool')
        && validatorSource.includes('const MAX_ACTIVE_CHECKPOINT_WATCHDOG_SECS: u64 = 15 * 60;')
        && validatorSource.includes('let Some(active_checkpoint_guard) = ActiveCheckpointGuard::try_begin()')
        && validatorSource.includes('let _active_checkpoint_guard = active_checkpoint_guard;')
        && validatorSource.includes('elapsed > Duration::from_secs(MAX_ACTIVE_CHECKPOINT_WATCHDOG_SECS)'),
    'validator watchdog gives active checkpoints finite maintenance grace without disabling stall recovery',
);
const ledgerStateSource = fs.readFileSync(repoPath('core/src/state/ledger_state.rs'), 'utf8');
const archiveV2StateSource = fs.readFileSync(repoPath('core/src/state/archive_v2_state.rs'), 'utf8');
const snapshotIoSource = fs.readFileSync(repoPath('core/src/state/snapshot_io.rs'), 'utf8');
assert(
    ledgerStateSource.includes('get_hot_or_legacy_cold_block_for_checkpoint')
        && ledgerStateSource.includes('get_hot_or_legacy_cold_block_by_slot_for_checkpoint')
        && ledgerStateSource.includes('get_hot_or_legacy_cold_transaction_for_checkpoint')
        && archiveV2StateSource.includes('export_public_history_category_range_cursor_untracked_with_source')
        && archiveV2StateSource.includes('export_snapshot_category_cursor_untracked_with_source'),
    'hot-repair export has a private hot/legacy-cold checkpoint source independent of public Archive V2 policy',
);
assert(
    snapshotIoSource.includes('const HOT_REPAIR_CHECKPOINT_CACHE_MB: usize = 128;')
        && /Self::open_with_cache_mb\(\s*checkpoint_dir,\s*Some\(HOT_REPAIR_CHECKPOINT_CACHE_MB\)\s*\)/.test(snapshotIoSource)
        && /Self::open_checkpoint_with_cache_mb\(\s*staging,\s*checkpoint_cache_mb\s*\)/.test(snapshotIoSource),
    'hot-repair checkpoint materialization and verification use bounded RocksDB caches',
);
assert(
    snapshotIoSource.includes('hot_repair_checkpoint_materializes_bounded_cold_history_without_cold_store')
        && snapshotIoSource.includes('.get_block(&block_hash)\n            .unwrap_err()')
        && snapshotIoSource.includes('let restored = reopened.get_hot_block_by_slot(7).unwrap().unwrap();'),
    'regression proves consensus public denial and private self-contained cold-tail checkpoint materialization',
);
assert(
    ledgerStateSource.includes('get_hot_or_legacy_cold_block_for_checkpoint')
        && archiveV2StateSource.includes('attach_archive_v2_deferred_checkpoint_catalog')
        && archiveV2StateSource.includes('archive_v2_deferred_checkpoint_catalog_root')
        && validatorSource.includes('.or(state.archive_v2_deferred_checkpoint_catalog_root(history_start_slot)?)')
        && validatorSource.includes('fresh_join_accepts_only_exact_deferred_archive_v2_checkpoint_binding')
        && validatorSource.includes('Archive V2 catalog tip hash conflicts with the local role handoff parent')
        && validatorSource.includes('should_activate_deferred_archive_v2_before_genesis_gate(')
        && validatorSource.indexOf('should_activate_deferred_archive_v2_before_genesis_gate(')
            < validatorSource.indexOf('pre_consensus_genesis_is_ready(is_joining_network'),
    'fresh join accepts only an exact deferred catalog binding, proves its hot-window handoff, and activates it before the genesis gate',
);
const archiveCliSource = fs.readFileSync(repoPath('core/src/bin/lichen-archive-v2.rs'), 'utf8');
const archiveRolesSource = fs.readFileSync(repoPath('core/src/archive_v2/roles.rs'), 'utf8');
assert(
    archiveRolesSource.includes('pub const ARCHIVE_V2_MIN_RECENT_HISTORY_SLOTS: u64 = 50_000;')
        && archiveCliSource.includes('!allow_local_dev_short_history || !local_dev_mode')
        && validatorSource.includes('local_dev_mode && role_config.recent_history_slots < ARCHIVE_V2_MIN_RECENT_HISTORY_SLOTS'),
    'public Archive V2 role admission keeps the 50,000-slot minimum while short gates remain double-gated local development only',
);
assert(
    archiveCliSource.includes('fn verify_full_archive_preflight_local_range(')
        && archiveCliSource.includes('ArchiveV2Role::FullArchive => {')
        && archiveCliSource.includes('verify_full_archive_preflight_local_range(&state, hot_start, finalized_slot)')
        && archiveCliSource.includes('ArchiveV2Role::VerifiedCache | ArchiveV2Role::Consensus => state')
        && validatorSource.includes('verify_local_archive_v2_block_range(state, local_history_start, finalized_slot)')
        && validatorSource.includes('fn runtime_archive_v2_local_block(')
        && validatorSource.includes('archive_v2_full_archive_activation_accepts_cold_catalog_tip_and_handoff'),
    'stopped and runtime full-archive admission both verify owned hot/cold tails while cache and consensus remain hot-only',
);
assert(
    archiveCliSource.includes('archive_v2_state_admission_fingerprint(capability)')
        && archiveCliSource.includes('StateStore::open_read_only_with_cache_mb(&state_dir, Some(64))')
        && archiveCliSource.includes('state.sync_hot_wal()?')
        && archiveCliSource.indexOf('store_archive_v2_role_marker_create_new(&marker_path, &marker)?')
            < archiveCliSource.indexOf('state.put_metadata('),
    'role-bootstrap validates conflicts first and durably binds stopped-state admission after role-marker publication',
);
for (const supportPath of supportPaths) {
    verifySource(supportPath);
}
for (const policyPath of policyPaths) {
    verifySource(policyPath);
}
const activationPlan = fs.readFileSync(
    repoPath('docs/deployment/ARCHIVE_V2_ACTIVATION_CADENCE_AND_VALIDATOR_LIVENESS_PLAN_2026-08-18.md'),
    'utf8',
);
for (const region of ['US', 'EU', 'SEA', 'IN']) {
    assert(
        activationPlan.includes(
            `| ${region} | \`verified_cache\` | authenticated primary and replica R2 HTTPS gateways | 2 GiB hard quota |`,
        ),
        `${region} final testnet topology uses the common verified-cache role and quota`,
    );
}
assert(
    activationPlan.includes('all four validators use `verified_cache`')
        && activationPlan.includes('normal 100,000-slot hot window')
        && activationPlan.includes('same authenticated primary/replica source')
        && !activationPlan.includes('US/EU `verified_cache`, SEA/IN `consensus`'),
    'final testnet topology keeps role, retention, and source policy equal on all four validators',
);

const allDependencies = new Set();
const allPackages = new Map();
for (const entrypoint of entrypoints) {
    assert(
        harness.includes(`node "$REPO_ROOT/${entrypoint}"`),
        `${harnessPath} invokes ${entrypoint}`,
    );
    const { dependencies, missing, packages } = collectLocalDependencies(entrypoint);
    for (const unresolved of missing) {
        console.error(`  Missing local dependency: ${unresolved}`);
    }
    assert(missing.length === 0, `${entrypoint} resolves every local require`);
    for (const dependency of dependencies) {
        allDependencies.add(dependency);
    }
    for (const [packageName, importers] of packages) {
        const allImporters = allPackages.get(packageName) || [];
        allImporters.push(...importers);
        allPackages.set(packageName, allImporters);
    }
}

for (const dependency of Array.from(allDependencies).sort()) {
    verifySource(dependency);
}

const volumeE2e = fs.readFileSync(repoPath('tests/e2e-volume.js'), 'utf8');
const dexSetupHelper = fs.readFileSync(repoPath('tests/helpers/dex-setup.js'), 'utf8');
const cancelAllOffset = volumeE2e.indexOf('buildCancelAllOrders(wallet.address, pairId)');
const phaseOneRefreshOffset = volumeE2e.indexOf("await refreshPairOneOracleBand('Phase 1')");
const phaseOneOffset = volumeE2e.indexOf("section('Phase 1: Multi-Wallet Trading — LICN/lUSD')");
const phaseTwoRefreshOffset = volumeE2e.indexOf("await refreshPairOneOracleBand('Phase 2')");
const phaseTwoOffset = volumeE2e.indexOf("section('Phase 2: Orderbook Depth Stress')");
const phaseThreeRefreshOffset = volumeE2e.indexOf("await refreshPairOneOracleBand('Phase 3')");
const phaseThreeOffset = volumeE2e.indexOf("section('Phase 3: Multi-Pair Volume Sweep')");
assert(
    volumeE2e.includes('async function refreshPairOneOracleBand(phase)')
        && volumeE2e.includes('refreshed.validatorCount < 2')
        && volumeE2e.includes('refreshed.dexBandSourceSlot !== refreshed.sourceSlot')
        && cancelAllOffset >= 0
        && phaseOneRefreshOffset > cancelAllOffset
        && phaseOneOffset > phaseOneRefreshOffset
        && phaseTwoRefreshOffset > phaseOneOffset
        && phaseTwoOffset > phaseTwoRefreshOffset
        && phaseThreeRefreshOffset > phaseTwoOffset
        && phaseThreeOffset > phaseThreeRefreshOffset,
    'strict volume E2E proves a fresh quorum-backed pair-1 band before every CLOB trading phase',
);
const marginRefreshOffset = volumeE2e.indexOf('await refreshMarginMarkPrice(');
const marginOpenOffset = volumeE2e.indexOf('const args = buildOpenPosition(');
assert(
    volumeE2e.includes("require('./helpers/dex-setup')")
        && marginRefreshOffset >= 0
        && marginOpenOffset > marginRefreshOffset
        && dexSetupHelper.includes("buildOracleAttestationData('LICN', oraclePrice, 8)")
        && dexSetupHelper.includes("'validator-keypair.json'")
        && dexSetupHelper.includes("rpcCall(rpcUrl, 'getValidators')")
        && dexSetupHelper.includes('waitForFreshMarginMark('),
    'strict volume E2E advances the canonical validator-oracle quorum before opening margin positions',
);
const wsPhaseOffset = volumeE2e.indexOf("section('Phase 11: WebSocket Live Events')");
const wsBandRefreshOffset = volumeE2e.indexOf('await refreshMarginMarkPrice(', wsPhaseOffset);
const wsSellOffset = volumeE2e.indexOf("'WS trigger: Eve sell'", wsPhaseOffset);
assert(
    wsPhaseOffset >= 0
        && wsBandRefreshOffset > wsPhaseOffset
        && wsSellOffset > wsBandRefreshOffset
        && volumeE2e.includes('refreshed.dexBandSourceSlot === refreshed.sourceSlot')
        && dexSetupHelper.includes('async function waitForFreshDexBand(')
        && dexSetupHelper.includes('`dex_band_${pairId}`')
        && dexSetupHelper.includes('band.price !== marginPrice'),
    'strict volume E2E refreshes and verifies the canonical DEX price band immediately before the WebSocket trade',
);

const packageJson = JSON.parse(fs.readFileSync(repoPath('package.json'), 'utf8'));
const packageLock = JSON.parse(fs.readFileSync(repoPath('package-lock.json'), 'utf8'));
const declaredPackages = {
    ...packageJson.dependencies,
    ...packageJson.devDependencies,
    ...packageJson.optionalDependencies,
};
const lockedRootPackages = {
    ...packageLock.packages?.['']?.dependencies,
    ...packageLock.packages?.['']?.devDependencies,
    ...packageLock.packages?.['']?.optionalDependencies,
};
for (const [packageName, importers] of Array.from(allPackages).sort()) {
    assert(
        Object.hasOwn(declaredPackages, packageName),
        `${packageName} is declared for ${importers.join(', ')}`,
    );
    assert(
        Object.hasOwn(lockedRootPackages, packageName)
            && Boolean(packageLock.packages?.[`node_modules/${packageName}`]),
        `${packageName} is pinned by the root package lock`,
    );
}

const releaseWorkflow = fs.readFileSync(repoPath('.github/workflows/release.yml'), 'utf8');
const rollingReleaseDeploy = fs.readFileSync(repoPath('scripts/rolling-release-deploy.sh'), 'utf8');
const archiveJob = extractWorkflowJob(releaseWorkflow, 'archive-parity-local-gate');
const contractBundleJob = extractWorkflowJob(releaseWorkflow, 'contract-bundle');
const releaseBuildJob = extractWorkflowJob(releaseWorkflow, 'build');
const checksumJob = extractWorkflowJob(releaseWorkflow, 'checksums');
const setupNodeOffset = archiveJob.indexOf('actions/setup-node@');
const npmCiOffset = archiveJob.indexOf('npm ci --ignore-scripts');
const harnessOffset = archiveJob.indexOf('bash tests/local-multi-validator-test.sh 4');
const contractDownloadOffset = releaseBuildJob.indexOf('name: Download genesis contract bundle');
const contractStageOffset = releaseBuildJob.indexOf('name: Stage the tested genesis contracts for binary embedding');
const releaseBinaryBuildOffset = releaseBuildJob.indexOf('name: Build release binary');
assert(archiveJob.length > 0, 'release workflow defines the archive parity job');
assert(
    !archiveJob.includes('needs: release-quality-gate')
        && !contractBundleJob.includes('needs: release-quality-gate'),
    'independent release hard gates start in parallel instead of extending deployment downtime',
);
assert(
    releaseBuildJob.includes('needs: [contract-bundle, compiler-sandbox-gate]')
        && checksumJob.includes('needs: [release-quality-gate, archive-parity-local-gate, build]'),
    'checksums remain fail-closed behind quality, Archive V2 parity, contract, compiler, and platform gates',
);
assert(setupNodeOffset >= 0, 'archive parity job installs pinned Node.js');
assert(archiveJob.includes('node-version: "22"'), 'archive parity job uses Node.js 22');
assert(npmCiOffset >= 0, 'archive parity job installs locked journey dependencies');
assert(
    setupNodeOffset < npmCiOffset && npmCiOffset < harnessOffset,
    'archive parity job installs dependencies before the four-validator harness',
);
assert(
    releaseWorkflow.includes('LICHEN_RUN_VOLUME_E2E=1'),
    'release workflow enables strict volume journeys',
);
assert(
    releaseWorkflow.includes('LICHEN_RUN_LAUNCHPAD_E2E=1'),
    'release workflow enables launchpad journeys',
);
assert(
    releaseWorkflow.includes('LICHEN_ARCHIVE_V2_FRESH_JOIN_RECENT_HISTORY_SLOTS=5000'),
    'hosted local-dev release gate uses a substantial realizable half-checkpoint fresh-history window',
);
assert(
    harnessOffset >= 0,
    'release workflow runs the tracked four-validator harness',
);
assert(
    contractDownloadOffset >= 0
        && contractStageOffset > contractDownloadOffset
        && releaseBinaryBuildOffset > contractStageOffset,
    'signed platform binaries embed the exact contract bundle that passed the contract gate',
);
assert(
    contractBundleJob.includes("-name 'abi.json'")
        && contractBundleJob.includes('test "$abi_count" -eq "$wasm_count"'),
    'signed contract bundle pairs every tested WASM with its ABI',
);
assert(
    releaseBuildJob.includes('cp "$wasm" "$destination"')
        && releaseBuildJob.includes('test "$contract_count" -gt 0'),
    'release binary build fails closed when the tested contract bundle cannot be staged',
);
assert(
    rollingReleaseDeploy.includes('LICHEN_REPAIR_TESTNET_DEX_CONTRACTS')
        && rollingReleaseDeploy.includes('DEX contract repair is testnet-only and requires LICHEN_COORDINATED_RELEASE=1')
        && rollingReleaseDeploy.includes('all validators are stopped; applying the signed Testnet DEX code/ABI repair'),
    'preserved-chain DEX repair is testnet-only and runs only after the coordinated fleet stop',
);
assert(
    rollingReleaseDeploy.includes("grep -Fxq 'contracts=17'")
        && rollingReleaseDeploy.includes("grep -Fxq 'changed=0'")
        && rollingReleaseDeploy.includes('--db-path "$state_dir"')
        && !rollingReleaseDeploy.includes('--data-dir "$state_dir"')
        && rollingReleaseDeploy.includes('--confirm repair-dex-contracts:testnet:v0.5.272'),
    'coordinated DEX repair targets preserved state, writes explicitly, and proves idempotent completion',
);
assert(
    releaseWorkflow.includes('--bin lichen-archive-v2')
        && releaseWorkflow.includes('cp target/${{ matrix.target }}/release/lichen-archive-v2 ${{ matrix.artifact }}/')
        && releaseWorkflow.includes('Copy-Item target/${{ matrix.target }}/release/lichen-archive-v2.exe ${{ matrix.artifact }}/'),
    'release workflow builds and packages the Archive V2 operator CLI on every platform',
);
assert(
    releaseBuildJob.includes('--bin lichen-moss-provider')
        && releaseBuildJob.includes('for bin in lichen-custody lichen-faucet lichen-moss-provider; do')
        && releaseBuildJob.includes('strip target/${{ matrix.target }}/release/lichen-moss-provider')
        && releaseBuildJob.includes('aarch64-linux-gnu-strip target/${{ matrix.target }}/release/lichen-moss-provider')
        && releaseBuildJob.includes('cp -R deploy ${{ matrix.artifact }}/deploy'),
    'Linux release archives build, strip, and package the Moss provider service',
);
for (const region of ['us', 'eu', 'sea', 'in']) {
    assert(
        isFile(`deploy/Caddyfile.testnet-moss-${region}`)
            && isFile(`deploy/Caddyfile.mainnet-moss-${region}`),
        `signed release source includes testnet/mainnet Moss ingress for ${region.toUpperCase()}`,
    );
}
assert(
    rollingReleaseDeploy.includes('require_archive_bin_sha "$archive" "$root" lichen-archive-v2')
        && rollingReleaseDeploy.includes('check_file_hash /usr/local/bin/lichen-archive-v2 "$EXPECTED_ARCHIVE_V2_SHA" lichen-archive-v2'),
    'signed release deployment requires, installs, and verifies the Archive V2 operator CLI',
);

console.log(`\nArchive parity gate asset QA: ${passed} passed, ${failed} failed`);
if (failed > 0) {
    process.exit(1);
}
