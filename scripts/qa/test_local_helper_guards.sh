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

    if ! grep -Fq "$expected" "$file_path"; then
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
    mkdir -p "$fixture_root/keypairs"
    printf '{}' >"$fixture_root/data/state-7001/genesis-keys/genesis-primary-lichen-testnet-1.json"
    printf '{}' >"$fixture_root/data/state-7001/genesis-keys/treasury-lichen-testnet-1.json"
    printf '{"privateKey":[0]}' >"$fixture_root/keypairs/release-signing-key.json"
    expected_signing_key="$(cd "$fixture_root/keypairs" && pwd)/release-signing-key.json"

    write_file_from_stdin "$fixture_root/run-validator.sh" <<'EOF'
#!/usr/bin/env bash
exit 0
EOF
    chmod +x "$fixture_root/run-validator.sh"
    write_file_from_stdin "$fixture_root/scripts/run-custody.sh" <<'EOF'
#!/usr/bin/env bash
exit 0
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
exit 0
EOF
    chmod +x "$fixture_root/target/release/lichen-custody"
    write_file_from_stdin "$fixture_root/target/release/lichen-faucet" <<'EOF'
#!/usr/bin/env bash
exit 0
EOF
    chmod +x "$fixture_root/target/release/lichen-faucet"
    setup_fake_curl "$fixture_root"

    local output_file="$TMP_DIR/start-local-stack-cleanup.log"
    (
        cd "$fixture_root"
        env PATH="$fixture_root/bin:$PATH" ./scripts/start-local-stack.sh testnet
    ) >"$output_file" 2>&1

    assert_peer_trust_state_removed "start-local-stack cleanup" "$fixture_root" 7001 7002 7003
    assert_output_contains "start-local-stack metadata signer default" "$expected_signing_key" "$fixture_root/bootstrap-keypair-path.txt"
    echo "✅ start-local-stack peer trust cleanup"
}

assert_start_local_3validators_clears_peer_trust_state() {
    local fixture_root
    fixture_root="$(make_fixture_dir start-local-3validators-cleanup)"

    copy_repo_script "scripts/start-local-3validators.sh" "$fixture_root"
    seed_peer_trust_state "$fixture_root" 7001 7002 7003
    mkdir -p "$fixture_root/data/local-cluster"
    mkdir -p "$fixture_root/keypairs"
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
    setup_fake_curl "$fixture_root"

    local output_file="$TMP_DIR/start-local-3validators-cleanup.log"
    (
        cd "$fixture_root"
        env PATH="$fixture_root/bin:$PATH" ./scripts/start-local-3validators.sh start
    ) >"$output_file" 2>&1

    assert_peer_trust_state_removed "start-local-3validators cleanup" "$fixture_root" 7001 7002 7003
    echo "✅ start-local-3validators peer trust cleanup"
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

assert_rejected \
    "clean-slate redeploy guard" \
    "Refusing clean-slate redeploy without explicit confirmation." \
    "$ROOT_DIR/scripts/clean-slate-redeploy.sh" testnet

assert_local_insecure_custody_defaults_zero_threshold
assert_start_local_stack_clears_peer_trust_state
assert_start_local_3validators_clears_peer_trust_state

echo "============================================================"
echo "Local helper guards: 7 passed, 0 failed"
