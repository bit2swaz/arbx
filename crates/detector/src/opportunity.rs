//! Two-hop arbitrage path scanner with AMM output formulas.
//!
//! # Architecture
//! The [`PathScanner`] walks the in-memory [`PoolStateStore`] looking for
//! circular two-hop paths of the form:
//!
//! ```text
//! token_in → pool_a (affected) → token_mid → pool_b → token_in
//! ```
//!
//! All arithmetic is in `U256` to avoid lossy conversions.  The public
//! free-function [`compute_v2`] is exported so that property-based tests
//! can call it without constructing a full [`PathScanner`].

use alloy::primitives::{Address, U256};
use arbx_common::types::{ArbPath, DexKind, PoolState};
use arbx_ingestion::pool_state::PoolStateStore;
use tracing::debug;

// ─── Public free function (used by property tests) ───────────────────────────

/// Constant-product (UniswapV2-style) output formula.
///
/// ```text
/// amount_out = (amount_in × (10_000 − fee_tier/100) × reserve_out)
///            / (reserve_in × 10_000 + amount_in × (10_000 − fee_tier/100))
/// ```
///
/// `fee_tier` follows the Uniswap convention where `3000` means 0.3 % (i.e.
/// the effective factor is `10_000 − fee_tier / 100 = 9_970`).
///
/// Returns `U256::ZERO` when any argument is zero.
pub fn compute_v2(amount_in: U256, reserve_in: U256, reserve_out: U256, fee_tier: u32) -> U256 {
    if amount_in.is_zero() || reserve_in.is_zero() || reserve_out.is_zero() {
        return U256::ZERO;
    }
    // fee_tier = 3000 → 0.3 % → fee_factor = 10_000 − 30 = 9_970
    let fee_factor = U256::from(10_000u32 - fee_tier / 100);
    let numerator = amount_in * fee_factor * reserve_out;
    let denominator = reserve_in * U256::from(10_000u32) + amount_in * fee_factor;
    if denominator.is_zero() {
        return U256::ZERO;
    }
    numerator / denominator
}

// ─── PartialPath ─────────────────────────────────────────────────────────────

/// Intermediate path description used while searching for the optimal flash
/// loan amount before an [`ArbPath`] is finalised.
pub struct PartialPath {
    pub token_in: Address,
    pub pool_a: PoolState,
    pub token_mid: Address,
    pub pool_b: PoolState,
}

// ─── PathScanner ─────────────────────────────────────────────────────────────

/// Scans the in-memory pool store for two-hop arbitrage cycles triggered by
/// a reserve update on a specific pool.
pub struct PathScanner {
    pool_store: PoolStateStore,
}

impl PathScanner {
    /// Creates a new [`PathScanner`] backed by the given pool store.
    pub fn new(pool_store: PoolStateStore) -> Self {
        Self { pool_store }
    }

    /// Finds all two-hop cycles passing through `affected_pool`.
    ///
    /// Each returned [`ArbPath`] has `token_out == token_in` (i.e.
    /// [`ArbPath::is_circular`] is always `true`).  Paths where no profit is
    /// currently available are still returned — the caller filters on
    /// profitability.
    pub fn scan(&self, affected_pool: Address) -> Vec<ArbPath> {
        let Some(pool_a) = self.pool_store.get(&affected_pool) else {
            return Vec::new();
        };

        let mut paths = Vec::new();

        // Try both token orientations of pool_a.
        for &(token_in, token_mid) in &[
            (pool_a.token0, pool_a.token1),
            (pool_a.token1, pool_a.token0),
        ] {
            // Find every pool that contains token_mid.
            let candidates = self.pool_store.pools_containing_token(&token_mid);

            for pool_b in candidates {
                // Never route through the same pool twice.
                if pool_b.address == affected_pool {
                    continue;
                }
                // pool_b must also contain token_in to close the cycle.
                if pool_b.token0 != token_in && pool_b.token1 != token_in {
                    continue;
                }

                let partial = PartialPath {
                    token_in,
                    pool_a: pool_a.clone(),
                    token_mid,
                    pool_b: pool_b.clone(),
                };
                let flash_amount = self.optimal_flash_loan_amount(&partial);
                let mid_out = self.pool_output(token_in, flash_amount, &pool_a);
                let final_out = self.pool_output(token_mid, mid_out, &pool_b);

                let estimated_profit = final_out.saturating_sub(flash_amount);

                debug!(
                    pool_a = %pool_a.address,
                    pool_b = %pool_b.address,
                    token_in = %token_in,
                    profit = %estimated_profit,
                    "two-hop path candidate",
                );

                paths.push(ArbPath {
                    token_in,
                    pool_a: pool_a.address,
                    token_mid,
                    pool_b: pool_b.address,
                    token_out: token_in, // circular — closes back to token_in
                    estimated_profit_wei: estimated_profit,
                    flash_loan_amount_wei: flash_amount,
                });
            }
        }

        paths
    }

