#!/usr/bin/env bash
set -euo pipefail

# Build a bounded, contiguous Archive V2 catalog tail from a stable hot-state
# checkpoint and a read-only legacy cold store. This script never accesses R2
# and never deletes its scratch roots. Publication is a separate dual-domain
# operation performed by archive-v2-r2-dual-publish.sh.

for tool in jq sha256sum; do
  command -v "$tool" >/dev/null 2>&1 || {
    echo "missing required tool: $tool" >&2
    exit 2
  }
done

: "${ARCHIVE_V2_BINARY:?ARCHIVE_V2_BINARY is required}"
: "${ARCHIVE_V2_SNAPSHOT_STATE_DIR:?ARCHIVE_V2_SNAPSHOT_STATE_DIR is required}"
: "${ARCHIVE_V2_COLD_STORE:?ARCHIVE_V2_COLD_STORE is required}"
: "${ARCHIVE_V2_ROOT:?ARCHIVE_V2_ROOT is required}"
: "${ARCHIVE_V2_REPLICA_ROOT:?ARCHIVE_V2_REPLICA_ROOT is required}"
: "${ARCHIVE_V2_EXPECTED_CATALOG_SHA256:?ARCHIVE_V2_EXPECTED_CATALOG_SHA256 is required}"
: "${ARCHIVE_V2_START_SLOT:?ARCHIVE_V2_START_SLOT is required}"
: "${ARCHIVE_V2_END_SLOT:?ARCHIVE_V2_END_SLOT is required}"
: "${ARCHIVE_V2_SEGMENT_LIST:?ARCHIVE_V2_SEGMENT_LIST is required}"

NETWORK_ID="${ARCHIVE_V2_NETWORK_ID:-lichen-testnet-1}"
GENESIS_HASH="${ARCHIVE_V2_GENESIS_HASH:-f08308ef2520af0967120f3314fa95b14d8239a898d34a6993981cb93f740884}"
SEGMENT_SLOTS="${ARCHIVE_V2_SEGMENT_SLOTS:-50000}"
FINALITY_DEPTH_SLOTS="${ARCHIVE_V2_FINALITY_DEPTH_SLOTS:-50000}"
MAX_SEGMENTS="${ARCHIVE_V2_MAX_SEGMENTS:-32}"
ZSTD_LEVEL="${ARCHIVE_V2_ZSTD_LEVEL:-6}"
FRAME_BYTES="${ARCHIVE_V2_FRAME_BYTES:-1048576}"
MAX_OBJECT_BYTES="${ARCHIVE_V2_MAX_OBJECT_BYTES:-1073741824}"

for value_name in ARCHIVE_V2_START_SLOT ARCHIVE_V2_END_SLOT SEGMENT_SLOTS FINALITY_DEPTH_SLOTS MAX_SEGMENTS FRAME_BYTES MAX_OBJECT_BYTES; do
  value=${!value_name}
  [[ "$value" =~ ^[0-9]+$ ]] || { echo "$value_name must be an unsigned integer" >&2; exit 2; }
