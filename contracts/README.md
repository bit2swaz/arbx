# ArbExecutor Contract

Balancer V2 flash loan + two-hop atomic arbitrage executor for Arbitrum.

## Install deps

```bash
forge install foundry-rs/forge-std --no-git
forge install OpenZeppelin/openzeppelin-contracts --no-git
forge install balancer-labs/balancer-v2-monorepo --no-git
```

> `lib/` is git-ignored. Run `forge install` after every fresh clone.

## Build

```bash
forge build
```

## Test

```bash
# Unit tests (no RPC required)
forge test -vvv

# Fork tests against Arbitrum mainnet
ARBITRUM_RPC_URL=<your_url> forge test -vvv --fork-url arbitrum
```

## Deploy

See [script/Deploy.s.sol](script/Deploy.s.sol) — written in Mini-Phase 2.5.
