#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/../.." && pwd)"
TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/lichen-helper-guards.XXXXXX")"
trap 'rm -rf "$TMP_DIR"' EXIT

write_file_from_stdin() {
    local path="$1"

    mkdir -p "$(dirname "$path")"
    cat >"$path"
}

copy_repo_script() {
    local relative_path="$1"
    local fixture_root="$2"

    mkdir -p "$fixture_root/$(dirname "$relative_path")"
    cp "$ROOT_DIR/$relative_path" "$fixture_root/$relative_path"
    chmod +x "$fixture_root/$relative_path"
}

assert_output_contains() {
    local label="$1"
    local expected="$2"
    local file_path="$3"

    if ! grep -Fq -- "$expected" "$file_path"; then
        echo "❌ ${label}: expected output missing"
        echo "Expected: $expected"
        echo "Actual output:"
        cat "$file_path"
        exit 1
    fi
}

assert_path_missing() {
    local label="$1"
    local path="$2"

    if [ -e "$path" ]; then
        echo "❌ ${label}: expected path to be removed: $path"
        exit 1
    fi
}

make_fixture_dir() {
    local name="$1"
    local fixture_dir="$TMP_DIR/$name"

    mkdir -p "$fixture_dir"
    printf '%s\n' "$fixture_dir"
}

seed_peer_trust_state() {
    local fixture_root="$1"
    shift

    local port
    for port in "$@"; do
        mkdir -p "$fixture_root/data/state-${port}/home/.lichen/validators"
        printf 'known-peer\n' >"$fixture_root/data/state-${port}/known-peers.json"
        printf 'peer-id\n' >"$fixture_root/data/state-${port}/home/.lichen/peer_identities.json"
        printf 'validator-state\n' >"$fixture_root/data/state-${port}/home/.lichen/validators/current.json"
    done
}

assert_peer_trust_state_removed() {
    local label_prefix="$1"
    local fixture_root="$2"
    shift 2

    local port
    for port in "$@"; do
        assert_path_missing "$label_prefix known peers ${port}" "$fixture_root/data/state-${port}/known-peers.json"
        assert_path_missing "$label_prefix peer identities ${port}" "$fixture_root/data/state-${port}/home/.lichen/peer_identities.json"
        assert_path_missing "$label_prefix validators dir ${port}" "$fixture_root/data/state-${port}/home/.lichen/validators"
    done
}

