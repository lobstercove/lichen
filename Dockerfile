# Build stage
FROM rust:1-bookworm AS builder

ENV CARGO_HTTP_MULTIPLEXING=false \
    CARGO_HTTP_TIMEOUT=120 \
    CARGO_NET_RETRY=10 \
    CARGO_REGISTRIES_CRATES_IO_PROTOCOL=sparse

RUN apt-get update && apt-get install -y --no-install-recommends \
    clang libclang-dev && rm -rf /var/lib/apt/lists/*

WORKDIR /build

# Cache dependencies: copy manifests first
COPY Cargo.toml Cargo.lock ./
COPY core/Cargo.toml core/Cargo.toml
COPY validator/Cargo.toml validator/Cargo.toml
COPY rpc/Cargo.toml rpc/Cargo.toml
COPY cli/Cargo.toml cli/Cargo.toml
COPY p2p/Cargo.toml p2p/Cargo.toml
COPY faucet-service/Cargo.toml faucet-service/Cargo.toml
COPY moss-provider/Cargo.toml moss-provider/Cargo.toml
COPY sdk/rust/Cargo.toml sdk/rust/Cargo.toml
COPY custody/Cargo.toml custody/Cargo.toml
COPY genesis/Cargo.toml genesis/Cargo.toml
COPY compiler/Cargo.toml compiler/Cargo.toml
COPY third_party/arrayref/ third_party/arrayref/

# LTO links several large binaries; serialize both build layers by default so
# an 8 GiB release builder cannot kill the validator link under parallel load.
ARG CARGO_BUILD_JOBS=1

# Create dummy source files for dependency caching
RUN set -eu; \
    for target in \
        core/src/lib.rs \
        rpc/src/lib.rs \
        p2p/src/lib.rs \
        sdk/rust/src/lib.rs \
        genesis/src/lib.rs \
        compiler/src/lib.rs; do \
        mkdir -p "$(dirname "$target")"; \
        : > "$target"; \
    done; \
    for target in \
        core/src/bin/lichen-archive-v2.rs \
        validator/src/main.rs \
        rpc/src/bin/bridge_auth_payload.rs \
        rpc/src/bin/keypair_from_seed_byte.rs \
        rpc/src/bin/withdrawal_auth_payload.rs \
        rpc/src/bin/wrapped_burn.rs \
        cli/src/main.rs \
        cli/src/marketplace_demo.rs \
        cli/src/zk_prove.rs \
        cli/src/bin/bountyboard_v2_migrate.rs \
        cli/src/bin/compute_market_v3_migrate.rs \
        cli/src/bin/dex_margin_v2_migrate.rs \
        cli/src/bin/lichenauction_v3_migrate.rs \
        cli/src/bin/lichenmarket_v3_migrate.rs \
        cli/src/bin/protocol_governance_contract_call.rs \
        cli/src/bin/sporepay_v3_migrate.rs \
        cli/src/bin/sporepump_v3_migrate.rs \
        cli/src/bin/sporevault_v2_migrate.rs \
        faucet-service/src/main.rs \
        moss-provider/src/main.rs \
        custody/src/main.rs \
        genesis/src/main.rs; do \
        mkdir -p "$(dirname "$target")"; \
        printf '%s\n' 'fn main() {}' > "$target"; \
    done

# Build dependencies only (cached layer)
RUN cargo build --release --locked --jobs "${CARGO_BUILD_JOBS}"

# Copy real source code
COPY core/ core/
COPY validator/ validator/
COPY rpc/ rpc/
COPY cli/ cli/
COPY p2p/ p2p/
COPY faucet-service/ faucet-service/
COPY moss-provider/ moss-provider/
COPY sdk/rust/ sdk/rust/
COPY custody/ custody/
COPY genesis/ genesis/
COPY compiler/ compiler/
COPY seeds.json ./
COPY shared/incident-guardian-pause-allowlist.json shared/incident-guardian-pause-allowlist.json
COPY contracts/lusd_token/abi.json contracts/lusd_token/abi.json
COPY config.toml .

# Force rebuild with real sources
RUN touch core/src/lib.rs validator/src/main.rs && \
    cargo build --release --locked --jobs "${CARGO_BUILD_JOBS}"

# Runtime stage
FROM debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y --no-install-recommends \
    ca-certificates \
    curl \
    libssl3 \
    && rm -rf /var/lib/apt/lists/*

# Create non-root user
RUN groupadd -r lichen && useradd -r -g lichen -d /home/lichen -m lichen

# Copy binaries
COPY --from=builder /build/target/release/lichen-validator /usr/local/bin/
COPY --from=builder /build/target/release/lichen-genesis /usr/local/bin/
COPY --from=builder /build/target/release/lichen /usr/local/bin/
COPY --from=builder /build/target/release/lichen-faucet /usr/local/bin/
COPY --from=builder /build/target/release/lichen-custody /usr/local/bin/

# Copy default config
COPY config.toml /etc/lichen/config.toml

# Data directory
RUN mkdir -p /var/lib/lichen && chown lichen:lichen /var/lib/lichen

USER lichen
WORKDIR /home/lichen

# P2P port
EXPOSE 7001
# RPC port
EXPOSE 8899
# WebSocket port
EXPOSE 8900
# Validator Metrics port
EXPOSE 9100
# Faucet port (when running lichen-faucet entrypoint)
EXPOSE 9101

ENV LICHEN_DATA_DIR=/var/lib/lichen
ENV LICHEN_CONFIG=/etc/lichen/config.toml
ENV RUST_LOG=info

VOLUME ["/var/lib/lichen"]

HEALTHCHECK --interval=30s --timeout=10s --start-period=15s --retries=3 \
    CMD curl -sf http://localhost:8899/ -d '{"jsonrpc":"2.0","id":1,"method":"getHealth"}' -H 'Content-Type: application/json' || exit 1

ENTRYPOINT ["lichen-validator"]
CMD ["--db-path", "/var/lib/lichen"]
