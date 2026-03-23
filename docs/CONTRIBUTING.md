# Contributing to arbx

Thank you for taking an interest in `arbx`.

This project aims to be both a working arbitrage engine and a well-documented reference implementation for Arbitrum MEV infrastructure. Contributions that improve correctness, clarity, test quality, or operational safety are all valuable.

## Getting Started

### Prerequisites

You should have the following installed:

- Rust toolchain, via `rustup`
- Foundry, for Solidity builds and tests
- Git
- An Arbitrum RPC URL for fork tests and local validation

### Setup

```bash
git clone https://github.com/yourname/arbx.git
cd arbx
```

Create a local environment file:

```bash
cp .env.example .env
```

Build the workspace:

```bash
cargo build --workspace
forge build
```

## Development Workflow

The repository follows a mini-phase structure described in `docs/ROADMAP.md`, with architecture and ground-truth details in `docs/SSOT.md`.

The intended workflow is:

1. Read `docs/SSOT.md` before making meaningful architectural changes.
2. Work in small, reviewable changes.
3. Add tests before or alongside implementation.
4. Keep the system green at every step.

### TDD contract

`arbx` follows a strict testing culture.

- Every new feature should include tests.
- Bug fixes should usually come with a regression test.
- No mini-phase is considered done until the relevant test suite passes.
- Broken code should not be carried forward to the next phase.

If your change affects runtime behavior, detection logic, simulation logic, submission flow, or budget handling, include coverage that proves the new behavior.

## Running the Test Suite

### Full Rust workspace

```bash
cargo test --workspace
```

Runs the complete Rust test suite across all crates and top-level test targets.

### Common crate only

```bash
cargo test -p arbx-common
```

Useful when working on shared types, config, metrics, or PnL tracking.

### Integration tests

```bash
cargo test --test integration
```

Runs the end-to-end Rust integration tests in `tests/integration/`.

### Solidity tests

```bash
forge test
```

Runs the Foundry test suite for `ArbExecutor.sol`.

### Fork tests

```bash
forge test --fork-url "$ARBITRUM_RPC_URL"
```

Runs Solidity fork tests against live Arbitrum state. This requires a working RPC URL and enough rate limit headroom.

### Benchmarks

```bash
cargo bench
```

Runs Criterion benchmarks for the current hot paths.

## Code Style

### Rust style checks

CI enforces a strict Rust style baseline.

- `cargo clippy --workspace -- -D warnings`
- `cargo fmt --check`
- `cargo test --workspace`

Warnings are treated as errors in CI, so code should be cleaned up before submission.

### Formatting

Use `cargo fmt` before opening a pull request.

### Linting

Use `cargo clippy --workspace -- -D warnings` locally before you push.

### Dependency and license policy

The repository includes `deny.toml` for dependency and license checks. If you add a new dependency, make sure it does not introduce a license conflict or an avoidable duplicate dependency tree.

## Submitting a PR

Use this checklist before opening or updating a pull request.

- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace -- -D warnings` passes
- [ ] `cargo fmt --check` passes
- [ ] `forge test` passes
- [ ] new functionality has tests
- [ ] `docs/SSOT.md` is updated if architecture changed
- [ ] config changes are reflected in documentation when needed
- [ ] no secrets or private keys were added to the repository

### PR notes

A good pull request should explain:

- what changed
- why it changed
- how it was tested
- whether any follow-up work remains

Small, focused pull requests are much easier to review than broad mixed changes.

## Good First Issues

If you want to contribute but are not sure where to start, these are high-value areas:

1. **Additional DEX integrations**
   - Camelot V3
   - Trader Joe V2

2. **Three-hop path detection using petgraph**
   - extend pathfinding beyond two-hop cycles
   - keep test coverage high while increasing graph complexity

3. **Timeboost express lane research**
   - measure likely cost versus expected edge
   - document when participation becomes economically sensible

## Documentation expectations

This project treats docs as part of the product.

If you change architecture, safety assumptions, runtime behavior, or operator workflow, update the matching documentation in the same PR. At minimum, that often means one or more of:

- `docs/SSOT.md`
- `docs/ROADMAP.md`
- `docs/ARCHITECTURE.md`
- `README.md`

## Questions and discussion

If you are unsure how a change fits the current direction, read `docs/SSOT.md` first. If the answer is still unclear, open a discussion in the pull request and explain the tradeoff you are considering.

Correctness, clarity, and safety are preferred over clever shortcuts.