setup_fake_curl() {
    local fixture_root="$1"
    write_file_from_stdin "$fixture_root/bin/curl" <<'EOF'
#!/usr/bin/env bash
payload=""
for ((i = 1; i <= $#; i++)); do
    if [ "${!i}" = "-d" ] && [ $((i + 1)) -le $# ]; then
        next_index=$((i + 1))
        payload="${!next_index}"
        break
    fi
done

if printf '%s' "$payload" | grep -Fq '"method":"getValidators"'; then
    printf '%s\n' '{"jsonrpc":"2.0","result":{"validators":[{"stake":1},{"stake":1},{"stake":1}]}}'
elif printf '%s' "$payload" | grep -Fq '"method":"getLatestBlock"'; then
    printf '%s\n' '{"jsonrpc":"2.0","result":{"slot":42,"hash":"fixture-tip"}}'
else
    printf '%s\n' '{"jsonrpc":"2.0","result":{"status":"ok"}}'
fi
EOF
    chmod +x "$fixture_root/bin/curl"
}

assert_local_insecure_custody_defaults_zero_threshold() {
    local fixture_root
    fixture_root="$(make_fixture_dir custody-insecure-threshold)"

    copy_repo_script "scripts/run-custody.sh" "$fixture_root"
    mkdir -p "$fixture_root/data/local-cluster"
    write_file_from_stdin "$fixture_root/target/release/lichen-custody" <<'EOF'
#!/usr/bin/env bash
printf 'endpoint=%s\n' "${CUSTODY_SIGNER_ENDPOINTS:-unset}"
printf 'threshold=%s\n' "${CUSTODY_SIGNER_THRESHOLD:-unset}"
EOF
    chmod +x "$fixture_root/target/release/lichen-custody"

    local output_file="$TMP_DIR/custody-insecure-threshold.log"
    (
        cd "$fixture_root"
        env \
            LICHEN_LOCAL_DEV=1 \
            CUSTODY_ALLOW_INSECURE_SEED=1 \
            CUSTODY_API_AUTH_TOKEN=test-local-token \
            ./scripts/run-custody.sh testnet
    ) >"$output_file" 2>&1

    assert_output_contains "insecure custody threshold" 'endpoint=' "$output_file"
    assert_output_contains "insecure custody threshold" 'threshold=0' "$output_file"
    echo "✅ insecure custody threshold default"
}

assert_start_local_stack_clears_peer_trust_state() {
    local fixture_root
    local expected_signing_key
    fixture_root="$(make_fixture_dir start-local-stack-cleanup)"

    copy_repo_script "scripts/start-local-stack.sh" "$fixture_root"
    seed_peer_trust_state "$fixture_root" 7001 7002 7003
    mkdir -p "$fixture_root/data/state-7001/genesis-keys"
    mkdir -p "$fixture_root/data/local-cluster"
    mkdir -p "$fixture_root/keypairs"
    printf '{}' >"$fixture_root/data/state-7001/genesis-keys/genesis-primary-lichen-testnet-1.json"
    printf '{}' >"$fixture_root/data/state-7001/genesis-keys/treasury-lichen-testnet-1.json"
    printf 'fixture-local-keypair-password' >"$fixture_root/data/local-cluster/keypair-password"
    printf '{"privateKey":[0]}' >"$fixture_root/keypairs/release-signing-key.json"
    expected_signing_key="$(cd "$fixture_root/keypairs" && pwd)/release-signing-key.json"

    write_file_from_stdin "$fixture_root/run-validator.sh" <<'EOF'
#!/usr/bin/env bash
exit 0
EOF
    chmod +x "$fixture_root/run-validator.sh"
    write_file_from_stdin "$fixture_root/scripts/run-custody.sh" <<'EOF'
#!/usr/bin/env bash
exec ./target/release/lichen-custody
EOF
    chmod +x "$fixture_root/scripts/run-custody.sh"
    write_file_from_stdin "$fixture_root/scripts/first-boot-deploy.sh" <<'EOF'
#!/usr/bin/env bash
printf '%s' "${SIGNED_METADATA_KEYPAIR:-}" > "$PWD/bootstrap-keypair-path.txt"
exit 0
EOF
    chmod +x "$fixture_root/scripts/first-boot-deploy.sh"
    write_file_from_stdin "$fixture_root/scripts/start-local-3validators.sh" <<'EOF'
#!/usr/bin/env bash
exit 0
EOF
    chmod +x "$fixture_root/scripts/start-local-3validators.sh"
    write_file_from_stdin "$fixture_root/target/release/lichen-validator" <<'EOF'
#!/usr/bin/env bash
exit 0
EOF
    chmod +x "$fixture_root/target/release/lichen-validator"
    write_file_from_stdin "$fixture_root/target/release/lichen-custody" <<'EOF'
#!/usr/bin/env bash
trap 'exit 0' TERM INT
while :; do sleep 1; done
EOF
    chmod +x "$fixture_root/target/release/lichen-custody"
    write_file_from_stdin "$fixture_root/target/release/lichen-faucet" <<'EOF'
#!/usr/bin/env bash
trap 'exit 0' TERM INT
while :; do sleep 1; done
EOF
    chmod +x "$fixture_root/target/release/lichen-faucet"
    write_file_from_stdin "$fixture_root/scripts/local-solana-rpc-mock.py" <<'EOF'
import time
time.sleep(60)
EOF
    write_file_from_stdin "$fixture_root/scripts/local-evm-rpc-mock.py" <<'EOF'
import time
time.sleep(60)
EOF
    setup_fake_curl "$fixture_root"

    local output_file="$TMP_DIR/start-local-stack-cleanup.log"
    (
        cd "$fixture_root"
        env \
            PATH="$fixture_root/bin:$PATH" \
            LICHEN_LOCAL_CUSTODY_PORT=29105 \
            LICHEN_LOCAL_FAUCET_PORT=29100 \
            LICHEN_LOCAL_SOLANA_RPC_PORT=28899 \
            LICHEN_LOCAL_EVM_RPC_PORT=28545 \
            LICHEN_LOCAL_NEOX_RPC_PORT=28546 \
            LICHEN_LOCAL_BNB_RPC_PORT=28547 \
            ./scripts/start-local-stack.sh testnet
    ) >"$output_file" 2>&1

    assert_peer_trust_state_removed "start-local-stack cleanup" "$fixture_root" 7001 7002 7003
    assert_output_contains "start-local-stack metadata signer default" "$expected_signing_key" "$fixture_root/bootstrap-keypair-path.txt"
    while IFS=$'\t' read -r _kind pid; do
        kill "$pid" 2>/dev/null || true
    done <"$fixture_root/data/local-cluster/stack-testnet-pids.tsv"
    echo "✅ start-local-stack peer trust cleanup"
}

assert_start_local_3validators_clears_peer_trust_state() {
    local fixture_root
    fixture_root="$(make_fixture_dir start-local-3validators-cleanup)"

    copy_repo_script "scripts/start-local-3validators.sh" "$fixture_root"
    seed_peer_trust_state "$fixture_root" 7001 7002 7003
    mkdir -p "$fixture_root/data/local-cluster"
    mkdir -p "$fixture_root/keypairs"
    printf 'fixture-local-keypair-password' >"$fixture_root/data/local-cluster/keypair-password"
    printf '{}' >"$fixture_root/keypairs/release-signing-key.json"

    write_file_from_stdin "$fixture_root/run-validator.sh" <<'EOF'
#!/usr/bin/env bash
exit 0
EOF
    chmod +x "$fixture_root/run-validator.sh"
    mkdir -p "$fixture_root/target/release"
    for bin_name in lichen lichen-genesis lichen-validator; do
        write_file_from_stdin "$fixture_root/target/release/$bin_name" <<'EOF'
#!/usr/bin/env bash
exit 0
EOF
        chmod +x "$fixture_root/target/release/$bin_name"
    done
    write_file_from_stdin "$fixture_root/bin/node" <<'EOF'
#!/usr/bin/env bash
out=""
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--out" ]; then
    out="$2"
    break
  fi
  shift
done
if [ -n "$out" ]; then
  mkdir -p "$(dirname "$out")"
    printf '{}' >"$out"
fi
EOF
    chmod +x "$fixture_root/bin/node"
    write_file_from_stdin "$fixture_root/bin/lsof" <<'EOF'
#!/usr/bin/env bash
exit 1
EOF
    chmod +x "$fixture_root/bin/lsof"
    write_file_from_stdin "$fixture_root/bin/pkill" <<'EOF'
#!/usr/bin/env bash
exit 0
EOF
    chmod +x "$fixture_root/bin/pkill"
    setup_fake_curl "$fixture_root"

    local output_file="$TMP_DIR/start-local-3validators-cleanup.log"
    (
        cd "$fixture_root"
        env PATH="$fixture_root/bin:$PATH" ./scripts/start-local-3validators.sh start
    ) >"$output_file" 2>&1

    assert_peer_trust_state_removed "start-local-3validators cleanup" "$fixture_root" 7001 7002 7003
    echo "✅ start-local-3validators peer trust cleanup"
}

assert_run_validator_reattaches_existing_cold_store() {
    local fixture_root
    fixture_root="$(make_fixture_dir run-validator-cold-store)"
    fixture_root="$(cd "$fixture_root" && pwd)"

    copy_repo_script "run-validator.sh" "$fixture_root"
    mkdir -p "$fixture_root/data/archive-7002" "$fixture_root/data/local-cluster"
    mkdir -p "$fixture_root/target/release"
    printf 'rocksdb\n' >"$fixture_root/data/archive-7002/CURRENT"
    printf 'fixture-custody-token\n' >"$fixture_root/data/local-cluster/custody-api-auth-token"
    write_file_from_stdin "$fixture_root/target/release/lichen-validator" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "$*"
printf 'custody_url=%s\n' "${CUSTODY_URL:-unset}"
printf 'custody_token=%s\n' "${CUSTODY_API_AUTH_TOKEN:-unset}"
EOF
    chmod +x "$fixture_root/target/release/lichen-validator"

    local output_file="$TMP_DIR/run-validator-cold-store.log"
    (
        cd "$fixture_root"
        env \
            LICHEN_LOCAL_DEV=1 \
            LICHEN_DISABLE_SUPERVISOR=1 \
            ./run-validator.sh testnet 2
    ) >"$output_file" 2>&1

    assert_output_contains \
        "existing cold store attachment" \
        "--archive-mode --cold-store $fixture_root/data/archive-7002" \
        "$output_file"
    assert_output_contains "local validator custody URL" 'custody_url=http://127.0.0.1:9105' "$output_file"
    assert_output_contains "local validator custody token" 'custody_token=fixture-custody-token' "$output_file"
    echo "✅ existing cold store attachment"
}

assert_run_validator_uses_current_dynamic_identity_setup() {
    if ! grep -Fq 'LICHEN_LOCAL_VALIDATOR_COUNT' "$ROOT_DIR/run-validator.sh"; then
        echo "❌ run-validator dynamic count: missing LICHEN_LOCAL_VALIDATOR_COUNT"
        exit 1
    fi
    if ! grep -Fq 'seq 1 "$LOCAL_VALIDATOR_COUNT"' "$ROOT_DIR/run-validator.sh"; then
        echo "❌ run-validator dynamic count: identity setup is not count-driven"
        exit 1
    fi
    if ! grep -Fq '"$CLI_BIN" identity new' "$ROOT_DIR/run-validator.sh"; then
        echo "❌ run-validator identity setup: canonical identity command missing"
        exit 1
    fi
    if grep -Eq '"\$CLI_BIN"[[:space:]]+init([[:space:]]|$)' "$ROOT_DIR/run-validator.sh"; then
        echo "❌ run-validator identity setup: removed init command remains"
        exit 1
    fi
    echo "✅ run-validator dynamic canonical identity setup"
}

assert_current_health_rpc_surface() {
    local files=(
        "$ROOT_DIR/tests/local-multi-validator-test.sh"
    )
    if grep -En '"method":"health"|rpc(_ok|_has_result)?[^#]*"health"' "${files[@]}"; then
        echo "❌ current health RPC surface: removed health alias remains"
        exit 1
    fi
    if ! grep -Fq 'href="#getHealth"' "$ROOT_DIR/developers/rpc-reference.html"; then
        echo "❌ current health RPC surface: developer reference anchor is stale"
        exit 1
    fi
    echo "✅ current getHealth-only RPC surface"
}

assert_current_e2e_transaction_protocol() {
    local source
    for source in \
        "$ROOT_DIR/core/src/signing.rs" \
        "$ROOT_DIR/sdk/js/src/transaction.ts" \
        "$ROOT_DIR/sdk/python/lichen/transaction.py" \
        "$ROOT_DIR/wallet/extension/src/core/tx-service.js"; do
        if ! grep -Fq 'LICHEN-SIG' "$source"; then
            echo "❌ current E2E transaction protocol: canonical signing envelope missing from $source"
            exit 1
        fi
    done
    if ! grep -Fq 'pub const TX_WIRE_MAGIC' "$ROOT_DIR/core/src/transaction.rs" || \
        ! grep -Fq 'Missing transaction V1 wire envelope' "$ROOT_DIR/core/src/transaction.rs"; then
        echo "❌ current E2E transaction protocol: canonical native wire envelope missing"
        exit 1
    fi
    echo "✅ current E2E transaction protocol"
}

assert_public_history_repair_stays_quiesced() {
    local script="$ROOT_DIR/scripts/stream-public-history-repair.sh"
    local verifier="$ROOT_DIR/scripts/verify-testnet-archive-parity.sh"
    local output_file="$TMP_DIR/public-history-repair-help.log"
    bash -n "$script"
    bash "$script" --help >"$output_file" 2>&1
    assert_output_contains \
        "public-history quiesced repair option" \
        "--leave-target-stopped" \
        "$output_file"
    if ! grep -Fq 'Leaving $SERVICE stopped on $target for fleet-level offline parity' "$script"; then
        echo "❌ public-history quiesced repair: stopped-target behavior missing"
        exit 1
    fi
    for required_guard in \
        'LICHEN_PUBLIC_HISTORY_BACKUP_CONFIRM' \
        'REQUIRED_FREE_RESERVE_BYTES="${LICHEN_PUBLIC_HISTORY_FREE_RESERVE_BYTES:-10737418240}"' \
        'WRITE_HEADROOM_PERCENT="${LICHEN_PUBLIC_HISTORY_WRITE_HEADROOM_PERCENT:-150}"' \
        'Running mandatory full target dry-run before execute' \
        'Dry-run found $conflict_rows conflict row(s)' \
        'without byte/conflict counters' \
        'required_free_bytes=$required_free_bytes' \
        '--verify-contiguous-block-range' \
        'ControlMaster=auto' \
        'ControlPersist=600' \
        'SSH_CONTROL_DIR="$(mktemp -d /tmp/lichen-ph-repair-ssh.XXXXXX)"' \
        'record_page_integrity "$page_file" "$integrity_file"' \
        'validate_page_import "$category" "$row_count" "$import_file"' \
        'sha256sum "$page_file"' \
        'attempt_output="${output_file}.attempt-${attempt}"' \
        'mv -f "$attempt_output" "$output_file"' \
        'status=$?' \
        'remote_pipeline="gzip -dc | $1"' \
        'bash -o pipefail -c $remote_pipeline_quoted' \
        'DIRECT_TRANSFER_MODE="${LICHEN_PUBLIC_HISTORY_DIRECT_TRANSFER:-auto}"' \
        '! ssh-add -l >/dev/null 2>&1' \
        '-o ForwardAgent=yes' \
        'sudo /usr/sbin/sshd -i -e -f %q' \
        'echo "AllowTcpForwarding no"' \
        'echo "AllowStreamLocalForwarding no"' \
        'echo "PermitTTY no"' \
        'DIRECT_AGENT_PUBLIC_REMOTE="$DIRECT_RELAY_REMOTE_DIR/agent-identity.pub"' \
        'DIRECT_TARGET_CONTROL_PREFIX="/tmp/lichen-public-history-target-${TRANSFER_ID}"' \
        'IdentitiesOnly=yes -o IdentityFile=%q' \
        'open_direct_target_control "$target"' \
        'ControlMaster=yes -o ControlPersist=no' \
        'ssh -S %q -O check %q >/dev/null 2>&1' \
        'ssh_run "$SOURCE_HOST" "$direct_copy_command"' \
        'close_direct_target_controls' \
        'StrictHostKeyChecking=%q' \
        'remote_target_attempt="${remote_target_page}.attempt"' \
        'cmp -s "$source_integrity_file" "$target_integrity_file"' \
        'cleanup_remote_transfer_files' \
        'import_page_dry_run_all_targets "$category" "$page_file" "$row_count" "$page_index"' \
        'import_remote_page_dry_run_all_targets "$category" "$row_count"' \
        'if wait "${pids[$index]}"; then' \
        'Dry-run import failed for ${labels[$index]}' \
        'Execute requires --leave-target-stopped' \
        'Execute block repair requires explicit --from-slot and --to-slot bounds.'; do
        if ! grep -Fq -- "$required_guard" "$script"; then
            echo "❌ public-history repair preflight: missing $required_guard"
            exit 1
        fi
    done
    if grep -Fq -- '--stream-pages' "$script" || \
        grep -Fq 'ssh_base "$SSH_USER@$SOURCE_HOST"' "$script" || \
        grep -Fq 'gzip -dc "$input_file" | ssh_base' "$script"; then
        echo "❌ public-history repair transport: concurrent two-hop stream must not bypass bounded page integrity"
        exit 1
    fi
    if grep -Eq 'id_(rsa|ecdsa|ed25519)|scp[^;]*(private|keypair)' "$script"; then
        echo "❌ public-history direct transport: private keys must not be copied to validators"
        exit 1
    fi
    repair_ssh_function="$(sed -n '/^ssh_run()/,/^}/p' "$script")"
    if ! grep -Fq 'else' <<<"$repair_ssh_function" || \
        ! grep -Fq 'status=$?' <<<"$repair_ssh_function"; then
        echo "❌ public-history repair transport: SSH retries must preserve the failed command status"
        exit 1
    fi
    bash -n "$verifier"
    if ! grep -Fq -- '--offline-repair-gate' "$verifier" || \
        ! grep -Fq 'all validator services remain stopped pending parity decision' "$verifier" || \
        ! grep -Fq 'offline manifest roots differ; validators remain stopped' "$verifier" || \
        ! grep -Fq 'ControlMaster=auto' "$verifier" || \
        ! grep -Fq 'ControlPersist=120' "$verifier" || \
        ! grep -Fq 'SSH_RETRY_DELAY_SECS="${LICHEN_ARCHIVE_PARITY_SSH_RETRY_DELAY_SECS:-31}"' "$verifier" || \
        ! grep -Fq 'exec >"$RUN_LOG" 2>&1' "$verifier" || \
        grep -Fq 'exec > >(tee' "$verifier"; then
        echo "❌ public-history offline parity gate: stopped-fleet behavior missing"
        exit 1
    fi
    verifier_ssh_function="$(sed -n '/^ssh_run()/,/^}/p' "$verifier")"
    if ! grep -Fq 'else' <<<"$verifier_ssh_function" || \
        ! grep -Fq 'status=$?' <<<"$verifier_ssh_function"; then
        echo "❌ public-history offline parity gate: SSH retries must preserve the failed command status"
        exit 1
    fi
    manifest_function="$(sed -n '/^remote_manifest()/,/^}/p' "$verifier")"
    if grep -Fq -- '--archive-mode' <<<"$manifest_function" || \
        grep -Fq -- '--cold-store' <<<"$manifest_function"; then
        echo "❌ public-history offline parity gate: public archive configuration must come from the network invariant"
        exit 1
    fi
    if ! grep -Fq -- "sudo rm -rf '\$secondary_dir'" <<<"$manifest_function"; then
        echo "❌ public-history offline parity gate: live secondary cleanup must handle service-owned files"
        exit 1
    fi
    if grep -Fq -- '--cold-store' "$script" || \
        grep -Fq -- '--archive-mode' "$script"; then
        echo "❌ public-history stream repair: public archive configuration must come from the network invariant"
        exit 1
    fi
    echo "✅ public-history quiesced repair"
}

assert_local_archive_parity_uses_immutable_checkpoints() {
    local script="$ROOT_DIR/tests/local-multi-validator-test.sh"
    local required_pattern

    for required_pattern in \
        'wait_for_common_checkpoint "reused-cluster parity"' \
        'wait_for_common_checkpoint "pre-journey parity"' \
        'PRE_JOURNEY_CHECKPOINT_SLOT="$COMMON_CHECKPOINT_SLOT"' \
        'verify_public_history_manifest_parity offline "$PRE_JOURNEY_CHECKPOINT_SLOT"' \
        'wait_for_common_checkpoint "post-journey parity"' \
        'POST_JOURNEY_CHECKPOINT_SLOT="$COMMON_CHECKPOINT_SLOT"' \
        'verify_public_history_manifest_parity offline "$POST_JOURNEY_CHECKPOINT_SLOT"'; do
        if ! grep -Fq -- "$required_pattern" "$script"; then
            echo "❌ local archive parity: missing immutable checkpoint guard: $required_pattern"
            exit 1
        fi
    done
    if ! grep -Fq 'verify_public_history_manifest_parity offline "$COMMON_CHECKPOINT_SLOT"' "$script"; then
        echo "❌ local archive parity: reused clusters do not compare an immutable checkpoint"
        exit 1
    fi
    echo "✅ local archive parity uses immutable checkpoints"
}

assert_sdk_tests_preserve_failure_status() {
    local script="$ROOT_DIR/scripts/test-all-sdks.sh"
    bash -n "$script"
    if grep -Fq 'if ! npx tsc -p tsconfig.test.json' "$script" || \
        grep -Fq 'if ! node "$ts_out_dir/test-all-features.js"' "$script"; then
        echo "❌ SDK test helper: negated commands cannot preserve their original failure status"
        exit 1
    fi
    for required_guard in \
        'if npx tsc -p tsconfig.test.json' \
        'if node "$ts_out_dir/test-all-features.js"' \
        'return "$status"'; do
        if ! grep -Fq -- "$required_guard" "$script"; then
            echo "❌ SDK test helper: missing failure propagation guard: $required_guard"
            exit 1
        fi
    done
    echo "✅ SDK test failure propagation"
}

assert_rejected() {
    local label="$1"
    local expected="$2"
    shift 2

    local output_file="$TMP_DIR/${label//[^a-zA-Z0-9]/_}.log"
    if env -u LICHEN_LOCAL_DEV -u LICHEN_CLEAN_SLATE_REDEPLOY_CONFIRM "$@" >"$output_file" 2>&1; then
        echo "❌ ${label}: command unexpectedly succeeded"
        cat "$output_file"
        exit 1
    fi

    if ! grep -Fq "$expected" "$output_file"; then
        echo "❌ ${label}: expected output missing"
        echo "Expected: $expected"
        echo "Actual output:"
        cat "$output_file"
        exit 1
    fi

    echo "✅ ${label}"
}

echo
echo "🔒 Local Helper Guard Tests"
echo "============================================================"

assert_rejected \
    "run-validator guard" \
    "run-validator.sh is restricted to explicit local development." \
    "$ROOT_DIR/run-validator.sh" testnet 1

assert_rejected \
    "run-custody guard" \
    "run-custody.sh is restricted to explicit local development." \
    "$ROOT_DIR/scripts/run-custody.sh" testnet

assert_rejected \
    "lichen-start custody guard" \
    "Error: --custody is restricted to explicit local development." \
    "$ROOT_DIR/lichen-start.sh" testnet --custody

assert_local_insecure_custody_defaults_zero_threshold
assert_start_local_stack_clears_peer_trust_state
assert_start_local_3validators_clears_peer_trust_state
assert_run_validator_reattaches_existing_cold_store
assert_run_validator_uses_current_dynamic_identity_setup
assert_current_health_rpc_surface
assert_current_e2e_transaction_protocol
assert_public_history_repair_stays_quiesced
assert_local_archive_parity_uses_immutable_checkpoints
assert_sdk_tests_preserve_failure_status

echo "============================================================"
echo "Local helper guards: 13 passed, 0 failed"
