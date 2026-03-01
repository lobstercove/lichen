#!/usr/bin/env bash
# T-MKT-008: Validate ?filter=featured browse filter (matrix test)
# Validates MKT-H04 — featured/creators URL params are parsed and applied
set -euo pipefail

cd "$(dirname "$0")/.." || exit 1
PASS=0
FAIL=0
TOTAL=0

check() {
  TOTAL=$((TOTAL+1))
  local label="$1"; shift
  if "$@" >/dev/null 2>&1; then
    PASS=$((PASS+1))
    echo "  ✅ $label"
  else
    FAIL=$((FAIL+1))
    echo "  ❌ $label"
  fi
}

echo "=== T-MKT-008: Featured Filter Validation ==="

BROWSE_JS="marketplace/js/browse.js"
INDEX_HTML="marketplace/index.html"

# 1. browse.js parses filter=featured from URL params
check "browse.js reads URLSearchParams filter param" \
  grep -q "params.get('filter')" "$BROWSE_JS"

# 2. browse.js has the featured filter mode guard
check "browse.js checks filterMode === 'featured'" \
  grep -q "filterMode === 'featured'" "$BROWSE_JS"

# 3. browse.js applies featured filter logic (checks featured/is_featured/verified/rarity)
check "browse.js applies featured item filter (is_featured/verified/rarity)" \
  grep -q "item.featured === true\|item.is_featured === true\|item.verified === true" "$BROWSE_JS"

# 4. browse.js applies creators filter mode
check "browse.js checks filterMode === 'creators'" \
  grep -q "filterMode === 'creators'" "$BROWSE_JS"

# 5. index.html has link to browse.html?filter=featured
check "index.html links to browse.html?filter=featured" \
  grep -q 'browse.html?filter=featured' "$INDEX_HTML"

# 6. urlFilterMode variable is declared
check "browse.js declares urlFilterMode variable" \
  grep -q "urlFilterMode" "$BROWSE_JS"

# 7. Featured filter checks rarity values (epic/legendary fallback)
check "browse.js featured filter has rarity fallback (epic/legendary)" \
  grep -q "legendary" "$BROWSE_JS"

# 8. Live RPC available (cluster prerequisite)
check "RPC available at 8899" \
  curl -sf --connect-timeout 2 --max-time 3 http://127.0.0.1:8899 \
    -X POST -H 'Content-Type: application/json' \
    -d '{"jsonrpc":"2.0","id":1,"method":"getSlot","params":[]}'

# 9. Marketplace static files exist
check "marketplace/browse.html exists" test -f marketplace/browse.html
check "marketplace/js/browse.js exists" test -f "$BROWSE_JS"

# 10. Marketplace listings endpoint responds (validates backend serves filter data)
check "getMarketListings RPC method responds" \
  bash -c 'resp=$(curl -sf --connect-timeout 2 --max-time 5 http://127.0.0.1:8899 \
    -X POST -H "Content-Type: application/json" \
    -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getMarketListings\",\"params\":[{\"limit\":5}]}"); echo "$resp" | grep -q "result"'

echo ""
echo "T-MKT-008: TOTAL=$TOTAL PASS=$PASS FAIL=$FAIL"
[[ "$FAIL" -eq 0 ]] && exit 0 || exit 1