    // ── AMM output helpers ────────────────────────────────────────────────────

    /// Dispatches to the correct AMM formula based on the pool's [`DexKind`].
    fn pool_output(&self, token_in: Address, amount_in: U256, pool: &PoolState) -> U256 {
        match pool.dex {
            DexKind::UniswapV3 => self
                .compute_output_v3(pool, token_in, amount_in)
                .unwrap_or(U256::ZERO),
            DexKind::CamelotV2 | DexKind::SushiSwap | DexKind::TraderJoeV1 => self
                .compute_output_v2(pool, token_in, amount_in)
                .unwrap_or(U256::ZERO),
        }
    }

    /// UniswapV2-style constant-product output for a specific pool.
    ///
    /// Returns `None` when:
    /// - `amount_in` is zero,
    /// - `token_in` is not a token in the pool,
    /// - either reserve is zero, or
    /// - `amount_in` would drain the pool (`amount_in > reserve_in`).
    pub fn compute_output_v2(
        &self,
        pool: &PoolState,
        token_in: Address,
        amount_in: U256,
    ) -> Option<U256> {
        if amount_in.is_zero() {
            return None;
        }
        let (reserve_in, reserve_out) = if token_in == pool.token0 {
            (pool.reserve0, pool.reserve1)
        } else if token_in == pool.token1 {
            (pool.reserve1, pool.reserve0)
        } else {
            return None;
        };
        if reserve_in.is_zero() || reserve_out.is_zero() {
            return None;
        }
        if amount_in > reserve_in {
            return None;
        }
        Some(compute_v2(
            amount_in,
            reserve_in,
            reserve_out,
            pool.fee_tier,
        ))
    }

    /// UniswapV3 simplified sqrtPrice-based output approximation.
    ///
    /// Uses `sqrtPriceX96` (stored in `pool.reserve0`) to derive the effective
    /// spot price, then applies the fee.
    ///
    /// **TODO: replace with full tick-math in Phase 10.**
    ///
    /// Returns `None` on zero input, unknown token, zero sqrtPrice, or
    /// arithmetic overflow.
    pub fn compute_output_v3(
        &self,
        pool: &PoolState,
        token_in: Address,
        amount_in: U256,
    ) -> Option<U256> {
        if amount_in.is_zero() {
            return None;
        }
        // pool.reserve0 holds sqrtPriceX96 (price of token1 expressed in token0, Q96)
        let sqrt_price_x96 = pool.reserve0;
        if sqrt_price_x96.is_zero() {
            return None;
        }

        // fee_factor / 10_000 is the fraction of input that is not taken as fee.
        let fee_factor = U256::from(10_000u32 - pool.fee_tier / 100);
        // Q96 = 2^96
        let q96 = U256::from(1u128) << 96;

        if token_in == pool.token0 {
            // price = (sqrtPriceX96 / 2^96)^2  (token1 per token0)
            // output ≈ amount_in × price × fee_factor / 10_000
            //        = amount_in × sqrtPriceX96 / 2^96 × sqrtPriceX96 / 2^96 × fee_factor / 10_000
            let step1 = amount_in.checked_mul(sqrt_price_x96)?.checked_div(q96)?;
            let step2 = step1.checked_mul(sqrt_price_x96)?.checked_div(q96)?;
            let out = step2.checked_mul(fee_factor)? / U256::from(10_000u32);
            if out.is_zero() {
                None
            } else {
                Some(out)
            }
        } else if token_in == pool.token1 {
            // Inverse price: 1 / (sqrtPriceX96 / 2^96)^2  (token0 per token1)
            // output ≈ amount_in / price × fee_factor / 10_000
            //        = amount_in × (2^96 / sqrtPriceX96)^2 × fee_factor / 10_000
            let step1 = amount_in.checked_mul(q96)?.checked_div(sqrt_price_x96)?;
            let step2 = step1.checked_mul(q96)?.checked_div(sqrt_price_x96)?;
            let out = step2.checked_mul(fee_factor)? / U256::from(10_000u32);
            if out.is_zero() {
                None
            } else {
                Some(out)
            }
        } else {
            None
        }
    }

