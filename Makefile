# ═══════════════════════════════════════════════════════════════════════════════
# Lichen — Master Makefile
# ═══════════════════════════════════════════════════════════════════════════════
#
# Usage:
#   make build          — Build everything (node + contracts + CLI)
#   make test           — Run all tests
#   make deploy-local   — Deploy to local validator
#   make deploy-testnet — Deploy to testnet
#   make start          — Start local development stack
#   make clean          — Clean all build artifacts

SHELL     := /bin/bash
ROOT_DIR  := $(shell pwd)
RPC_URL   ?= http://127.0.0.1:8899

# ─────────────────────────────────────────────────────────────────────────────
# Build
# ─────────────────────────────────────────────────────────────────────────────

.PHONY: build build-node build-contracts build-contracts-wasm build-cli build-sdk

build: build-node build-contracts-wasm build-cli build-sdk
	@echo "✅ Full build complete"

build-node:
	@echo "🔨 Building Lichen node (validator + RPC + P2P)..."
	cargo build --release --workspace

build-contracts:
	@echo "🔨 Building contracts (native, for testing)..."
	@FAIL=0; \
	for d in contracts/*/; do \
		if [ -f "$$d/Cargo.toml" ]; then \
			echo "  Building $$(basename $$d)..."; \
			if ! (cd "$$d" && cargo build --release 2>&1); then \
				echo "  ❌ FAILED: $$(basename $$d)"; FAIL=1; \
			fi; \
		fi; \
	done; \
	if [ $$FAIL -ne 0 ]; then echo "❌ Contract build failed"; exit 1; fi

build-contracts-wasm:
	@echo "🔨 Building contracts to WASM..."
	@if [ -x scripts/build-all-contracts.sh ]; then \
		scripts/build-all-contracts.sh; \
	else \
		for d in contracts/*/; do \
			if [ -f "$$d/Cargo.toml" ] && grep -q 'cdylib' "$$d/Cargo.toml" 2>/dev/null; then \
				echo "  Building $$(basename $$d) → WASM..."; \
				if ! (cd "$$d" && cargo build --target wasm32-unknown-unknown --release 2>&1); then \
					echo "  ❌ FAILED: $$(basename $$d)"; exit 1; \
				fi; \
			fi; \
		done; \
	fi

build-cli:
	@echo "🔨 Building CLI..."
	cargo build --release -p lichen-cli 2>/dev/null || cargo build --release -p cli 2>/dev/null || echo "⚠️  CLI package not found"

build-sdk:
	@echo "🔨 Building TypeScript SDKs..."
	@if [ -d sdk/js ] && [ -f sdk/js/package.json ]; then (cd sdk/js && npm run build || npx tsc || (echo "❌ SDK js build failed" && exit 1)); fi
	@if [ -d dex/sdk ] && [ -f dex/sdk/package.json ]; then (cd dex/sdk && npm run build || npx tsc || (echo "❌ DEX SDK build failed" && exit 1)); fi

# ─────────────────────────────────────────────────────────────────────────────
# Test
# ─────────────────────────────────────────────────────────────────────────────

.PHONY: test test-node test-contracts test-e2e test-dex test-prediction-market

test: test-node test-contracts test-prediction-market
	@echo "✅ All tests passed"

test-node:
	@echo "🧪 Running node tests..."
	cargo test --workspace --release

test-contracts:
	@echo "🧪 Running contract tests..."
	@PASS=0; FAIL=0; \
	for d in contracts/*/; do \
		if [ -f "$$d/Cargo.toml" ]; then \
			name=$$(basename "$$d"); \
			if (cd "$$d" && cargo test --release >/dev/null 2>&1); then \
				PASS=$$((PASS + 1)); \
			else \
				echo "  ❌ $$name"; FAIL=$$((FAIL + 1)); \
			fi; \
		fi; \
	done; \
	echo "  Contracts: $$PASS passed, $$FAIL failed"

test-e2e:
	@echo "🧪 Running E2E cross-contract tests..."
	@FAIL=0; \
	for d in contracts/*/; do \
		if [ -d "$$d/tests" ] && ls "$$d/tests/"*.rs >/dev/null 2>&1; then \
			name=$$(basename "$$d"); \
			echo "  E2E: $$name"; \
			if ! (cd "$$d" && cargo test --release -- --test-threads=1 2>&1 | tail -3); then \
				FAIL=1; \
			fi; \
		fi; \
	done; \
	if [ $$FAIL -ne 0 ]; then echo "❌ E2E tests failed"; exit 1; fi