done
[[ "$ZSTD_LEVEL" =~ ^-?[0-9]+$ ]] || { echo "ARCHIVE_V2_ZSTD_LEVEL must be an integer" >&2; exit 2; }
[[ "$GENESIS_HASH" =~ ^[0-9a-f]{64}$ ]] || { echo "ARCHIVE_V2_GENESIS_HASH is invalid" >&2; exit 2; }
[[ "$ARCHIVE_V2_EXPECTED_CATALOG_SHA256" =~ ^[0-9a-f]{64}$ ]] || {
  echo "ARCHIVE_V2_EXPECTED_CATALOG_SHA256 is invalid" >&2
  exit 2
}
[ "$SEGMENT_SLOTS" -gt 0 ] && [ "$FINALITY_DEPTH_SLOTS" -gt 0 ] && [ "$MAX_SEGMENTS" -gt 0 ] || {
  echo "segment, finality, and count bounds must be non-zero" >&2
  exit 2
}
[ "$ARCHIVE_V2_END_SLOT" -ge "$ARCHIVE_V2_START_SLOT" ] || {
  echo "Archive V2 tail end precedes its start" >&2
  exit 2
}
range_slots=$((ARCHIVE_V2_END_SLOT - ARCHIVE_V2_START_SLOT + 1))
[ $((ARCHIVE_V2_START_SLOT % SEGMENT_SLOTS)) -eq 0 ] || {
  echo "Archive V2 tail start is not segment aligned" >&2
  exit 2
}
[ $((range_slots % SEGMENT_SLOTS)) -eq 0 ] || {
  echo "Archive V2 tail length is not a whole number of segments" >&2
  exit 2
}
segment_count=$((range_slots / SEGMENT_SLOTS))
[ "$segment_count" -le "$MAX_SEGMENTS" ] || {
  echo "Archive V2 tail exceeds the configured segment bound" >&2
  exit 2
}

for directory in "$ARCHIVE_V2_SNAPSHOT_STATE_DIR" "$ARCHIVE_V2_COLD_STORE" "$ARCHIVE_V2_ROOT" "$ARCHIVE_V2_REPLICA_ROOT"; do
  [ -d "$directory" ] && [ ! -L "$directory" ] || {
    echo "required directory is missing or unsafe: $directory" >&2
    exit 2
  }
done
[ "$ARCHIVE_V2_ROOT" != "$ARCHIVE_V2_REPLICA_ROOT" ] || {
  echo "primary and replica scratch roots must differ" >&2
  exit 2
}
[ ! -e "$ARCHIVE_V2_SEGMENT_LIST" ] && [ ! -L "$ARCHIVE_V2_SEGMENT_LIST" ] || {
  echo "refusing to overwrite segment list" >&2
  exit 2
}

for root in "$ARCHIVE_V2_ROOT" "$ARCHIVE_V2_REPLICA_ROOT"; do
  catalog="$root/catalog.av2"
  [ -f "$catalog" ] && [ ! -L "$catalog" ] || {
    echo "scratch catalog is missing or unsafe: $catalog" >&2
    exit 2
  }
  [ "$(sha256sum "$catalog" | awk '{print $1}')" = "$ARCHIVE_V2_EXPECTED_CATALOG_SHA256" ] || {
    echo "scratch catalog does not match the expected starting catalog" >&2
    exit 2
  }
done

# The checkpoint must be independent of the live DB and have no SST symlinks.
if find "$ARCHIVE_V2_SNAPSHOT_STATE_DIR" -maxdepth 1 -type l -name '*.sst' -print -quit | grep -q .; then
  echo "snapshot contains an unmaterialized SST symlink" >&2
  exit 2
fi

profile_json="$("$ARCHIVE_V2_BINARY" profile-source \
  --state-dir "$ARCHIVE_V2_SNAPSHOT_STATE_DIR" \
  --cold-store "$ARCHIVE_V2_COLD_STORE" \
  --start-slot "$ARCHIVE_V2_START_SLOT" \
  --end-slot "$ARCHIVE_V2_END_SLOT" \
  --top-blocks 1)"
[ "$(jq -er .operation <<<"$profile_json")" = profile_source ]
[ "$(jq -er .start_slot <<<"$profile_json")" = "$ARCHIVE_V2_START_SLOT" ]
[ "$(jq -er .end_slot <<<"$profile_json")" = "$ARCHIVE_V2_END_SLOT" ]
finalized_slot="$(jq -er .finalized_slot <<<"$profile_json")"
[ "$ARCHIVE_V2_END_SLOT" -le $((finalized_slot - FINALITY_DEPTH_SLOTS)) ] || {
  echo "tail end is inside the required finality depth" >&2
  exit 2
}
[ "$(jq -er .genesis_hash <<<"$profile_json")" = "$GENESIS_HASH" ]

