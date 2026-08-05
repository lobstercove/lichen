#!/usr/bin/env bash
set -euo pipefail

# Upload one already-verified Archive V2 segment, its manifest, and the latest
# append-only catalog to one private R2 bucket. Short-lived R2 credentials are
# supplied only through the environment and are moved into a mode-0600 curl
# config in tmpfs so the secret is never present in curl's process arguments.

for tool in curl jq sha256sum; do
  command -v "$tool" >/dev/null 2>&1 || {
    echo "missing required tool: $tool" >&2
    exit 2
  }
done

: "${ARCHIVE_V2_ROOT:?ARCHIVE_V2_ROOT is required}"
: "${ARCHIVE_V2_BINARY:?ARCHIVE_V2_BINARY is required}"
: "${ARCHIVE_V2_SEGMENT_INDEX:?ARCHIVE_V2_SEGMENT_INDEX is required}"
: "${ARCHIVE_V2_SEGMENT_OBJECT_HASH:?ARCHIVE_V2_SEGMENT_OBJECT_HASH is required}"
: "${R2_ENDPOINT:?R2_ENDPOINT is required}"
: "${R2_BUCKET:?R2_BUCKET is required}"
: "${R2_PREFIX:?R2_PREFIX is required}"
: "${R2_FAILURE_DOMAIN:?R2_FAILURE_DOMAIN is required}"
: "${AWS_ACCESS_KEY_ID:?AWS_ACCESS_KEY_ID is required}"
: "${AWS_SECRET_ACCESS_KEY:?AWS_SECRET_ACCESS_KEY is required}"
: "${AWS_SESSION_TOKEN:?AWS_SESSION_TOKEN is required}"

case "$R2_ENDPOINT" in
  https://*.r2.cloudflarestorage.com) ;;
  *) echo "R2_ENDPOINT must be an HTTPS Cloudflare R2 account endpoint" >&2; exit 2 ;;
esac
[[ "$R2_BUCKET" =~ ^[a-z0-9][a-z0-9.-]{1,61}[a-z0-9]$ ]] || {
  echo "R2_BUCKET is invalid" >&2
  exit 2
}
[[ "$R2_PREFIX" =~ ^[A-Za-z0-9._/-]+$ ]] || {
  echo "R2_PREFIX contains unsupported characters" >&2
  exit 2
}
[[ "$R2_FAILURE_DOMAIN" =~ ^[A-Za-z0-9._-]+$ ]] || {
  echo "R2_FAILURE_DOMAIN contains unsupported characters" >&2
  exit 2
}
[[ "$ARCHIVE_V2_SEGMENT_INDEX" =~ ^[0-9]+$ ]] || {
  echo "ARCHIVE_V2_SEGMENT_INDEX must be an unsigned integer" >&2
  exit 2
}
[[ "$ARCHIVE_V2_SEGMENT_OBJECT_HASH" =~ ^[0-9a-f]{64}$ ]] || {
  echo "ARCHIVE_V2_SEGMENT_OBJECT_HASH must be lowercase SHA-256 hex" >&2
  exit 2
}
for credential in "$AWS_ACCESS_KEY_ID" "$AWS_SECRET_ACCESS_KEY" "$AWS_SESSION_TOKEN"; do
  [[ "$credential" =~ ^[A-Za-z0-9_+=./:-]+$ ]] || {
    echo "R2 temporary credential contains unsupported characters" >&2
    exit 2
  }
done

object="$ARCHIVE_V2_ROOT/objects/$ARCHIVE_V2_SEGMENT_OBJECT_HASH.av2s"
manifest="$ARCHIVE_V2_ROOT/manifests/$ARCHIVE_V2_SEGMENT_OBJECT_HASH.av2m"
catalog="$ARCHIVE_V2_ROOT/catalog.av2"
for file in "$object" "$manifest" "$catalog"; do
  [ -f "$file" ] || {
    echo "required Archive V2 file is missing: $file" >&2
    exit 2
  }
done

verify_json="$("$ARCHIVE_V2_BINARY" verify \
  --root "$ARCHIVE_V2_ROOT" \
  --start-index "$ARCHIVE_V2_SEGMENT_INDEX" \
  --max-objects 1)"
verified_hash="$(jq -er '.verified_object_hashes | select(length == 1) | .[0]' <<<"$verify_json")"
[ "$verified_hash" = "$ARCHIVE_V2_SEGMENT_OBJECT_HASH" ] || {
  echo "verified catalog index does not match ARCHIVE_V2_SEGMENT_OBJECT_HASH" >&2
  exit 2
}

tmp_root=/dev/shm
[ -d "$tmp_root" ] && [ -w "$tmp_root" ] || tmp_root=/tmp
umask 077
curl_config="$(mktemp "$tmp_root/lichen-r2-curl.XXXXXX")"
cleanup() {
  if [ -f "$curl_config" ]; then
    : >"$curl_config"
    unlink "$curl_config"
  fi
}
trap cleanup EXIT HUP INT TERM
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

put_and_verify() {
  local source=$1
  local key=$2
  local expected_sha remote_sha
  expected_sha="$(sha256sum "$source" | awk '{print $1}')"
  curl --config "$curl_config" \
    --request PUT \
    --upload-file "$source" \
    "$endpoint/$R2_BUCKET/$prefix/$key"
  remote_sha="$(
    curl --config "$curl_config" "$endpoint/$R2_BUCKET/$prefix/$key" |
      sha256sum |
      awk '{print $1}'
  )"
  [ "$remote_sha" = "$expected_sha" ] || {
    echo "R2 read-after-write hash mismatch for $key" >&2
    exit 1
  }
  jq -cn \
    --arg bucket "$R2_BUCKET" \
    --arg failure_domain "$R2_FAILURE_DOMAIN" \
    --arg key "$prefix/$key" \
    --arg sha256 "$expected_sha" \
    '{bucket:$bucket,failure_domain:$failure_domain,key:$key,sha256:$sha256}'
}

put_and_verify "$object" "objects/$ARCHIVE_V2_SEGMENT_OBJECT_HASH.av2s"
put_and_verify "$manifest" "manifests/$ARCHIVE_V2_SEGMENT_OBJECT_HASH.av2m"
put_and_verify "$catalog" "catalog.av2"

verified_unix_seconds="$(date +%s)"
jq -cn \
  --arg destination "r2://$R2_BUCKET/$prefix" \
  --arg failure_domain "$R2_FAILURE_DOMAIN" \
  --arg segment_object_hash "$ARCHIVE_V2_SEGMENT_OBJECT_HASH" \
  --argjson verified_unix_seconds "$verified_unix_seconds" \
  '{operation:"archive-v2-r2-put",destination:$destination,failure_domain:$failure_domain,segment_object_hash:$segment_object_hash,verified_unix_seconds:$verified_unix_seconds,retirement_evidence:($destination+","+$failure_domain+","+($verified_unix_seconds|tostring))}'
