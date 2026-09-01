#!/usr/bin/env bash
set -euo pipefail

# Stopped-validator, non-destructive Archive V2 role-marker bootstrap. This
# resolves the low-space transition loop without starting the validator,
# deleting legacy history, weakening capacity floors, or installing binaries.

: "${ARCHIVE_V2_BINARY:?ARCHIVE_V2_BINARY is required}"
: "${ARCHIVE_V2_EXPECTED_BINARY_SHA256:?ARCHIVE_V2_EXPECTED_BINARY_SHA256 is required}"
: "${ARCHIVE_V2_STATE_DIR:?ARCHIVE_V2_STATE_DIR is required}"
: "${ARCHIVE_V2_COLD_STORE:?ARCHIVE_V2_COLD_STORE is required}"
: "${ARCHIVE_V2_ROOT:?ARCHIVE_V2_ROOT is required}"
: "${ARCHIVE_V2_ROLE:?ARCHIVE_V2_ROLE is required}"
: "${ARCHIVE_V2_RECENT_HISTORY_SLOTS:?ARCHIVE_V2_RECENT_HISTORY_SLOTS is required}"
: "${ARCHIVE_V2_WAL:?ARCHIVE_V2_WAL is required}"
: "${ARCHIVE_V2_IDENTITY_FILE:?ARCHIVE_V2_IDENTITY_FILE is required}"
: "${ARCHIVE_V2_RECOVERY_FILE:?ARCHIVE_V2_RECOVERY_FILE is required}"
: "${ARCHIVE_V2_SERVICE:?ARCHIVE_V2_SERVICE is required}"
: "${ARCHIVE_V2_EVIDENCE_OUTPUT:?ARCHIVE_V2_EVIDENCE_OUTPUT is required}"

ARCHIVE_V2_SOURCE_MAX_OBJECT_BYTES="${ARCHIVE_V2_SOURCE_MAX_OBJECT_BYTES:-2147483648}"

for command_name in sha256sum jq systemctl stat mktemp; do
  command -v "$command_name" >/dev/null 2>&1 || {
    echo "$command_name is required" >&2
    exit 2
  }
done

[[ "$ARCHIVE_V2_EXPECTED_BINARY_SHA256" =~ ^[0-9a-f]{64}$ ]] || {
  echo "ARCHIVE_V2_EXPECTED_BINARY_SHA256 must be lowercase SHA-256 hex" >&2
  exit 2
}
[[ "$ARCHIVE_V2_RECENT_HISTORY_SLOTS" =~ ^[1-9][0-9]*$ ]] || {
  echo "ARCHIVE_V2_RECENT_HISTORY_SLOTS must be a positive integer" >&2
  exit 2
}
[[ "$ARCHIVE_V2_SOURCE_MAX_OBJECT_BYTES" =~ ^[1-9][0-9]*$ ]] || {
  echo "ARCHIVE_V2_SOURCE_MAX_OBJECT_BYTES must be a positive integer" >&2
  exit 2
}
case "$ARCHIVE_V2_ROLE" in
  consensus|full-archive|verified-cache) ;;
  *)
    echo "ARCHIVE_V2_ROLE must be consensus, full-archive, or verified-cache" >&2
    exit 2
    ;;
esac

require_regular_file() {
  local path="$1"
  local label="$2"
  [ -f "$path" ] && [ ! -L "$path" ] && [ -s "$path" ] || {
    echo "$label must be a non-empty regular file: $path" >&2
    exit 2
  }
}

require_directory() {
  local path="$1"
  local label="$2"
  [ -d "$path" ] && [ ! -L "$path" ] || {
    echo "$label must be a real directory: $path" >&2
    exit 2
  }
}

require_regular_file "$ARCHIVE_V2_BINARY" "Archive V2 binary"
require_directory "$ARCHIVE_V2_STATE_DIR" "state directory"
require_directory "$ARCHIVE_V2_COLD_STORE" "legacy cold store"
require_directory "$ARCHIVE_V2_ROOT" "Archive V2 root"
require_regular_file "$ARCHIVE_V2_ROOT/catalog.av2" "Archive V2 catalog"
require_regular_file "$ARCHIVE_V2_WAL" "consensus WAL"
require_regular_file "$ARCHIVE_V2_IDENTITY_FILE" "validator identity"
require_regular_file "$ARCHIVE_V2_RECOVERY_FILE" "recovery file"