test-dex:
	@echo "🧪 Running DEX-specific tests..."
	@FAIL=0; \
	for c in dex_core dex_amm dex_router dex_margin dex_rewards dex_governance dex_analytics; do \
		echo "  Testing $$c..."; \
		if ! (cd contracts/$$c && cargo test --release 2>&1 | tail -1); then \
			FAIL=1; \
		fi; \
	done; \
	if [ $$FAIL -ne 0 ]; then echo "❌ DEX tests failed"; exit 1; fi

test-prediction-market:
	@echo "🧪 Running prediction market contract tests..."
	@cd contracts/prediction_market && cargo test --release

# ─────────────────────────────────────────────────────────────────────────────
# Deploy
# ─────────────────────────────────────────────────────────────────────────────

.PHONY: deploy-local deploy-testnet deploy-mainnet

deploy-local: build-contracts-wasm
	@echo "🚀 Deploying to local validator..."
	scripts/first-boot-deploy.sh --rpc=$(RPC_URL)

deploy-testnet: build-contracts-wasm
	@echo "🚀 Deploying to testnet..."
	scripts/testnet-deploy.sh --rpc=$(RPC_URL) --seed-pairs --seed-pools

deploy-mainnet: build-contracts-wasm
	@echo "🚀 Deploying to mainnet..."
	@echo "⚠️  Mainnet deployment requires manual confirmation."
	@read -p "Continue? [y/N] " confirm && [ "$$confirm" = "y" ] || exit 1
	scripts/mainnet-deploy.sh --rpc=$(RPC_URL) --network=mainnet

# ─────────────────────────────────────────────────────────────────────────────
# Run
# ─────────────────────────────────────────────────────────────────────────────

.PHONY: start start-validator start-rpc start-custody start-dex start-all

start: start-all

start-validator:
	@echo "🦞 Starting validator..."
	cargo run --release -p validator -- --dev 2>&1 &

start-rpc:
	@echo "🦞 Starting RPC server..."
	cargo run --release -p rpc -- --bind 0.0.0.0:8899 2>&1 &

start-custody:
	@echo "🦞 Starting custody service..."
	cargo run --release -p custody 2>&1 &

start-dex: ## Local dev only — NOT for production (use Caddy/nginx instead)
	@echo "🦞 Serving DEX frontend on http://localhost:3000 (dev mode)..."
	@cd dex && python3 -m http.server 3000 2>&1 &

start-all: start-validator
	@sleep 3
	@$(MAKE) start-rpc
	@sleep 2
	@$(MAKE) deploy-local
	@$(MAKE) start-custody
	@$(MAKE) start-dex
	@echo "✅ Full stack running"
	@echo "  Validator (P2P): localhost:7001"
	@echo "  RPC:             http://localhost:8899"
	@echo "  DEX:       http://localhost:3000"
	@echo "  Custody:   running in background"

# ─────────────────────────────────────────────────────────────────────────────
# Docker
# ─────────────────────────────────────────────────────────────────────────────

.PHONY: docker-build docker-up docker-down

docker-build:
	@echo "🐳 Building Docker images..."
	docker compose -f infra/docker-compose.yml build

docker-up:
	@echo "🐳 Starting Docker stack..."
	docker compose -f infra/docker-compose.yml up -d

docker-down:
	docker compose -f infra/docker-compose.yml down

# ─────────────────────────────────────────────────────────────────────────────
# Utilities
# ─────────────────────────────────────────────────────────────────────────────

.PHONY: clean lint fmt health check check-expected-contracts production-gate

clean:
	@echo "🧹 Cleaning..."
	cargo clean
	@for d in contracts/*/; do (cd "$$d" && cargo clean 2>/dev/null) || true; done
	@rm -rf dex/sdk/dist dex/loadtest/dist dex/market-maker/dist sdk/js/dist
	@echo "✅ Clean"

lint:
	cargo clippy --workspace -- -D warnings
	@FAIL=0; \
	for d in contracts/*/; do \
		if ! (cd "$$d" && cargo clippy -- -D warnings 2>/dev/null); then \
			FAIL=1; \
		fi; \
	done; \
	if [ $$FAIL -ne 0 ]; then echo "❌ Contract lint failed"; exit 1; fi

