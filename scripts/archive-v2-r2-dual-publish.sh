#!/usr/bin/env bash
set -euo pipefail

# Publish one or more locally verified Archive V2 segments to both canonical
# R2 buckets. Objects and manifests are uploaded and read back from both
# domains before the append-only catalog is replaced in either bucket.
#
# ARCHIVE_V2_SEGMENT_LIST is a mode-0600 TSV with exactly:
#   <catalog-index><TAB><lowercase-segment-object-sha256>
# Credentials are accepted only through the environment and are copied to a
# private temporary curl config so they never appear in process arguments.

for tool in curl jq sha256sum; do
  command -v "$tool" >/dev/null 2>&1 || {
    echo "missing required tool: $tool" >&2
    exit 2
  }
done

: "${ARCHIVE_V2_ROOT:?ARCHIVE_V2_ROOT is required}"
: "${ARCHIVE_V2_BINARY:?ARCHIVE_V2_BINARY is required}"
: "${ARCHIVE_V2_SEGMENT_LIST:?ARCHIVE_V2_SEGMENT_LIST is required}"
: "${ARCHIVE_V2_EXPECTED_PREVIOUS_CATALOG_SHA256:?ARCHIVE_V2_EXPECTED_PREVIOUS_CATALOG_SHA256 is required}"
: "${R2_ENDPOINT:?R2_ENDPOINT is required}"
: "${R2_PRIMARY_BUCKET:?R2_PRIMARY_BUCKET is required}"
: "${R2_REPLICA_BUCKET:?R2_REPLICA_BUCKET is required}"
: "${R2_PREFIX:?R2_PREFIX is required}"
: "${R2_PRIMARY_ACCESS_KEY_ID:?R2_PRIMARY_ACCESS_KEY_ID is required}"
: "${R2_PRIMARY_SECRET_ACCESS_KEY:?R2_PRIMARY_SECRET_ACCESS_KEY is required}"
: "${R2_PRIMARY_SESSION_TOKEN:?R2_PRIMARY_SESSION_TOKEN is required}"
: "${R2_REPLICA_ACCESS_KEY_ID:?R2_REPLICA_ACCESS_KEY_ID is required}"
: "${R2_REPLICA_SECRET_ACCESS_KEY:?R2_REPLICA_SECRET_ACCESS_KEY is required}"
: "${R2_REPLICA_SESSION_TOKEN:?R2_REPLICA_SESSION_TOKEN is required}"
ARCHIVE_V2_MAX_OBJECT_BYTES="${ARCHIVE_V2_MAX_OBJECT_BYTES:-1073741824}"

case "$R2_ENDPOINT" in
  https://*.r2.cloudflarestorage.com) ;;
  *) echo "R2_ENDPOINT must be an HTTPS Cloudflare R2 account endpoint" >&2; exit 2 ;;
esac
for bucket in "$R2_PRIMARY_BUCKET" "$R2_REPLICA_BUCKET"; do
  [[ "$bucket" =~ ^[a-z0-9][a-z0-9.-]{1,61}[a-z0-9]$ ]] || {
    echo "R2 bucket name is invalid" >&2
    exit 2
  }
done
[ "$R2_PRIMARY_BUCKET" != "$R2_REPLICA_BUCKET" ] || {
  echo "R2 primary and replica buckets must be distinct" >&2
  exit 2
}
[[ "$R2_PREFIX" =~ ^[A-Za-z0-9._/-]+$ ]] || {
  echo "R2_PREFIX contains unsupported characters" >&2
  exit 2
}
normalized_prefix="${R2_PREFIX#/}"
normalized_prefix="${normalized_prefix%/}"
[ -n "$normalized_prefix" ] || {
  echo "R2_PREFIX must select a non-root object namespace" >&2
  exit 2
}
IFS='/' read -r -a prefix_components <<<"$normalized_prefix"
for component in "${prefix_components[@]}"; do
  [ -n "$component" ] && [ "$component" != "." ] && [ "$component" != ".." ] || {
    echo "R2_PREFIX contains an empty or relative path component" >&2
    exit 2
  }