    // ── Optimal flash-loan amount search ────────────────────────────────────

    /// Returns the flash loan amount that maximises net profit on `path`.
    ///
    /// The profit function for constant-product pools is unimodal, so ternary
    /// search over `[1e6, 1e24]` converges to the global maximum.
    pub fn optimal_flash_loan_amount(&self, path: &PartialPath) -> U256 {
        let mut low = U256::from(1_000_000u128); // 1e6
        let mut high = U256::from(10u128.pow(24)); // 1e24

        for _ in 0..64 {
            if low >= high {
                break;
            }
            let third = (high - low) / U256::from(3u32);
            if third.is_zero() {
                break;
            }
            let m1 = low + third;
            let m2 = high - third;
            if self.path_profit(path, m1) < self.path_profit(path, m2) {
                low = m1;
            } else {
                high = m2;
            }
        }

        (low + high) / U256::from(2u32)
    }

    /// Net profit for a trial flash loan `amount` through `path`.
    fn path_profit(&self, path: &PartialPath, amount: U256) -> U256 {
        let mid_out = self.pool_output(path.token_in, amount, &path.pool_a);
        let final_out = self.pool_output(path.token_mid, mid_out, &path.pool_b);
        final_out.saturating_sub(amount)
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::Address;
    use arbx_common::types::{DexKind, PoolState};

    // ── Test helpers ─────────────────────────────────────────────────────────

    fn addr(seed: u8) -> Address {
        Address::from([seed; 20])
    }

    fn make_pool(
        pool_addr: Address,
        token0: Address,
        token1: Address,
        r0: u128,
        r1: u128,
        fee: u32,
        dex: DexKind,
    ) -> PoolState {
        PoolState {
            address: pool_addr,
            token0,
            token1,
            reserve0: U256::from(r0),
            reserve1: U256::from(r1),
            fee_tier: fee,
            last_updated_block: 1,
            dex,
        }
    }

    fn make_store(pools: Vec<PoolState>) -> PoolStateStore {
        let store = PoolStateStore::new();
        for p in pools {
            store.upsert(p);
        }
        store
    }

    // ── Scan tests ───────────────────────────────────────────────────────────

    #[test]
    fn test_scan_finds_two_hop_cycle() {
        // Two pools sharing both USDC and ETH — forms a two-hop cycle.
        let usdc = addr(1);
        let eth = addr(2);
        let pool_a_addr = addr(10);
        let pool_b_addr = addr(11);

        let store = make_store(vec![
            make_pool(
                pool_a_addr,
                usdc,
                eth,
                1_000_000,
                500,
                3000,
                DexKind::CamelotV2,
            ),
            make_pool(
                pool_b_addr,
                eth,
                usdc,
                500,
                1_000_000,
                3000,
                DexKind::SushiSwap,
            ),
        ]);

        let scanner = PathScanner::new(store);
        let paths = scanner.scan(pool_a_addr);

        assert!(!paths.is_empty(), "expected at least one two-hop cycle");
        // Every path must be circular.
        for p in &paths {
            assert!(p.is_circular(), "path must close back to token_in");
        }
    }

    #[test]
    fn test_scan_no_path_no_shared_token() {
        // Two pools with completely disjoint token sets — no cycle possible.
        let usdc = addr(1);
        let eth = addr(2);
        let arb = addr(3);
        let wbtc = addr(4);
        let pool_a_addr = addr(10);
        let pool_b_addr = addr(11);

        let store = make_store(vec![
            make_pool(
                pool_a_addr,
                usdc,
                eth,
                1_000_000,
                500,
                3000,
                DexKind::CamelotV2,
            ),
            make_pool(
                pool_b_addr,
                arb,
                wbtc,
                500_000,
                10,
                3000,
                DexKind::SushiSwap,
            ),
        ]);

        let scanner = PathScanner::new(store);
        let paths = scanner.scan(pool_a_addr);

        assert!(
            paths.is_empty(),
            "disjoint token sets should yield no paths"
        );
    }

    #[test]
    fn test_scan_finds_multiple_paths() {
        // Pool A (USDC/ETH) is affected.
        // Pool B and Pool C both trade ETH/USDC → two pool_b candidates per
        // orientation → four total paths.
        let usdc = addr(1);
        let eth = addr(2);
        let pool_a_addr = addr(10);
        let pool_b_addr = addr(11);
        let pool_c_addr = addr(12);

        let store = make_store(vec![
            make_pool(
                pool_a_addr,
                usdc,
                eth,
                1_000_000,
                500,
                3000,
                DexKind::CamelotV2,
            ),
            make_pool(
                pool_b_addr,
                eth,
                usdc,
                490,
                990_000,
                3000,
                DexKind::SushiSwap,
            ),
            make_pool(
                pool_c_addr,
                eth,
                usdc,
                510,
                1_010_000,
                3000,
                DexKind::UniswapV3,
            ),
        ]);

        let scanner = PathScanner::new(store);
        let paths = scanner.scan(pool_a_addr);

        // 2 orientations × 2 pool_b candidates = 4 paths.
        assert_eq!(paths.len(), 4, "expected exactly 4 two-hop paths");
    }

    #[test]
    fn test_scan_only_considers_known_pools() {
        // The affected pool is NOT in the store — nothing to scan.
        let usdc = addr(1);
        let eth = addr(2);
        let pool_a_addr = addr(10); // not inserted
        let pool_b_addr = addr(11);

        let store = make_store(vec![make_pool(
            pool_b_addr,
            usdc,
            eth,
            500_000,
            250,
            3000,
            DexKind::SushiSwap,
        )]);

        let scanner = PathScanner::new(store);
        let paths = scanner.scan(pool_a_addr);

        assert!(
            paths.is_empty(),
            "affected pool not in store → should return no paths"
        );
    }

    #[test]
    fn test_arb_path_is_circular() {
        // Every path returned by scan() must satisfy is_circular().
        let usdc = addr(1);
        let eth = addr(2);
        let pool_a_addr = addr(10);
        let pool_b_addr = addr(11);

        let store = make_store(vec![
            make_pool(
                pool_a_addr,
                usdc,
                eth,
                1_000_000,
                500,
                3000,
                DexKind::CamelotV2,
            ),
            make_pool(
                pool_b_addr,
                eth,
                usdc,
                500,
                1_000_000,
                3000,
                DexKind::SushiSwap,
            ),
        ]);

        let scanner = PathScanner::new(store);
        let paths = scanner.scan(pool_a_addr);

        assert!(!paths.is_empty());
        for path in paths {
            assert!(
                path.is_circular(),
                "all scan results must satisfy is_circular()"
            );
        }
    }

    // ── compute_output_v2 tests ───────────────────────────────────────────────

    #[test]
    fn test_compute_output_v2_known_values() {
        // reserve0 = 1_000_000 USDC, reserve1 = 400 WETH, fee = 3000 (0.3 %)
        // amount_in = 10_000 USDC
        // expected = floor((10000 × 9970 × 400) / (1000000 × 10000 + 10000 × 9970))
        //          = floor(39_880_000_000 / 10_099_700_000)  =  3
        let usdc = addr(1);
        let weth = addr(2);
        let pool = make_pool(
            addr(10),
            usdc,
            weth,
            1_000_000,
            400,
            3000,
            DexKind::CamelotV2,
        );

        let scanner = PathScanner::new(PoolStateStore::new());
        let output = scanner
            .compute_output_v2(&pool, usdc, U256::from(10_000u64))
            .expect("should return Some for valid input");

        let expected = U256::from(3u64);
        // Assert within 1 wei of expected.
        let diff = if output >= expected {
            output - expected
        } else {
            expected - output
        };
        assert!(diff <= U256::from(1u64), "expected ≈3, got {output}");
    }

    #[test]
    fn test_compute_output_v2_zero_input() {
        let usdc = addr(1);
        let weth = addr(2);
        let pool = make_pool(
            addr(10),
            usdc,
            weth,
            1_000_000,
            400,
            3000,
            DexKind::SushiSwap,
        );
        let scanner = PathScanner::new(PoolStateStore::new());

        let result = scanner.compute_output_v2(&pool, usdc, U256::ZERO);
        assert!(result.is_none(), "zero input should return None");
    }

    #[test]
    fn test_compute_output_v2_exceeds_reserves() {
        // amount_in > reserve_in → should return None.
        let usdc = addr(1);
        let weth = addr(2);
        let pool = make_pool(addr(10), usdc, weth, 1_000, 400, 3000, DexKind::CamelotV2);
        let scanner = PathScanner::new(PoolStateStore::new());

        let result = scanner.compute_output_v2(&pool, usdc, U256::from(1_001u64));
        assert!(
            result.is_none(),
            "amount_in > reserve_in should return None"
        );
    }

    // ── compute_output_v3 tests ───────────────────────────────────────────────

    #[test]
    fn test_compute_output_v3_directional_correct() {
        // Higher sqrtPriceX96 → higher effective price (more token1 per token0)
        // → larger output when swapping token0 → token1.
        let token0 = addr(1);
        let token1 = addr(2);
        let q96 = U256::from(1u128) << 96;

        // pool_low: sqrtPriceX96 = Q96 (price = 1)
        let pool_low = PoolState {
            address: addr(10),
            token0,
            token1,
            reserve0: q96,              // sqrtPriceX96
            reserve1: U256::from(1u64), // liquidity (not used by formula)
            fee_tier: 3000,
            last_updated_block: 1,
            dex: DexKind::UniswapV3,
        };
        // pool_high: sqrtPriceX96 = 2×Q96 (price = 4)
        let pool_high = PoolState {
            address: addr(11),
            reserve0: q96 * U256::from(2u32),
            ..pool_low.clone()
        };

        let scanner = PathScanner::new(PoolStateStore::new());
        let amount_in = U256::from(1_000u64);

        let out_low = scanner
            .compute_output_v3(&pool_low, token0, amount_in)
            .expect("low-price pool should return Some");
        let out_high = scanner
            .compute_output_v3(&pool_high, token0, amount_in)
            .expect("high-price pool should return Some");

        assert!(
            out_high > out_low,
            "higher price should produce more output: out_high={out_high}, out_low={out_low}"
        );
    }

    // ─── Property tests ───────────────────────────────────────────────────────

    #[cfg(test)]
    mod property_tests {
        use super::*;
        use proptest::prelude::*;

        proptest! {
            // Conservation: when reserve0 == reserve1, output < input (fee
            // removes value regardless of amount size).
            #[test]
            fn prop_v2_output_less_than_input_equal_reserves(
                amount_in in 1u64..1_000_000_000u64,
                reserve in 1_000_000u64..1_000_000_000_000u64,
            ) {
                let reserve = U256::from(reserve);
                let amount  = U256::from(amount_in);
                let output  = compute_v2(amount, reserve, reserve, 3000);
                prop_assert!(output < amount, "output={output}, amount={amount}");
            }

            // Monotonicity: larger input ⟹ larger output.
            #[test]
            fn prop_v2_output_monotone_in_amount(
                amount_a in 1u64..1_000_000u64,
                amount_b in 1u64..1_000_000u64,
                reserve_in  in 10_000_000u64..1_000_000_000u64,
                reserve_out in 10_000_000u64..1_000_000_000u64,
            ) {
                let (a, b) = if amount_a <= amount_b {
                    (amount_a, amount_b)
                } else {
                    (amount_b, amount_a)
                };
                let out_a = compute_v2(
                    U256::from(a), U256::from(reserve_in), U256::from(reserve_out), 3000,
                );
                let out_b = compute_v2(
                    U256::from(b), U256::from(reserve_in), U256::from(reserve_out), 3000,
                );
                prop_assert!(out_a <= out_b, "out_a={out_a}, out_b={out_b}");
            }

            // No free money: two hops through identical reserves always loses
            // value to fees.
            #[test]
            fn prop_no_arb_through_identical_pools(
                amount in 1u64..1_000_000u64,
                reserve in 10_000_000u64..1_000_000_000u64,
            ) {
                let r   = U256::from(reserve);
                let mid = compute_v2(U256::from(amount), r, r, 3000);
                let out = compute_v2(mid, r, r, 3000);
                prop_assert!(
                    out < U256::from(amount),
                    "two-hop through equal reserves must lose to fees: out={out}, amount={amount}"
                );
            }
        }
    }
}