actual_binary_sha256="$(sha256sum "$ARCHIVE_V2_BINARY" | awk '{print $1}')"
[ "$actual_binary_sha256" = "$ARCHIVE_V2_EXPECTED_BINARY_SHA256" ] || {
  echo "Archive V2 binary SHA-256 mismatch" >&2
  exit 2
}

[ "$(systemctl show "$ARCHIVE_V2_SERVICE" --property=LoadState --value)" = "loaded" ] || {
  echo "validator service is not loaded: $ARCHIVE_V2_SERVICE" >&2
  exit 2
}
if systemctl is-active --quiet "$ARCHIVE_V2_SERVICE"; then
  echo "refusing role bootstrap while $ARCHIVE_V2_SERVICE is active" >&2
  exit 2
fi

marker="$ARCHIVE_V2_ROOT/role-config-v1.bin"
marker_preexisting_sha256=""
if [ -e "$marker" ] || [ -L "$marker" ]; then
  require_regular_file "$marker" "existing Archive V2 role marker"
  marker_preexisting_sha256="$(sha256sum "$marker" | awk '{print $1}')"
fi
[ ! -e "$ARCHIVE_V2_EVIDENCE_OUTPUT" ] && [ ! -L "$ARCHIVE_V2_EVIDENCE_OUTPUT" ] || {
  echo "refusing to overwrite evidence output: $ARCHIVE_V2_EVIDENCE_OUTPUT" >&2
  exit 2
}

bootstrap_command=(
  "$ARCHIVE_V2_BINARY" role-bootstrap
  --state-dir "$ARCHIVE_V2_STATE_DIR"
  --cold-store "$ARCHIVE_V2_COLD_STORE"
  --root "$ARCHIVE_V2_ROOT"
  --role "$ARCHIVE_V2_ROLE"
  --recent-history-slots "$ARCHIVE_V2_RECENT_HISTORY_SLOTS"
  --source-max-object-bytes "$ARCHIVE_V2_SOURCE_MAX_OBJECT_BYTES"
  --wal "$ARCHIVE_V2_WAL"
  --identity-file "$ARCHIVE_V2_IDENTITY_FILE"
  --recovery-file "$ARCHIVE_V2_RECOVERY_FILE"
  --acknowledge-stopped-validator
  --acknowledge-low-space-legacy-retirement
)

case "$ARCHIVE_V2_ROLE" in
  verified-cache)
    : "${ARCHIVE_V2_CACHE_ROOT:?ARCHIVE_V2_CACHE_ROOT is required for verified-cache}"
    : "${ARCHIVE_V2_CACHE_QUOTA_BYTES:?ARCHIVE_V2_CACHE_QUOTA_BYTES is required for verified-cache}"
    : "${ARCHIVE_V2_SOURCE_ROOTS_FILE:?ARCHIVE_V2_SOURCE_ROOTS_FILE is required for verified-cache}"
    require_directory "$ARCHIVE_V2_CACHE_ROOT" "verified cache root"
    require_regular_file "$ARCHIVE_V2_SOURCE_ROOTS_FILE" "source-roots file"
    [ "$(stat -c '%a' "$ARCHIVE_V2_SOURCE_ROOTS_FILE")" = "600" ] || {
      echo "ARCHIVE_V2_SOURCE_ROOTS_FILE must have mode 600" >&2
      exit 2
    }
    [[ "$ARCHIVE_V2_CACHE_QUOTA_BYTES" =~ ^[1-9][0-9]*$ ]] || {
      echo "ARCHIVE_V2_CACHE_QUOTA_BYTES must be a positive integer" >&2
      exit 2
    }
    bootstrap_command+=(
      --cache-root "$ARCHIVE_V2_CACHE_ROOT"
      --cache-quota-bytes "$ARCHIVE_V2_CACHE_QUOTA_BYTES"
    )
    source_count=0
    while IFS= read -r source_root || [ -n "$source_root" ]; do
      [ -n "$source_root" ] || {
        echo "source-roots file contains an empty line" >&2
        exit 2
      }
      require_directory "$source_root" "Archive V2 source root"
      require_regular_file "$source_root/catalog.av2" "source catalog"
      bootstrap_command+=(--source-root "$source_root")
      source_count=$((source_count + 1))
    done <"$ARCHIVE_V2_SOURCE_ROOTS_FILE"
    [ "$source_count" -gt 0 ] || {
      echo "source-roots file is empty" >&2
      exit 2
    }
    ;;
  consensus|full-archive)
    [ -z "${ARCHIVE_V2_CACHE_ROOT:-}" ] \
      && [ -z "${ARCHIVE_V2_CACHE_QUOTA_BYTES:-}" ] \
      && [ -z "${ARCHIVE_V2_SOURCE_ROOTS_FILE:-}" ] || {
      echo "$ARCHIVE_V2_ROLE must not configure cache or source roots" >&2
      exit 2
    }
    ;;