done
[[ "$ARCHIVE_V2_EXPECTED_PREVIOUS_CATALOG_SHA256" =~ ^[0-9a-f]{64}$ ]] || {
  echo "ARCHIVE_V2_EXPECTED_PREVIOUS_CATALOG_SHA256 is invalid" >&2
  exit 2
}
[[ "$ARCHIVE_V2_MAX_OBJECT_BYTES" =~ ^[1-9][0-9]*$ ]] || {
  echo "ARCHIVE_V2_MAX_OBJECT_BYTES must be a positive integer" >&2
  exit 2
}
for credential in \
  "$R2_PRIMARY_ACCESS_KEY_ID" \
  "$R2_PRIMARY_SECRET_ACCESS_KEY" \
  "$R2_PRIMARY_SESSION_TOKEN" \
  "$R2_REPLICA_ACCESS_KEY_ID" \
  "$R2_REPLICA_SECRET_ACCESS_KEY" \
  "$R2_REPLICA_SESSION_TOKEN"; do
  [[ "$credential" =~ ^[A-Za-z0-9_+=./:-]+$ ]] || {
    echo "R2 temporary credential contains unsupported characters" >&2
    exit 2
  }
done
[ "$R2_PRIMARY_ACCESS_KEY_ID" != "$R2_REPLICA_ACCESS_KEY_ID" ] || {
  echo "R2 primary and replica publications require distinct scoped credentials" >&2
  exit 2
}

for path in "$ARCHIVE_V2_ROOT" "$ARCHIVE_V2_SEGMENT_LIST"; do
  [ ! -L "$path" ] || { echo "refusing symlink input: $path" >&2; exit 2; }
done
[ -d "$ARCHIVE_V2_ROOT" ] || { echo "Archive V2 root is missing" >&2; exit 2; }
[ -f "$ARCHIVE_V2_SEGMENT_LIST" ] || { echo "segment list is missing" >&2; exit 2; }
[ "$(stat -c '%a' "$ARCHIVE_V2_SEGMENT_LIST")" = 600 ] || {
  echo "segment list must have mode 0600" >&2
  exit 2
}

catalog="$ARCHIVE_V2_ROOT/catalog.av2"
[ -f "$catalog" ] && [ ! -L "$catalog" ] || {
  echo "canonical local catalog is missing or unsafe" >&2
  exit 2
}
new_catalog_sha="$(sha256sum "$catalog" | awk '{print $1}')"
[ "$new_catalog_sha" != "$ARCHIVE_V2_EXPECTED_PREVIOUS_CATALOG_SHA256" ] || {
  echo "new catalog must differ from the expected previous catalog" >&2
  exit 2
}

segment_count=0
previous_index=-1
declare -a indexes=()
declare -a hashes=()
while IFS=$'\t' read -r index object_hash extra; do
  [ -z "${extra:-}" ] || { echo "segment list has extra fields" >&2; exit 2; }
  [[ "$index" =~ ^[0-9]+$ ]] || { echo "invalid segment index" >&2; exit 2; }
  [[ "$object_hash" =~ ^[0-9a-f]{64}$ ]] || { echo "invalid segment hash" >&2; exit 2; }
  [ "$index" -gt "$previous_index" ] || {
    echo "segment indexes must be strictly increasing" >&2
    exit 2
  }
  object="$ARCHIVE_V2_ROOT/objects/$object_hash.av2s"
  manifest="$ARCHIVE_V2_ROOT/manifests/$object_hash.av2m"
  for file in "$object" "$manifest"; do
    [ -f "$file" ] && [ ! -L "$file" ] || {
      echo "required immutable Archive V2 file is missing or unsafe: $file" >&2
      exit 2
    }
  done
  [ "$(stat -c '%s' "$object")" -le "$ARCHIVE_V2_MAX_OBJECT_BYTES" ] || {
    echo "segment $object_hash exceeds the verified-cache object limit" >&2
    exit 2
  }
  verify_json="$("$ARCHIVE_V2_BINARY" verify --root "$ARCHIVE_V2_ROOT" --start-index "$index" --max-objects 1)"
  verified_hash="$(jq -er '.verified_object_hashes | select(length == 1) | .[0]' <<<"$verify_json")"
  [ "$verified_hash" = "$object_hash" ] || {
    echo "catalog index $index does not match $object_hash" >&2
    exit 2
  }
  indexes+=("$index")
  hashes+=("$object_hash")
  previous_index=$index
  segment_count=$((segment_count + 1))
done <"$ARCHIVE_V2_SEGMENT_LIST"
[ "$segment_count" -gt 0 ] || { echo "segment list is empty" >&2; exit 2; }
[ "$segment_count" -le 1000 ] || { echo "segment list exceeds 1000 objects" >&2; exit 2; }

