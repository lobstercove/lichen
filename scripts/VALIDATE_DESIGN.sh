#!/bin/bash
# Design System Validation Script
# Checks that all CSS files properly import theme.css

echo "🦞 Lichen Design System Validator"
echo "======================================"
echo ""

errors=0
warnings=0

# Check if theme.css exists
if [ ! -f "shared/theme.css" ]; then
    echo "❌ ERROR: shared/theme.css not found!"
    errors=$((errors + 1))
else
    echo "✅ shared/theme.css exists"
fi

# Check component CSS files import theme
echo ""
echo "Checking CSS imports..."
echo ""

components=("website/website.css" "explorer/css/explorer.css" "wallet/wallet.css" "marketplace/marketplace.css" "programs/programs.css" "faucet/faucet.css")

for css in "${components[@]}"; do
    if [ -f "$css" ]; then
        if grep -q "@import.*theme.css" "$css"; then
            echo "✅ $css imports theme.css"
        else
            echo "❌ $css does NOT import theme.css"
            errors=$((errors + 1))
        fi
    else
        echo "⚠️  $css not found"
        warnings=$((warnings + 1))
    fi
done

# Check HTML files link to correct CSS
echo ""
echo "Checking HTML CSS links..."
echo ""

# Website
if grep -q 'href="website.css"' website/index.html 2>/dev/null; then
    echo "✅ website/index.html links to website.css"
else
    echo "❌ website/index.html CSS link incorrect"
    errors=$((errors + 1))
fi

# Explorer
explorer_count=$(grep -l 'href="css/explorer.css"' explorer/*.html 2>/dev/null | wc -l)
if [ "$explorer_count" -gt 0 ]; then
    echo "✅ $explorer_count explorer HTML files link to css/explorer.css"
else
    echo "❌ No explorer HTML files link to css/explorer.css"
    errors=$((errors + 1))
fi

# Wallet
if grep -q 'href="wallet.css"' wallet/index.html 2>/dev/null; then
    echo "✅ wallet/index.html links to wallet.css"
else
    echo "❌ wallet/index.html CSS link incorrect"
    errors=$((errors + 1))
fi

# Marketplace
if grep -q 'href="marketplace.css"' marketplace/index.html 2>/dev/null; then
    echo "✅ marketplace/index.html links to marketplace.css"
else
    echo "❌ marketplace/index.html CSS link incorrect"
    errors=$((errors + 1))
fi

# Programs
if grep -q 'href="programs.css"' programs/index.html 2>/dev/null; then
    echo "✅ programs/index.html links to programs.css"
else
    echo "❌ programs/index.html CSS link incorrect"
    errors=$((errors + 1))
fi

# Faucet
if grep -q 'href="faucet.css"' faucet/index.html 2>/dev/null; then
    echo "✅ faucet/index.html links to faucet.css"
else
    echo "❌ faucet/index.html CSS link incorrect"
    errors=$((errors + 1))
fi

# Summary
echo ""
echo "======================================"
echo "Validation Complete"
echo "======================================"
echo "Errors: $errors"
echo "Warnings: $warnings"
echo ""

if [ $errors -eq 0 ]; then
    echo "✅ All checks passed! Design system is correctly configured."
    exit 0
else
    echo "❌ Some checks failed. Please review errors above."
    exit 1
fi