esac

temporary="$(mktemp -d)"
trap 'rm -rf -- "$temporary"' EXIT
dry_run_json="$temporary/dry-run.json"
publish_json="$temporary/publish.json"

"${bootstrap_command[@]}" --dry-run >"$dry_run_json"
jq -e '
  .operation == "role_bootstrap"
  and .bootstrap_authorized == true
  and .marker_created == false
  and .state_admission_created == false
  and .dry_run == true
' "$dry_run_json" >/dev/null
if [ -n "$marker_preexisting_sha256" ]; then
  [ "$(sha256sum "$marker" | awk '{print $1}')" = "$marker_preexisting_sha256" ] || {
    echo "dry run changed the existing role marker" >&2
    exit 2
  }
else
  [ ! -e "$marker" ] && [ ! -L "$marker" ] || {
    echo "dry run unexpectedly created a role marker" >&2
    exit 2
  }
fi

"${bootstrap_command[@]}" >"$publish_json"
jq -e '
  .operation == "role_bootstrap"
  and .bootstrap_authorized == true
  and .state_admission_persisted == true
  and .dry_run == false
' "$publish_json" >/dev/null
require_regular_file "$marker" "published Archive V2 role marker"
if [ -n "$marker_preexisting_sha256" ]; then
  [ "$(sha256sum "$marker" | awk '{print $1}')" = "$marker_preexisting_sha256" ] || {
    echo "publish changed the existing role marker" >&2
    exit 2
  }
  [ "$(jq -r '.marker_created' "$publish_json")" = "false" ] || {
    echo "idempotent publish incorrectly reported a new marker" >&2
    exit 2
  }
else
  [ "$(jq -r '.marker_created' "$publish_json")" = "true" ] || {
    echo "new publish did not report marker creation" >&2
    exit 2
  }
fi
if [ "$(jq -r '.state_admission_persisted' "$dry_run_json")" = "false" ]; then
  [ "$(jq -r '.state_admission_created' "$publish_json")" = "true" ] || {
    echo "publish did not create the missing state-bound Archive V2 admission marker" >&2
    exit 2
  }
fi

for field in role network_id genesis_hash catalog_root catalog_segments catalog_end_slot finalized_slot required_archive_end hot_start_slot genesis_mossstake_slot_only; do
  dry_value="$(jq -cS ".${field}" "$dry_run_json")"
  publish_value="$(jq -cS ".${field}" "$publish_json")"
  [ "$dry_value" = "$publish_value" ] || {
    echo "role bootstrap dry-run/publish mismatch for $field" >&2
    exit 2
  }
done

evidence_parent="$(dirname "$ARCHIVE_V2_EVIDENCE_OUTPUT")"
mkdir -p "$evidence_parent"
umask 077
set -o noclobber
jq -n \
  --arg binary_sha256 "$actual_binary_sha256" \
  --arg service "$ARCHIVE_V2_SERVICE" \
  --arg marker_sha256 "$(sha256sum "$marker" | awk '{print $1}')" \
  --slurpfile dry_run "$dry_run_json" \
  --slurpfile publish "$publish_json" \
  '{
    operation: "archive_v2_low_space_role_bootstrap",
    binary_sha256: $binary_sha256,
    stopped_service: $service,
    marker_sha256: $marker_sha256,
    dry_run: $dry_run[0],
    publish: $publish[0]
  }' >"$ARCHIVE_V2_EVIDENCE_OUTPUT"
set +o noclobber
sync -f "$ARCHIVE_V2_EVIDENCE_OUTPUT"

echo "Archive V2 role and state-admission markers verified and published; no validator start or legacy deletion was performed."
echo "Evidence: $ARCHIVE_V2_EVIDENCE_OUTPUT"