tmp_root=/dev/shm
[ -d "$tmp_root" ] && [ -w "$tmp_root" ] || tmp_root=/tmp
umask 077
primary_curl_config="$(mktemp "$tmp_root/lichen-r2-primary-curl.XXXXXX")"
replica_curl_config="$(mktemp "$tmp_root/lichen-r2-replica-curl.XXXXXX")"
cleanup() {
  local config
  for config in "$primary_curl_config" "$replica_curl_config"; do
    if [ -f "$config" ]; then
      : >"$config"
      unlink "$config"
    fi
  done
}
trap cleanup EXIT
trap 'trap - EXIT; cleanup; exit 1' HUP INT TERM

write_curl_config() {
  local config=$1 access_key=$2 secret_key=$3 session_token=$4
  printf '%s\n' \
    'aws-sigv4 = "aws:amz:auto:s3"' \
    "user = \"$access_key:$secret_key\"" \
    "header = \"x-amz-security-token: $session_token\"" \
    'fail' \
    'silent' \
    'show-error' \
    'retry = 3' \
    'retry-all-errors' \
    'connect-timeout = 15' \
    'max-time = 1800' >"$config"
  chmod 0600 "$config"
}

write_curl_config \
  "$primary_curl_config" \
  "$R2_PRIMARY_ACCESS_KEY_ID" \
  "$R2_PRIMARY_SECRET_ACCESS_KEY" \
  "$R2_PRIMARY_SESSION_TOKEN"
write_curl_config \
  "$replica_curl_config" \
  "$R2_REPLICA_ACCESS_KEY_ID" \
  "$R2_REPLICA_SECRET_ACCESS_KEY" \
  "$R2_REPLICA_SESSION_TOKEN"

endpoint="${R2_ENDPOINT%/}"
prefix="$normalized_prefix"

remote_sha() {
  local config=$1 bucket=$2 key=$3
  curl --config "$config" "$endpoint/$bucket/$prefix/$key" |
    sha256sum |
    awk '{print $1}'
}

remote_etag() {
  local config=$1 bucket=$2 key=$3 etag
  etag="$(
    curl --config "$config" --head "$endpoint/$bucket/$prefix/$key" |
      awk '/^[Ee][Tt][Aa][Gg]:[[:space:]]*/ { sub(/^[^:]*:[[:space:]]*/, ""); sub(/\r$/, ""); print; exit }'
  )"
  [[ "$etag" =~ ^\"[A-Fa-f0-9-]+\"$ ]] || {
    echo "R2 returned an invalid or missing ETag for $bucket/$key" >&2
    exit 1
  }
  printf '%s\n' "$etag"
}

put_and_verify() {
  local config=$1 bucket=$2 source=$3 key=$4 expected_sha remote
  expected_sha="$(sha256sum "$source" | awk '{print $1}')"
  curl --config "$config" --request PUT --upload-file "$source" \
    "$endpoint/$bucket/$prefix/$key"
  remote="$(remote_sha "$config" "$bucket" "$key")"
  [ "$remote" = "$expected_sha" ] || {
    echo "R2 read-after-write mismatch for $bucket/$key" >&2
    exit 1
  }
  jq -cn --arg bucket "$bucket" --arg key "$prefix/$key" \
    --arg sha256 "$expected_sha" \
    '{operation:"verified_put",bucket:$bucket,key:$key,sha256:$sha256}'
}

put_catalog_and_verify() {
  local config=$1 bucket=$2 current_sha=$3 expected_etag=$4 remote
  if [ "$current_sha" != "$new_catalog_sha" ]; then
    curl --config "$config" --request PUT --header "If-Match: $expected_etag" \
      --upload-file "$catalog" "$endpoint/$bucket/$prefix/catalog.av2"
  fi
  remote="$(remote_sha "$config" "$bucket" catalog.av2)"
  [ "$remote" = "$new_catalog_sha" ] || {
    echo "conditional R2 catalog publication mismatch for $bucket/catalog.av2" >&2
    exit 1
  }
  jq -cn --arg bucket "$bucket" --arg key "$prefix/catalog.av2" \
    --arg sha256 "$new_catalog_sha" --arg previous_etag "$expected_etag" \
    '{operation:"verified_conditional_catalog_put",bucket:$bucket,key:$key,sha256:$sha256,previous_etag:$previous_etag}'
}

