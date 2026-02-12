#!/bin/bash
# JavaScript SDK Dependency Fix Script
# Tries multiple methods to install dependencies

set -e

echo "🦞 JavaScript SDK Dependency Fix"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

cd "$(dirname "$0")/js-sdk"

echo "📂 Working directory: $(pwd)"
echo ""

# Method 1: Clean npm install
echo "1️⃣  Method 1: Clean npm cache and install"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

if [ -d "node_modules" ]; then
    echo "   Removing old node_modules..."
    rm -rf node_modules
fi

if [ -f "package-lock.json" ]; then
    echo "   Removing package-lock.json..."
    rm -f package-lock.json
fi

echo "   Cleaning npm cache..."
npm cache clean --force 2>/dev/null || true

echo "   Installing dependencies with --prefer-offline..."
if timeout 30 npm install --prefer-offline --no-audit --no-fund 2>&1; then
    echo "   ✅ npm install succeeded!"
    SUCCESS=true
else
    echo "   ⏸️  npm install timed out or failed"
    SUCCESS=false
fi

# Method 2: Try yarn if npm failed
if [ "$SUCCESS" = false ]; then
    echo ""
    echo "2️⃣  Method 2: Try yarn"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    
    if command -v yarn &> /dev/null; then
        echo "   Found yarn, attempting install..."
        if timeout 30 yarn install --prefer-offline 2>&1; then
            echo "   ✅ yarn install succeeded!"
            SUCCESS=true
        else
            echo "   ⏸️  yarn install timed out or failed"
        fi
    else
        echo "   ℹ️  yarn not found, skipping"
    fi
fi

# Method 3: Manual package installation
if [ "$SUCCESS" = false ]; then
    echo ""
    echo "3️⃣  Method 3: Manual package installation"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    
    mkdir -p node_modules/.tmp
    
    PACKAGES=(
        "tweetnacl@1.0.3"
        "bs58@5.0.0"
        "axios@1.6.0"
        "buffer@6.0.3"
    )
    
    for pkg in "${PACKAGES[@]}"; do
        echo "   Installing $pkg..."
        timeout 10 npm install "$pkg" --no-save --prefer-offline 2>&1 || echo "   ⚠️  Failed: $pkg"
    done
    
    # Check if critical packages are present
    if [ -d "node_modules/tweetnacl" ] && [ -d "node_modules/bs58" ] && [ -d "node_modules/axios" ]; then
        echo "   ✅ Core dependencies installed!"
        SUCCESS=true
    else
        echo "   ❌ Core dependencies missing"
    fi
fi

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "📋 Checking installed packages"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

for pkg in tweetnacl bs58 axios buffer; do
    if [ -d "node_modules/$pkg" ]; then
        echo "   ✅ $pkg"
    else
        echo "   ❌ $pkg (MISSING)"
    fi
done

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "🔧 Fixing tsconfig.json (TextEncoder)"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

if [ -f "tsconfig.json" ]; then
    # Add DOM to lib array if not present
    if ! grep -q '"DOM"' tsconfig.json; then
        echo "   Adding DOM to tsconfig lib array..."
        sed -i.bak 's/"lib": \["ES2020"\]/"lib": ["ES2020", "DOM"]/' tsconfig.json
        echo "   ✅ tsconfig.json updated"
    else
        echo "   ℹ️  DOM already in tsconfig"
    fi
else
    echo "   ⚠️  tsconfig.json not found"
fi

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "🏗️  Building SDK"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

if [ -d "node_modules/typescript" ] || which tsc &> /dev/null; then
    echo "   Running: tsc"
    
    if npx tsc 2>&1; then
        echo "   ✅ Build succeeded!"
        
        if [ -f "dist/index.js" ]; then
            echo "   ✅ dist/index.js created"
            echo "   📦 SDK ready to use!"
        else
            echo "   ⚠️  dist/index.js not found"
        fi
    else
        echo "   ⚠️  Build failed (see errors above)"
    fi
else
    echo "   ⚠️  TypeScript not found, skipping build"
    echo "   Run: npm install -g typescript"
fi

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "✨ SDK Fix Complete"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

if [ "$SUCCESS" = true ] && [ -f "dist/index.js" ]; then
    echo "🎉 JavaScript SDK is ready!"
    exit 0
else
    echo "⚠️  SDK needs manual attention"
    exit 1
fi
