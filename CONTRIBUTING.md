# Contributing to MoltChain

Welcome! We're glad you're interested in contributing to MoltChain. This guide
will help you get started.

## Development Environment

### Prerequisites

- **Rust** (stable, edition 2021) — install via [rustup](https://rustup.rs/)
- **WASM target** for smart-contract builds:
  ```bash
  rustup target add wasm32-unknown-unknown
  ```
- **RocksDB** development libraries (usually pulled automatically by the
  `rocksdb` crate, but on some Linux distros you may need `librocksdb-dev` or
  equivalent)

### Clone & Build

```bash
git clone https://github.com/moltchain/moltchain.git
cd moltchain
cargo build --workspace
```

## Running Tests

### Unit & Integration Tests

```bash
cargo test --workspace
```

### Benchmarks

```bash
cargo bench --bench processor_bench
```

### RPC & CLI Integration Tests

Start the validator, then run:

```bash
cargo build --release --bin moltchain-validator
./target/release/moltchain-validator &
bash test-rpc-comprehensive.sh
bash test-cli-comprehensive.sh
```

## Code Quality

### Formatting

All code must be formatted with `rustfmt`:

```bash
cargo fmt --all          # auto-format
cargo fmt --all -- --check  # CI check
```

### Linting

Clippy must pass with zero warnings:

```bash
cargo clippy --workspace -- -D warnings
```

### Pre-commit Checklist

1. `cargo fmt --all`
2. `cargo clippy --workspace -- -D warnings`
3. `cargo test --workspace`
4. If you touched contracts: `cargo build --target wasm32-unknown-unknown --release -p <contract>`

## Building WASM Contracts

MoltChain ships 27 on-chain contracts under `contracts/`. Each is its own crate:

```bash
# Build a single contract
cargo build --target wasm32-unknown-unknown --release -p moltcoin

# Build all contracts
for dir in contracts/*/; do
  name=$(basename "$dir")
  cargo build --target wasm32-unknown-unknown --release -p "$name" 2>/dev/null && \
    echo "✓ $name" || echo "✗ $name"
done
```

Compiled `.wasm` artifacts land in `target/wasm32-unknown-unknown/release/`.

## Code Style Guidelines

- **Edition**: Rust 2021
- **Clippy-clean**: all warnings treated as errors in CI
- **`rustfmt`-required**: no manually formatted code
- **Error handling**: use `thiserror` for library errors; avoid `unwrap()` in
  non-test code
- **Naming**: follow Rust API guidelines —
  <https://rust-lang.github.io/api-guidelines/naming.html>
- **Documentation**: public items should have `///` doc comments
- **Unsafe**: avoid unless absolutely necessary and well-justified

## Pull Request Process

1. Fork the repository and create a feature branch from `main`.
2. Keep PRs focused — one logical change per PR.
3. Ensure CI passes (tests, clippy, fmt).
4. Add or update tests for any new functionality.
5. Update documentation if the public API changes.
6. Request a review from at least one maintainer.
7. Squash-merge when approved.

## Reporting Issues

Open an issue on GitHub with:

- A clear, descriptive title
- Steps to reproduce (if it's a bug)
- Expected vs. actual behaviour
- Relevant logs or error output
- Environment details (OS, Rust version, commit hash)

Feature requests are welcome — label them `enhancement`.

## Project Structure

| Directory    | Description                          |
|-------------|--------------------------------------|
| `core/`     | Blockchain engine, processor, state  |
| `validator/`| Validator node binary                |
| `p2p/`      | Peer-to-peer networking              |
| `rpc/`      | JSON-RPC server                      |
| `cli/`      | Command-line client                  |
| `contracts/`| On-chain WASM smart contracts        |
| `sdk/`      | JavaScript/TypeScript SDK            |
| `tools/`    | Deployment & testing utilities       |
| `explorer/` | Block explorer web app               |
| `wallet/`   | Wallet application                   |

## License

MoltChain uses a dual-license model:

- **Apache 2.0** — core blockchain (`core/`, `validator/`, `p2p/`, `rpc/`)
- **MIT** — SDK, tools, explorer, wallet, and contracts

By contributing you agree that your contributions will be licensed under the
applicable license for the component you are modifying.

---

Thank you for helping build MoltChain! 🦞
