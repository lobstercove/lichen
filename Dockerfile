# Build stage
FROM rust:1.75-bookworm AS builder

WORKDIR /build

# Cache dependencies: copy manifests first
COPY Cargo.toml Cargo.lock ./
COPY core/Cargo.toml core/Cargo.toml
COPY validator/Cargo.toml validator/Cargo.toml
COPY rpc/Cargo.toml rpc/Cargo.toml
COPY cli/Cargo.toml cli/Cargo.toml
COPY p2p/Cargo.toml p2p/Cargo.toml
COPY faucet-service/Cargo.toml faucet-service/Cargo.toml
COPY custody/Cargo.toml custody/Cargo.toml
COPY genesis/Cargo.toml genesis/Cargo.toml
COPY compiler/Cargo.toml compiler/Cargo.toml

# Create dummy source files for dependency caching
RUN mkdir -p core/src validator/src rpc/src cli/src p2p/src faucet-service/src custody/src genesis/src compiler/src && \
    echo "fn main() {}" > validator/src/main.rs && \
    echo "fn main() {}" > cli/src/main.rs && \
    echo "fn main() {}" > faucet-service/src/main.rs && \
    echo "fn main() {}" > genesis/src/main.rs && \
    echo "" > core/src/lib.rs && \
    echo "" > rpc/src/lib.rs && \
    echo "" > p2p/src/lib.rs && \
    echo "" > custody/src/lib.rs && \
    echo "" > compiler/src/lib.rs

# Build dependencies only (cached layer)
RUN cargo build --release 2>/dev/null || true

# Copy real source code
COPY core/ core/
COPY validator/ validator/
COPY rpc/ rpc/
COPY cli/ cli/
COPY p2p/ p2p/
COPY faucet-service/ faucet-service/
COPY custody/ custody/
COPY genesis/ genesis/
COPY compiler/ compiler/
COPY config.toml .

# Force rebuild with real sources
RUN touch core/src/lib.rs validator/src/main.rs && \
    cargo build --release

# Runtime stage
FROM debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y --no-install-recommends \
    ca-certificates \
    curl \
    libssl3 \
    && rm -rf /var/lib/apt/lists/*

# Create non-root user
RUN groupadd -r moltchain && useradd -r -g moltchain -d /home/moltchain -m moltchain

# Copy binaries
COPY --from=builder /build/target/release/moltchain-validator /usr/local/bin/
COPY --from=builder /build/target/release/molt-cli /usr/local/bin/
COPY --from=builder /build/target/release/moltchain-faucet /usr/local/bin/
COPY --from=builder /build/target/release/moltchain-custody /usr/local/bin/

# Copy default config
COPY config.toml /etc/moltchain/config.toml

# Data directory
RUN mkdir -p /var/lib/moltchain && chown moltchain:moltchain /var/lib/moltchain

USER moltchain
WORKDIR /home/moltchain

# P2P port
EXPOSE 7001
# RPC port
EXPOSE 8899
# WebSocket port
EXPOSE 8900
# Validator Metrics port
EXPOSE 9100
# Faucet port (when running moltchain-faucet entrypoint)
EXPOSE 9101

ENV MOLTCHAIN_DATA_DIR=/var/lib/moltchain
ENV MOLTCHAIN_CONFIG=/etc/moltchain/config.toml
ENV RUST_LOG=info

VOLUME ["/var/lib/moltchain"]

HEALTHCHECK --interval=30s --timeout=10s --start-period=15s --retries=3 \
    CMD curl -sf http://localhost:8899/ -d '{"jsonrpc":"2.0","id":1,"method":"getHealth"}' -H 'Content-Type: application/json' || exit 1

ENTRYPOINT ["moltchain-validator"]
CMD ["--data-dir", "/var/lib/moltchain"]