# Resume accepts either the exact old catalog or the exact new catalog. Any
# third value is an unrecognized concurrent/conflicting publication.
primary_etag_before="$(remote_etag "$primary_curl_config" "$R2_PRIMARY_BUCKET" catalog.av2)"
primary_current="$(remote_sha "$primary_curl_config" "$R2_PRIMARY_BUCKET" catalog.av2)"
primary_etag_after="$(remote_etag "$primary_curl_config" "$R2_PRIMARY_BUCKET" catalog.av2)"
[ "$primary_etag_before" = "$primary_etag_after" ] || {
  echo "catalog changed during preflight in $R2_PRIMARY_BUCKET" >&2
  exit 1
}
[ "$primary_current" = "$ARCHIVE_V2_EXPECTED_PREVIOUS_CATALOG_SHA256" ] \
  || [ "$primary_current" = "$new_catalog_sha" ] \
  || { echo "unexpected existing catalog in $R2_PRIMARY_BUCKET" >&2; exit 1; }
replica_etag_before="$(remote_etag "$replica_curl_config" "$R2_REPLICA_BUCKET" catalog.av2)"
replica_current="$(remote_sha "$replica_curl_config" "$R2_REPLICA_BUCKET" catalog.av2)"
replica_etag_after="$(remote_etag "$replica_curl_config" "$R2_REPLICA_BUCKET" catalog.av2)"
[ "$replica_etag_before" = "$replica_etag_after" ] || {
  echo "catalog changed during preflight in $R2_REPLICA_BUCKET" >&2
  exit 1
}
[ "$replica_current" = "$ARCHIVE_V2_EXPECTED_PREVIOUS_CATALOG_SHA256" ] \
  || [ "$replica_current" = "$new_catalog_sha" ] \
  || { echo "unexpected existing catalog in $R2_REPLICA_BUCKET" >&2; exit 1; }

# Publish the complete immutable object set first, then its manifests. No
# catalog visible to a reader can refer to an object missing in either bucket.
for object_hash in "${hashes[@]}"; do
  put_and_verify "$primary_curl_config" "$R2_PRIMARY_BUCKET" \
    "$ARCHIVE_V2_ROOT/objects/$object_hash.av2s" \
    "objects/$object_hash.av2s"
  put_and_verify "$replica_curl_config" "$R2_REPLICA_BUCKET" \
    "$ARCHIVE_V2_ROOT/objects/$object_hash.av2s" \
    "objects/$object_hash.av2s"
done
for object_hash in "${hashes[@]}"; do
  put_and_verify "$primary_curl_config" "$R2_PRIMARY_BUCKET" \
    "$ARCHIVE_V2_ROOT/manifests/$object_hash.av2m" \
    "manifests/$object_hash.av2m"
  put_and_verify "$replica_curl_config" "$R2_REPLICA_BUCKET" \
    "$ARCHIVE_V2_ROOT/manifests/$object_hash.av2m" \
    "manifests/$object_hash.av2m"
done

# Catalog replacement is deliberately last and idempotent. A process failure
# between buckets leaves all referenced immutable data present in both and a
# rerun completes the second catalog replacement.
put_catalog_and_verify "$primary_curl_config" "$R2_PRIMARY_BUCKET" \
  "$primary_current" "$primary_etag_before"
put_catalog_and_verify "$replica_curl_config" "$R2_REPLICA_BUCKET" \
  "$replica_current" "$replica_etag_before"

[ "$(remote_sha "$primary_curl_config" "$R2_PRIMARY_BUCKET" catalog.av2)" = "$new_catalog_sha" ] || {
  echo "final catalog mismatch in $R2_PRIMARY_BUCKET" >&2
  exit 1
}
[ "$(remote_sha "$replica_curl_config" "$R2_REPLICA_BUCKET" catalog.av2)" = "$new_catalog_sha" ] || {
  echo "final catalog mismatch in $R2_REPLICA_BUCKET" >&2
  exit 1
}

jq -cn \
  --arg operation archive_v2_r2_dual_publish \
  --arg primary_bucket "$R2_PRIMARY_BUCKET" \
  --arg replica_bucket "$R2_REPLICA_BUCKET" \
  --arg previous_catalog_sha256 "$ARCHIVE_V2_EXPECTED_PREVIOUS_CATALOG_SHA256" \
  --arg catalog_sha256 "$new_catalog_sha" \
  --argjson segment_count "$segment_count" \
  --arg published_unix_seconds "$(date +%s)" \
  '{operation:$operation,primary_bucket:$primary_bucket,replica_bucket:$replica_bucket,previous_catalog_sha256:$previous_catalog_sha256,catalog_sha256:$catalog_sha256,segment_count:$segment_count,published_unix_seconds:($published_unix_seconds|tonumber)}'
