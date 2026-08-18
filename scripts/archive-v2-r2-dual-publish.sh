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
: "${AWS_ACCESS_KEY_ID:?AWS_ACCESS_KEY_ID is required}"
: "${AWS_SECRET_ACCESS_KEY:?AWS_SECRET_ACCESS_KEY is required}"
: "${AWS_SESSION_TOKEN:?AWS_SESSION_TOKEN is required}"
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
[[ "$ARCHIVE_V2_EXPECTED_PREVIOUS_CATALOG_SHA256" =~ ^[0-9a-f]{64}$ ]] || {
  echo "ARCHIVE_V2_EXPECTED_PREVIOUS_CATALOG_SHA256 is invalid" >&2
  exit 2
}
[[ "$ARCHIVE_V2_MAX_OBJECT_BYTES" =~ ^[1-9][0-9]*$ ]] || {
  echo "ARCHIVE_V2_MAX_OBJECT_BYTES must be a positive integer" >&2
  exit 2
}
for credential in "$AWS_ACCESS_KEY_ID" "$AWS_SECRET_ACCESS_KEY" "$AWS_SESSION_TOKEN"; do
  [[ "$credential" =~ ^[A-Za-z0-9_+=./:-]+$ ]] || {
    echo "R2 temporary credential contains unsupported characters" >&2
    exit 2
  }
done

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
curl_config="$(mktemp "$tmp_root/lichen-r2-dual-curl.XXXXXX")"
cleanup() {
  if [ -f "$curl_config" ]; then
    : >"$curl_config"
    unlink "$curl_config"
  fi
}
trap cleanup EXIT
trap 'trap - EXIT; cleanup; exit 1' HUP INT TERM
printf '%s\n' \
  'aws-sigv4 = "aws:amz:auto:s3"' \
  "user = \"$AWS_ACCESS_KEY_ID:$AWS_SECRET_ACCESS_KEY\"" \
  "header = \"x-amz-security-token: $AWS_SESSION_TOKEN\"" \
  'fail' \
  'silent' \
  'show-error' \
  'retry = 3' \
  'retry-all-errors' \
  'connect-timeout = 15' \
  'max-time = 1800' >"$curl_config"

endpoint="${R2_ENDPOINT%/}"
prefix="${R2_PREFIX#/}"
prefix="${prefix%/}"

remote_sha() {
  local bucket=$1 key=$2
  curl --config "$curl_config" "$endpoint/$bucket/$prefix/$key" |
    sha256sum |
    awk '{print $1}'
}

put_and_verify() {
  local bucket=$1 source=$2 key=$3 expected_sha remote
  expected_sha="$(sha256sum "$source" | awk '{print $1}')"
  curl --config "$curl_config" --request PUT --upload-file "$source" \
    "$endpoint/$bucket/$prefix/$key"
  remote="$(remote_sha "$bucket" "$key")"
  [ "$remote" = "$expected_sha" ] || {
    echo "R2 read-after-write mismatch for $bucket/$key" >&2
    exit 1
  }
  jq -cn --arg bucket "$bucket" --arg key "$prefix/$key" \
    --arg sha256 "$expected_sha" \
    '{operation:"verified_put",bucket:$bucket,key:$key,sha256:$sha256}'
}

# Resume accepts either the exact old catalog or the exact new catalog. Any
# third value is an unrecognized concurrent/conflicting publication.
for bucket in "$R2_PRIMARY_BUCKET" "$R2_REPLICA_BUCKET"; do
  current="$(remote_sha "$bucket" catalog.av2)"
  [ "$current" = "$ARCHIVE_V2_EXPECTED_PREVIOUS_CATALOG_SHA256" ] \
    || [ "$current" = "$new_catalog_sha" ] \
    || { echo "unexpected existing catalog in $bucket" >&2; exit 1; }
done

# Publish the complete immutable object set first, then its manifests. No
# catalog visible to a reader can refer to an object missing in either bucket.
for object_hash in "${hashes[@]}"; do
  for bucket in "$R2_PRIMARY_BUCKET" "$R2_REPLICA_BUCKET"; do
    put_and_verify "$bucket" \
      "$ARCHIVE_V2_ROOT/objects/$object_hash.av2s" \
      "objects/$object_hash.av2s"
  done
done
for object_hash in "${hashes[@]}"; do
  for bucket in "$R2_PRIMARY_BUCKET" "$R2_REPLICA_BUCKET"; do
    put_and_verify "$bucket" \
      "$ARCHIVE_V2_ROOT/manifests/$object_hash.av2m" \
      "manifests/$object_hash.av2m"
  done
done

# Catalog replacement is deliberately last and idempotent. A process failure
# between buckets leaves all referenced immutable data present in both and a
# rerun completes the second catalog replacement.
put_and_verify "$R2_PRIMARY_BUCKET" "$catalog" catalog.av2
put_and_verify "$R2_REPLICA_BUCKET" "$catalog" catalog.av2

for bucket in "$R2_PRIMARY_BUCKET" "$R2_REPLICA_BUCKET"; do
  [ "$(remote_sha "$bucket" catalog.av2)" = "$new_catalog_sha" ] || {
    echo "final catalog mismatch in $bucket" >&2
    exit 1
  }
done

jq -cn \
  --arg operation archive_v2_r2_dual_publish \
  --arg primary_bucket "$R2_PRIMARY_BUCKET" \
  --arg replica_bucket "$R2_REPLICA_BUCKET" \
  --arg previous_catalog_sha256 "$ARCHIVE_V2_EXPECTED_PREVIOUS_CATALOG_SHA256" \
  --arg catalog_sha256 "$new_catalog_sha" \
  --argjson segment_count "$segment_count" \
  --arg published_unix_seconds "$(date +%s)" \
  '{operation:$operation,primary_bucket:$primary_bucket,replica_bucket:$replica_bucket,previous_catalog_sha256:$previous_catalog_sha256,catalog_sha256:$catalog_sha256,segment_count:$segment_count,published_unix_seconds:($published_unix_seconds|tonumber)}'