umask 077
segment_list_next="$ARCHIVE_V2_SEGMENT_LIST.next"
[ ! -e "$segment_list_next" ] && [ ! -L "$segment_list_next" ] || {
  echo "refusing existing segment list staging file" >&2
  exit 2
}
: >"$segment_list_next"
chmod 0600 "$segment_list_next"

start=$ARCHIVE_V2_START_SLOT
for ((offset = 0; offset < segment_count; offset += 1)); do
  end=$((start + SEGMENT_SLOTS - 1))
  report="$("$ARCHIVE_V2_BINARY" build \
    --state-dir "$ARCHIVE_V2_SNAPSHOT_STATE_DIR" \
    --cold-store "$ARCHIVE_V2_COLD_STORE" \
    --root "$ARCHIVE_V2_ROOT" \
    --network-id "$NETWORK_ID" \
    --genesis-hash "$GENESIS_HASH" \
    --start-slot "$start" \
    --end-slot "$end" \
    --finality-depth-slots "$FINALITY_DEPTH_SLOTS" \
    --zstd-level "$ZSTD_LEVEL" \
    --frame-bytes "$FRAME_BYTES" \
    --replica-root "$ARCHIVE_V2_REPLICA_ROOT" \
    --required-replicas 1 \
    --acknowledge-exact-testnet-missing-watermark)"
  [ "$(jq -er .operation <<<"$report")" = build ]
  [ "$(jq -er .start_slot <<<"$report")" = "$start" ]
  [ "$(jq -er .end_slot <<<"$report")" = "$end" ]
  [ "$(jq -er .promoted <<<"$report")" = true ]
  [ "$(jq -er .replica_acknowledgements <<<"$report")" -ge 1 ]
  [ "$(jq -er .segment_bytes <<<"$report")" -le "$MAX_OBJECT_BYTES" ] || {
    echo "built segment exceeds the verified-cache object limit" >&2
    exit 1
  }
  object_hash="$(jq -er .segment_object_hash <<<"$report")"
  [[ "$object_hash" =~ ^[0-9a-f]{64}$ ]]
  index=$((
    $(jq -er .segments <<<"$("$ARCHIVE_V2_BINARY" status --root "$ARCHIVE_V2_ROOT")") - 1
  ))
  printf '%s\t%s\n' "$index" "$object_hash" >>"$segment_list_next"
  start=$((end + 1))
done

[ "$start" -eq $((ARCHIVE_V2_END_SLOT + 1)) ]
cmp "$ARCHIVE_V2_ROOT/catalog.av2" "$ARCHIVE_V2_REPLICA_ROOT/catalog.av2"
final_status="$("$ARCHIVE_V2_BINARY" status --root "$ARCHIVE_V2_ROOT")"
[ "$(jq -er .operation <<<"$final_status")" = status ]
final_catalog_sha="$(sha256sum "$ARCHIVE_V2_ROOT/catalog.av2" | awk '{print $1}')"
[ "$final_catalog_sha" != "$ARCHIVE_V2_EXPECTED_CATALOG_SHA256" ]
[ "$(wc -l <"$segment_list_next" | tr -d ' ')" = "$segment_count" ]
mv "$segment_list_next" "$ARCHIVE_V2_SEGMENT_LIST"
sync -f "$ARCHIVE_V2_SEGMENT_LIST"

jq -cn \
  --arg operation archive_v2_build_tail \
  --arg previous_catalog_sha256 "$ARCHIVE_V2_EXPECTED_CATALOG_SHA256" \
  --arg catalog_sha256 "$final_catalog_sha" \
  --argjson start_slot "$ARCHIVE_V2_START_SLOT" \
  --argjson end_slot "$ARCHIVE_V2_END_SLOT" \
  --argjson finalized_slot "$finalized_slot" \
  --argjson segment_count "$segment_count" \
  '{operation:$operation,previous_catalog_sha256:$previous_catalog_sha256,catalog_sha256:$catalog_sha256,start_slot:$start_slot,end_slot:$end_slot,finalized_slot:$finalized_slot,segment_count:$segment_count}'
