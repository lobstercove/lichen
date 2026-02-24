#!/bin/bash
# Final warning check for all MoltChain contracts

echo "🦞⚡ FINAL WARNING CHECK - ALL CONTRACTS ⚡🦞"
echo ""

total_warnings=0
contracts=("moltcoin" "moltpunks" "moltswap" "moltmarket" "moltauction" "moltoracle" "moltdao")

for contract in "${contracts[@]}"; do
    echo "Checking $contract..."
    SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
    ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
    cd "$ROOT_DIR/contracts/$contract"
    
    # Build and capture warnings
    warnings=$(cargo build --target wasm32-unknown-unknown --release 2>&1 | grep "warning:" | grep -v "profiles for the non root" | wc -l | tr -d ' ')
    
    if [ "$warnings" -gt 0 ]; then
        echo "  ⚠️  $warnings warning(s) found"
        total_warnings=$((total_warnings + warnings))
    else
        echo "  ✅ No warnings!"
    fi
done

echo ""
echo "================================"
if [ "$total_warnings" -eq 0 ]; then
    echo "✅ ALL CONTRACTS CLEAN!"
    echo "🦞 No warnings - Ready to molt harder! ⚡"
else
    echo "⚠️  Total warnings: $total_warnings"
fi
echo "================================"
echo ""

# Show final contract sizes
echo "📦 PRODUCTION CONTRACT SIZES:"
ls -lh "$ROOT_DIR"/contracts/*/target/wasm32-unknown-unknown/release/*.wasm 2>/dev/null | awk '{printf "  %-30s %6s\n", $9, $5}' | sed 's|.*/||' | sort

exit $total_warnings
