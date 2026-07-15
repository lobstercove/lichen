#!/usr/bin/env bash
set -euo pipefail

IMAGE="${COMPILER_SANDBOX_IMAGE:-lichen-compiler-sandbox:audit}"
EXPECTED_ASSEMBLYSCRIPT_VERSION="${EXPECTED_ASSEMBLYSCRIPT_VERSION:-0.28.19}"

if [ "$(id -u)" = "0" ] || [ "$(id -g)" = "0" ]; then
  echo "compiler sandbox smoke test requires a non-root host UID and GID" >&2
  exit 1
fi

workspace="$(mktemp -d)"
trap 'rm -rf "$workspace"' EXIT
mkdir -p "$workspace/assembly" "$workspace/c" "$workspace/rust/src"

printf '%s\n' \
  'export function add(a: i32, b: i32): i32 { return a + b; }' \
  > "$workspace/assembly/contract.ts"
printf '%s\n' \
  'int add(int a, int b) { return a + b; }' \
  > "$workspace/c/contract.c"
printf '%s\n' \
  '[package]' \
  'name = "wasm-contract"' \
  'version = "0.1.0"' \
  'edition = "2021"' \
  '' \
  '[lib]' \
  'crate-type = ["cdylib"]' \
  '' \
  '[profile.release]' \
  'panic = "abort"' \
  > "$workspace/rust/Cargo.toml"
printf '%s\n' \
  '#[no_mangle]' \
  'pub extern "C" fn add(a: i32, b: i32) -> i32 { a + b }' \
  > "$workspace/rust/src/lib.rs"

uid="$(id -u)"
gid="$(id -g)"
runtime_args=(
  run --rm
  --network none
  --cap-drop ALL
  --security-opt no-new-privileges:true
  --user "$uid:$gid"
  --pids-limit 256
  --memory 1g
  --cpus 2
  --read-only
  --tmpfs /tmp:rw,exec,nosuid,size=256m
  -e HOME=/tmp
  -e TMPDIR=/tmp
  -e CARGO_HOME=/tmp/cargo-home
  -e CARGO_TARGET_DIR=/workspace/target
  -e RUSTUP_HOME=/usr/local/rustup
)

default_uid="$(docker run --rm --entrypoint id "$IMAGE" -u)"
if [ "$default_uid" != "10001" ]; then
  echo "compiler sandbox default UID is $default_uid, expected 10001" >&2
  exit 1
fi

asc_version="$(docker "${runtime_args[@]}" --entrypoint asc "$IMAGE" --version)"
case "$asc_version" in
  *"$EXPECTED_ASSEMBLYSCRIPT_VERSION"*) ;;
  *)
    echo "unexpected AssemblyScript version: $asc_version" >&2
    exit 1
    ;;
esac

docker "${runtime_args[@]}" \
  --mount "type=bind,source=$workspace/assembly,target=/workspace" \
  --workdir /workspace \
  "$IMAGE" \
  asc /workspace/contract.ts -o /workspace/contract.wasm --exportRuntime

docker "${runtime_args[@]}" \
  --mount "type=bind,source=$workspace/c,target=/workspace" \
  --workdir /workspace \
  "$IMAGE" \
  clang --target=wasm32 -nostdlib -Wl,--no-entry -Wl,--export-all \
  -o /workspace/contract.wasm /workspace/contract.c

docker "${runtime_args[@]}" \
  --mount "type=bind,source=$workspace/rust,target=/workspace" \
  --workdir /workspace \
  "$IMAGE" \
  cargo build --target wasm32-unknown-unknown --release

owner_uid() {
  stat -c %u "$1" 2>/dev/null || stat -f %u "$1"
}

for wasm in \
  "$workspace/assembly/contract.wasm" \
  "$workspace/c/contract.wasm" \
  "$workspace/rust/target/wasm32-unknown-unknown/release/wasm_contract.wasm"; do
  test -s "$wasm"
  magic="$(od -An -tx1 -N4 "$wasm" | tr -d ' \n')"
  if [ "$magic" != "0061736d" ]; then
    echo "$wasm is not a valid WASM binary" >&2
    exit 1
  fi
  if [ "$(owner_uid "$wasm")" != "$uid" ]; then
    echo "$wasm is not owned by host UID $uid" >&2
    exit 1
  fi
done

echo "compiler sandbox smoke passed: UID 10001, host UID $uid, Rust/C/AssemblyScript WASM"
