#!/usr/bin/env bash
# Deploy all Lichen frontends to Cloudflare Pages
# Usage: ./scripts/deploy-cloudflare-pages.sh [project-name]
# If project-name is given, only that project is deployed.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
STAGING="$(mktemp -d "${TMPDIR:-/tmp}/lichen-cf-staging.XXXXXX")"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
CYAN='\033[0;36m'
NC='\033[0m'

ok()   { printf "${GREEN}✓ %s${NC}\n" "$1"; }
info() { printf "${CYAN}→ %s${NC}\n" "$1"; }
fail() { printf "${RED}✗ %s${NC}\n" "$1"; exit 1; }

# Project list: "local_dir:cf_project_name"
PROJECTS=(
  "website:lichen-network-website"
  "explorer:lichen-network-explorer"
  "wallet:lichen-network-wallet"
  "dex:lichen-network-dex"
  "marketplace:lichen-network-marketplace"
  "programs:lichen-network-programs"
  "developers:lichen-network-developers"
  "monitoring:lichen-network-monitoring"
  "faucet:lichen-network-faucet"
)

run_predeploy_checks() {
  info "Running frontend asset integrity audit"
  (
    cd "$REPO_ROOT"
    node scripts/qa/test_frontend_asset_integrity.js
  )
  ok "Frontend predeploy checks complete"
  echo ""
}

get_cf_name() {
  for entry in "${PROJECTS[@]}"; do
    local key="${entry%%:*}"
    local val="${entry#*:}"
    if [[ "$key" == "$1" ]]; then
      echo "$val"
      return 0
    fi
  done
  return 1
}

get_extra_excludes() {
  case "$1" in
    wallet)   echo "extension" ;;
    dex)      echo "loadtest market-maker sdk dex.test.js" ;;
    programs) echo "programs.test.js deploy-services.sh" ;;
    faucet)   echo "src faucet.test.js" ;;
    *)        echo "" ;;
  esac
}

get_required_stage_assets() {
  case "$1" in
    dex) echo "charting_library/charting_library.standalone.js charting_library/bundles" ;;
    *)   echo "" ;;
  esac
}

verify_stage_assets() {
  local name="$1"
  local stage_dir="$2"
  local required_assets=""
  local asset=""

  required_assets="$(get_required_stage_assets "$name")"
  if [[ -z "$required_assets" ]]; then
    return 0
  fi

  for asset in $required_assets; do
    if [[ ! -e "$stage_dir/$asset" ]]; then
      fail "Staged deploy for $name is missing required asset: $asset"
    fi
  done

  ok "$name staged assets verified"
}

deploy_project() {
  local name="$1"
  local cf_name
  cf_name="$(get_cf_name "$name")" || fail "Unknown project: $name"
  local src_dir="${REPO_ROOT}/${name}"
  local stage_dir="${STAGING}/${name}"

  if [[ ! -d "$src_dir" ]]; then
    fail "Source directory not found: $src_dir"
  fi

  info "Deploying ${name} → ${cf_name}"

  # Build rsync exclude args
  local -a excludes=(--exclude .DS_Store --exclude .git --exclude node_modules --exclude __pycache__ --exclude "*.pyc")

  local extra
  extra="$(get_extra_excludes "$name")"
  if [[ -n "$extra" ]]; then
    for e in $extra; do
      excludes+=(--exclude "$e")
    done
  fi

  # Stage: rsync to temp dir (clean copy)
  rm -rf "$stage_dir"
  mkdir -p "$stage_dir"
  rsync -a "${excludes[@]}" "${src_dir}/" "${stage_dir}/"
  verify_stage_assets "$name" "$stage_dir"

  # Deploy
  npx wrangler pages deploy . \
    --cwd "$stage_dir" \
    --project-name "$cf_name" \
    --branch main \
    --commit-message "Deploy $(date +%Y-%m-%d)" \
    --commit-dirty=true

  ok "${name} deployed to ${cf_name}"
  echo ""
}

# Cleanup staging on exit
cleanup() { rm -rf "$STAGING"; }
trap cleanup EXIT

# All project names in order
ALL_NAMES=()
for entry in "${PROJECTS[@]}"; do
  ALL_NAMES+=("${entry%%:*}")
done

# Main
run_predeploy_checks

if [[ $# -gt 0 ]]; then
  # Deploy single project
  deploy_project "$1"
else
  # Deploy all
  info "Deploying all ${#PROJECTS[@]} frontends to Cloudflare Pages"
  echo ""
  for name in "${ALL_NAMES[@]}"; do
    deploy_project "$name"
  done
  ok "All ${#PROJECTS[@]} frontends deployed!"
fi