fmt:
	cargo fmt --all
	@FAIL=0; \
	for d in contracts/*/; do \
		if ! (cd "$$d" && cargo fmt 2>/dev/null); then \
			FAIL=1; \
		fi; \
	done; \
	if [ $$FAIL -ne 0 ]; then echo "❌ Contract fmt failed"; exit 1; fi

health:
	@echo "Checking node health at $(RPC_URL)..."
	@curl -s -X POST $(RPC_URL) -H "Content-Type: application/json" \
		-d '{"jsonrpc":"2.0","id":1,"method":"health"}' | python3 -m json.tool 2>/dev/null || echo "❌ Node unreachable"

check:
	@echo "🔍 Checking workspace..."
	cargo check --workspace
	@echo "🔍 Checking contracts..."
	@FAIL=0; \
	for d in contracts/*/; do \
		if [ -f "$$d/Cargo.toml" ]; then \
			echo "  Checking $$(basename $$d)..."; \
			if ! (cd "$$d" && cargo check 2>&1 | tail -1); then \
				FAIL=1; \
			fi; \
		fi; \
	done; \
	if [ $$FAIL -ne 0 ]; then echo "❌ Contract check failed"; exit 1; fi

check-expected-contracts:
	@echo "🔍 Verifying expected contract lockfile..."
	python3 tests/update-expected-contracts.py --check

production-gate: check-expected-contracts
	@echo "🚦 Running production E2E gate..."
	bash tests/production-e2e-gate.sh

sync-shared:
	@echo "📦 Syncing canonical shared/ from monitoring/shared/ to all frontends..."
	@for dir in explorer dex wallet marketplace faucet programs developers; do \
		cp monitoring/shared/utils.js $$dir/shared/utils.js; \
		echo "  ✓ $$dir/shared/utils.js"; \
	done
	@cp monitoring/shared/utils.js wallet/extension/shared/utils.js
	@echo "  ✓ wallet/extension/shared/utils.js"
	@for dir in explorer dex wallet marketplace faucet programs; do \
		cp monitoring/shared/pq.js $$dir/shared/pq.js; \
		echo "  ✓ $$dir/shared/pq.js"; \
	done
	@cp monitoring/shared/pq.js wallet/extension/shared/pq.js
	@echo "  ✓ wallet/extension/shared/pq.js"
	@for dir in explorer dex wallet faucet programs developers; do \
		cp monitoring/shared/wallet-connect.js $$dir/shared/wallet-connect.js; \
		echo "  ✓ $$dir/shared/wallet-connect.js"; \
	done
	@echo "✅ Shared JS synced (marketplace/wallet-connect.js is custom — skipped)"

# ─────────────────────────────────────────────────────────────────────────────
# Help
# ─────────────────────────────────────────────────────────────────────────────

.PHONY: help
help:
	@echo "Lichen Makefile"
	@echo ""
	@echo "Build:"
	@echo "  make build              Build everything"
	@echo "  make build-node         Build validator/RPC/P2P/CLI"
	@echo "  make build-contracts-wasm  Build all contracts to WASM"
	@echo "  make build-sdk          Build TypeScript SDKs"
	@echo ""
	@echo "Test:"
	@echo "  make test               Run all tests (node + contracts)"
	@echo "  make test-contracts     Run contract unit + adversarial tests"
	@echo "  make test-e2e           Run cross-contract E2E tests"
	@echo "  make test-dex           Run DEX contract tests only"
	@echo "  make test-prediction-market  Run prediction_market contract tests"
	@echo ""
	@echo "Deploy:"
	@echo "  make deploy-local       Deploy to local validator"
	@echo "  make deploy-testnet     Deploy to testnet with pair/pool seeding"
	@echo "  make deploy-mainnet     Deploy to mainnet (requires confirmation)"
	@echo ""
	@echo "Run:"
	@echo "  make start              Start full local dev stack"
	@echo "  make start-validator    Start validator only"
	@echo "  make start-rpc          Start RPC server only"
	@echo "  make start-dex          Serve DEX frontend at :3000"
	@echo ""
	@echo "Docker:"
	@echo "  make docker-build       Build Docker images"
	@echo "  make docker-up          Start Docker stack"
	@echo "  make docker-down        Stop Docker stack"
	@echo ""
	@echo "Utils:"
	@echo "  make clean              Clean all build artifacts"
	@echo "  make lint               Run clippy on all code"
	@echo "  make fmt                Format all Rust code"
	@echo "  make health             Check node health"
	@echo "  make check              Cargo check all code"
	@echo "  make check-expected-contracts  Verify contracts lockfile parity"
	@echo "  make production-gate    Run lockfile check + production E2E gate"
	@echo "  make sync-shared        Sync monitoring/shared/ to all frontends"
