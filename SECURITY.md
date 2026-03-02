# Security Policy

## Reporting Vulnerabilities

Do **not** open a public GitHub issue for security vulnerabilities.

If you discover a security issue in arbx, please disclose it responsibly:

- **Email:** [bit2swaz@gmail.com]
- **Subject line:** `[arbx] Security Disclosure`
- Include a description of the issue, steps to reproduce, and potential impact.

You will receive a response within 48 hours. Please allow reasonable time for a
fix to be developed before any public disclosure.

---

## Known Risk Areas

### `PRIVATE_KEY` environment variable
- Never log the private key -- not even partially.
- Never commit `.env` to git -- it is in `.gitignore`.
- Rotate immediately if it is ever accidentally exposed.
- The signing wallet should hold **only** the MEV gas budget (≤ $60).
  Never fund it with more than you are willing to lose entirely.

### RPC endpoints / API keys
- `ARBITRUM_RPC_URL` and `ARBITRUM_SEPOLIA_RPC_URL` contain Alchemy/QuickNode
  API keys as URL slugs.
- Treat these as secrets -- do not share, log, or commit them.
- Use separate keys for mainnet and testnet so you can revoke one without
  disrupting the other.

### `ARBISCAN_API_KEY`
- Used only for contract verification via `forge verify-contract`.
- Revoke and re-issue via the Arbiscan dashboard if exposed.

### Smart contract -- `ArbExecutor.sol`
- This contract is **not audited**.
- It has not been reviewed by a professional smart contract auditor.
- Use at your own risk.
- The contract enforces `require(output >= input + minProfitWei, "No profit")`
  as a last-resort guard, but this does not substitute for an audit.
- Deploy only after thorough Foundry fork testing (Phase 2.3 of the ROADMAP).

### MEV competition and gas losses
- The bot submits transactions that may revert on-chain if another bot beats it.
- Every revert costs the L2 gas for the failed transaction (~$0.01–$0.10).
- High revert rates will drain the gas budget.
- Start with the conservative gas budget in `config/default.toml` and monitor
  the observability funnel before scaling up.

### Arbitrum 2D gas model
- Standard `eth_estimateGas` returns only the L2 execution component.
- L1 calldata cost is a separate, volatile charge that can spike to $1.50+.
- The bot queries `NodeInterface` at `0x00000000000000000000000000000000000000C8`
  before every submission to get the true total cost.
- If this query is ever skipped, the bot may submit unprofitable transactions.

---

## Dependency Security

- `cargo audit` runs on every CI push and weekly via `.github/workflows/security.yml`.
- `cargo deny check` enforces license compliance and blocks known-bad versions.
- `gitleaks` scans every commit for accidentally committed secrets.
- All dependencies are pinned via `Cargo.lock` (committed to the repository).
- The Rust toolchain is pinned via `rust-toolchain.toml`.

---

## Out of Scope

- MEV competition losses (another bot winning the same opportunity) are expected
  behaviour, not a security vulnerability.
- Gas estimation errors caused by extreme mainnet congestion are a known risk
  documented in `docs/SSOT.md`.
