#!/usr/bin/env bash
set -euo pipefail

if (( $# > 1 )); then
    echo "usage: $0 [npm-prefix]" >&2
    exit 2
fi

audit_log="$(mktemp)"
trap 'rm -f "$audit_log"' EXIT

for attempt in 1 2 3; do
    : > "$audit_log"
    if [[ $# -eq 1 ]]; then
        if npm --prefix "$1" --fetch-retries=0 --fetch-timeout=60000 audit 2>&1 \
            | tee "$audit_log"; then
            exit 0
        fi
    elif npm --fetch-retries=0 --fetch-timeout=60000 audit 2>&1 | tee "$audit_log"; then
        exit 0
    fi

    # Dependency findings are deterministic release failures and must not be
    # softened. Retry only registry/network availability errors, then preserve
    # npm's failure if the authoritative audit endpoint remains unavailable.
    if ! grep -Eqi \
        '(^|[^0-9])5[0-9]{2} (Bad Gateway|Gateway Timeout|Service Unavailable)|EAI_AGAIN|ECONNRESET|ECONNREFUSED|ETIMEDOUT|network timeout|audit endpoint returned an error' \
        "$audit_log"; then
        exit 1
    fi
    if (( attempt == 3 )); then
        echo "npm audit endpoint remained unavailable after ${attempt} attempts" >&2
        exit 1
    fi

    delay=$((attempt * 10))
    echo "Transient npm audit service failure; retrying in ${delay}s (${attempt}/3)" >&2
    sleep "$delay"
done
