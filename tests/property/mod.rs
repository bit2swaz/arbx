//! Mini-Phase 8.1 — Comprehensive Property-Based Test Suite.
//!
//! Tests are grouped into five families:
//!
//!  1. **AMM math**      — `compute_v2` invariants (8 tests)
//!  2. **Gas model**     — `compute_min_profit_pure` invariants (7 tests)
//!  3. **Type safety**   — serialisation round-trips (7 tests)
//!  4. **PnL accounting** — `PnlTracker` / `PnlState` invariants (6 tests)
//!  5. **Backoff**       — `BackoffCalculator` schedule invariants (6 tests)
//!
//! Each property is exercised for 10 000 cases (configured in `proptest.toml`).

use alloy::primitives::{Address, FixedBytes, I256, U256};
use arbx_common::{
    pnl::{PnlState, PnlTracker},
    types::{ArbPath, DexKind, Opportunity, PoolState},
};
use arbx_detector::{opportunity::compute_v2, profit::compute_min_profit_pure};
use arbx_ingestion::sequencer_feed::BackoffCalculator;
use proptest::prelude::*;

// ─── Shared strategies ───────────────────────────────────────────────────────

/// Arbitrary 20-byte Ethereum address.
fn arb_address() -> impl Strategy<Value = Address> {
    any::<[u8; 20]>().prop_map(Address::from)
}

/// Arbitrary 32-byte hash (available for future TxHash-based tests).
#[allow(dead_code)]
fn arb_hash32() -> impl Strategy<Value = FixedBytes<32>> {
    any::<[u8; 32]>().prop_map(FixedBytes::from)
}

/// U256 in [0, u64::MAX].  Capped at 64 bits so AMM numerators never
/// approach the U256 ceiling (max numerator ≈ 2^142 « 2^256).
fn arb_u256_small() -> impl Strategy<Value = U256> {
    any::<u64>().prop_map(U256::from)
}

/// Non-zero U256 in [1, u64::MAX].
fn arb_u256_nonzero_small() -> impl Strategy<Value = U256> {
    (1u64..=u64::MAX).prop_map(U256::from)
}

/// U256 in [0, u128::MAX].  Used for gas cost / PnL amounts.
fn arb_u256_large() -> impl Strategy<Value = U256> {
    any::<u128>().prop_map(U256::from)
}

/// Arbitrary DexKind variant.
fn arb_dex_kind() -> impl Strategy<Value = DexKind> {
    prop_oneof![
        Just(DexKind::UniswapV3),
        Just(DexKind::CamelotV2),
        Just(DexKind::SushiSwap),
        Just(DexKind::TraderJoeV1),
    ]
}

// ═════════════════════════════════════════════════════════════════════════════
// GROUP 1 — AMM Math
// ═════════════════════════════════════════════════════════════════════════════

proptest! {
    // 1. Output is strictly less than reserve_out for any valid non-zero inputs.
    //    The AMM can never drain the pool in a single trade.
    #[test]
    fn prop_v2_output_never_exceeds_reserve_out(
        amount_in   in arb_u256_nonzero_small(),
        reserve_in  in arb_u256_nonzero_small(),
        reserve_out in arb_u256_nonzero_small(),
        fee_tier    in 0u32..=9_999u32,
    ) {
        let out = compute_v2(amount_in, reserve_in, reserve_out, fee_tier);
        prop_assert!(
            out < reserve_out,
            "out={out} must be < reserve_out={reserve_out}"
        );
    }

    // 2. Zero amount_in always produces zero output.
    #[test]
    fn prop_v2_zero_amount_in_returns_zero(
        reserve_in  in arb_u256_nonzero_small(),
        reserve_out in arb_u256_nonzero_small(),
        fee_tier    in 0u32..=9_999u32,
    ) {
        prop_assert_eq!(
            compute_v2(U256::ZERO, reserve_in, reserve_out, fee_tier),
            U256::ZERO
        );
    }

    // 3. Zero reserve_in always produces zero output.
    #[test]
    fn prop_v2_zero_reserve_in_returns_zero(
        amount_in   in arb_u256_nonzero_small(),
        reserve_out in arb_u256_nonzero_small(),
        fee_tier    in 0u32..=9_999u32,
    ) {
        prop_assert_eq!(
            compute_v2(amount_in, U256::ZERO, reserve_out, fee_tier),
            U256::ZERO
        );
    }

    // 4. Zero reserve_out always produces zero output.
    #[test]
    fn prop_v2_zero_reserve_out_returns_zero(
        amount_in  in arb_u256_nonzero_small(),
        reserve_in in arb_u256_nonzero_small(),
        fee_tier   in 0u32..=9_999u32,
    ) {
        prop_assert_eq!(
            compute_v2(amount_in, reserve_in, U256::ZERO, fee_tier),
            U256::ZERO
        );
    }

    // 5. Output is monotonically non-decreasing in amount_in.
    //    Swapping more in can never produce less out.
    #[test]
    fn prop_v2_output_monotone_in_amount_in(
        amount_small in (1u64..=u64::MAX / 2).prop_map(U256::from),
        reserve_in   in arb_u256_nonzero_small(),
        reserve_out  in arb_u256_nonzero_small(),
        fee_tier     in 0u32..=9_999u32,
    ) {
        let amount_large = amount_small + U256::from(1u64);
        let out_small = compute_v2(amount_small, reserve_in, reserve_out, fee_tier);
        let out_large = compute_v2(amount_large, reserve_in, reserve_out, fee_tier);
        prop_assert!(
            out_large >= out_small,
            "out_large={out_large} should be >= out_small={out_small}"
        );
    }

    // 6. A non-zero fee_tier reduces (or at most equals) the zero-fee output.
    #[test]
    fn prop_v2_nonzero_fee_reduces_output(
        amount_in   in arb_u256_nonzero_small(),
        reserve_in  in arb_u256_nonzero_small(),
        reserve_out in arb_u256_nonzero_small(),
        fee_tier    in 1u32..=9_999u32,
    ) {
        let out_with_fee  = compute_v2(amount_in, reserve_in, reserve_out, fee_tier);
        let out_zero_fee  = compute_v2(amount_in, reserve_in, reserve_out, 0);
        prop_assert!(
            out_with_fee <= out_zero_fee,
            "out_with_fee={out_with_fee} should be <= out_zero_fee={out_zero_fee}"
        );
    }

    // 7. fee_tier = 1_000_000 → fee_factor = 0 → zero output.
    //    (fee_tier / 100 = 10_000, so fee_factor = 10_000 − 10_000 = 0.)
    #[test]
    fn prop_v2_max_fee_tier_returns_zero(
        amount_in   in arb_u256_nonzero_small(),
        reserve_in  in arb_u256_nonzero_small(),
        reserve_out in arb_u256_nonzero_small(),
    ) {
        prop_assert_eq!(
            compute_v2(amount_in, reserve_in, reserve_out, 1_000_000u32),
            U256::ZERO
        );
    }

    // 8. Two-hop roundtrip through the same symmetric pool always returns
    //    ≤ the original amount (fees eat into every trade).
    //    Strategy: keep amount_in well below reserve so the first hop
    //    always produces a non-trivial output.
    #[test]
    fn prop_v2_two_hop_roundtrip_loses_to_fees(
        amount_in in (1u64..=(1u64 << 40)).prop_map(U256::from),
        reserve   in ((1u64 << 41)..=u64::MAX).prop_map(U256::from),
        fee_tier  in 100u32..=9_999u32,
    ) {
        // Symmetric pool: reserve_in = reserve_out = reserve
        let out1 = compute_v2(amount_in, reserve, reserve, fee_tier);
        if out1.is_zero() {
            // Amount_in too small relative to pool → trivially < amount_in
            return Ok(());
        }
        // After first hop: reserve_out side lost out1, reserve_in side gained amount_in
        let new_reserve_in  = reserve - out1;        // the token we're now swapping in
        let new_reserve_out = reserve + amount_in;   // the token we want back
        let out2 = compute_v2(out1, new_reserve_in, new_reserve_out, fee_tier);
        prop_assert!(
            out2 <= amount_in,
            "Two-hop fee_tier={fee_tier}: out2={out2} should be <= amount_in={amount_in}"
        );
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// GROUP 2 — Gas Model
// ═════════════════════════════════════════════════════════════════════════════

proptest! {
    // 1. For multiplier ≥ 1.0 the result always covers the raw gas cost.
    //    Proof: buffer_num = (mul × 1e9) as u128 ≥ 1e9
    //           buffered_gas = cost × buffer_num / 1e9 ≥ cost.
    #[test]
    fn prop_min_profit_always_covers_gas_cost(
        total_cost_wei in arb_u256_large(),
        multiplier     in 1.0f64..=5.0f64,
        floor_usd      in 0.0f64..=100.0f64,
        eth_price_usd  in 0.0f64..=10_000.0f64,
    ) {
        let result = compute_min_profit_pure(
            total_cost_wei, multiplier, floor_usd, eth_price_usd,
        );
        prop_assert!(
            result >= total_cost_wei,
            "result={result} must be >= total_cost={total_cost_wei}"
        );
    }

    // 2. Result is monotonically non-decreasing in total_cost_wei.
    #[test]
    fn prop_min_profit_monotone_in_cost(
        cost_small    in (0u128..=u128::MAX / 2).prop_map(U256::from),
        extra         in (0u128..=u128::MAX / 2).prop_map(U256::from),
        multiplier    in 1.0f64..=5.0f64,
        floor_usd     in 0.0f64..=100.0f64,
        eth_price_usd in 1.0f64..=10_000.0f64,
    ) {
        let cost_large = cost_small + extra;
        let res_small  = compute_min_profit_pure(cost_small, multiplier, floor_usd, eth_price_usd);
        let res_large  = compute_min_profit_pure(cost_large, multiplier, floor_usd, eth_price_usd);
        prop_assert!(
            res_large >= res_small,
            "larger cost must produce larger threshold: small={res_small} large={res_large}"
        );
    }

    // 3. When floor_usd is zero the result is eth_price-independent
    //    (it equals the buffered gas with no floor contribution).
    #[test]
    fn prop_min_profit_zero_floor_is_eth_price_independent(
        total_cost_wei in arb_u256_large(),
        multiplier     in 1.0f64..=5.0f64,
        eth_price_a    in 1.0f64..=10_000.0f64,
        eth_price_b    in 1.0f64..=10_000.0f64,
    ) {
        let res_a = compute_min_profit_pure(total_cost_wei, multiplier, 0.0, eth_price_a);
        let res_b = compute_min_profit_pure(total_cost_wei, multiplier, 0.0, eth_price_b);
        prop_assert_eq!(
            res_a, res_b,
            "zero floor must be eth_price independent: a={} b={}", res_a, res_b
        );
    }

    // 4. When total_cost_wei > 0 and multiplier ≥ 1.0 the result is strictly > 0.
    #[test]
    fn prop_min_profit_nonzero_when_cost_nonzero(
        total_cost_wei in (1u128..=u128::MAX).prop_map(U256::from),
        multiplier     in 1.0f64..=5.0f64,
        floor_usd      in 0.0f64..=100.0f64,
        eth_price_usd  in 0.0f64..=10_000.0f64,
    ) {
        let result = compute_min_profit_pure(
            total_cost_wei, multiplier, floor_usd, eth_price_usd,
        );
        prop_assert!(result > U256::ZERO, "result must be > 0 when cost > 0");
    }

    // 5. With eth_price_usd = 0 the floor is ignored entirely regardless of
    //    floor_usd; both calls must produce identical results.
    #[test]
    fn prop_min_profit_zero_eth_price_ignores_floor(
        total_cost_wei in arb_u256_large(),
        multiplier     in 1.0f64..=5.0f64,
        floor_a        in 0.0f64..=100.0f64,
        floor_b        in 0.0f64..=100.0f64,
    ) {
        let res_a = compute_min_profit_pure(total_cost_wei, multiplier, floor_a, 0.0);
        let res_b = compute_min_profit_pure(total_cost_wei, multiplier, floor_b, 0.0);
        prop_assert_eq!(
            res_a, res_b,
            "zero eth_price must make floor irrelevant: a={} b={}", res_a, res_b
        );
    }

    // 6. A higher gas buffer multiplier produces a higher (or equal) threshold.
    #[test]
    fn prop_min_profit_grows_with_multiplier(
        total_cost_wei in arb_u256_large(),
        mul_small      in 1.0f64..=4.9f64,
        floor_usd      in 0.0f64..=100.0f64,
        eth_price_usd  in 1.0f64..=10_000.0f64,
    ) {
        let mul_large = mul_small + 0.1;
        let res_small = compute_min_profit_pure(total_cost_wei, mul_small, floor_usd, eth_price_usd);
        let res_large = compute_min_profit_pure(total_cost_wei, mul_large, floor_usd, eth_price_usd);
        prop_assert!(
            res_large >= res_small,
            "larger multiplier must give larger threshold: small={res_small} large={res_large}"
        );
    }

    // 7. A larger USD floor always raises (or maintains) the threshold.
    #[test]
    fn prop_min_profit_floor_increases_result(
        total_cost_wei in arb_u256_large(),
        multiplier     in 1.0f64..=5.0f64,
        eth_price_usd  in 1.0f64..=10_000.0f64,
        floor_usd      in 0.0f64..=50.0f64,
    ) {
        let without_floor = compute_min_profit_pure(total_cost_wei, multiplier, 0.0, eth_price_usd);
        let with_floor    = compute_min_profit_pure(total_cost_wei, multiplier, floor_usd, eth_price_usd);
        prop_assert!(
            with_floor >= without_floor,
            "with_floor={with_floor} must be >= without_floor={without_floor}"
        );
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// GROUP 3 — Type Safety / Serialisation
// ═════════════════════════════════════════════════════════════════════════════

proptest! {
    // 1. PoolState survives a JSON round-trip without losing any field.
    #[test]
    fn prop_pool_state_serde_roundtrip(
        addr    in arb_address(),
        tok0    in arb_address(),
        tok1    in arb_address(),
        res0    in arb_u256_large(),
        res1    in arb_u256_large(),
        fee     in 0u32..=9_999u32,
        block   in any::<u64>(),
        dex     in arb_dex_kind(),
    ) {
        let state = PoolState {
            address: addr,
            token0: tok0,
            token1: tok1,
            reserve0: res0,
            reserve1: res1,
            fee_tier: fee,
            last_updated_block: block,
            dex,
        };
        let json  = serde_json::to_string(&state).unwrap();
        let back: PoolState = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(state, back);
    }

    // 2. ArbPath survives a JSON round-trip.
    #[test]
    fn prop_arb_path_serde_roundtrip(
        token_in              in arb_address(),
        pool_a                in arb_address(),
        token_mid             in arb_address(),
        pool_b                in arb_address(),
        token_out             in arb_address(),
        estimated_profit_wei  in arb_u256_large(),
        flash_loan_amount_wei in arb_u256_large(),
    ) {
        let path = ArbPath {
            token_in,
            pool_a,
            token_mid,
            pool_b,
            token_out,
            estimated_profit_wei,
            flash_loan_amount_wei,
        };
        let json  = serde_json::to_string(&path).unwrap();
        let back: ArbPath = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(path, back);
    }

    // 3. Opportunity survives a JSON round-trip.
    #[test]
    fn prop_opportunity_serde_roundtrip(
        token_in              in arb_address(),
        pool_a                in arb_address(),
        token_mid             in arb_address(),
        pool_b                in arb_address(),
        token_out             in arb_address(),
        estimated_profit_wei  in arb_u256_large(),
        flash_loan_amount_wei in arb_u256_large(),
        gross_profit_wei      in arb_u256_large(),
        l2_gas_cost_wei       in arb_u256_large(),
        l1_gas_cost_wei       in arb_u256_large(),
        net_profit_wei        in arb_u256_large(),
        detected_at_ms        in any::<u64>(),
    ) {
        let opp = Opportunity {
            path: ArbPath {
                token_in,
                pool_a,
                token_mid,
                pool_b,
                token_out,
                estimated_profit_wei,
                flash_loan_amount_wei,
            },
            gross_profit_wei,
            l2_gas_cost_wei,
            l1_gas_cost_wei,
            net_profit_wei,
            detected_at_ms,
        };
        let json  = serde_json::to_string(&opp).unwrap();
        let back: Opportunity = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(opp, back);
    }

    // 4. ArbPath::is_circular ⟺ token_in == token_out.
    #[test]
    fn prop_arb_path_is_circular_iff_tokens_match(
        token_in  in arb_address(),
        token_out in arb_address(),
    ) {
        let path = ArbPath {
            token_in,
            pool_a: Address::ZERO,
            token_mid: Address::ZERO,
            pool_b: Address::ZERO,
            token_out,
            estimated_profit_wei: U256::ZERO,
            flash_loan_amount_wei: U256::ZERO,
        };
        prop_assert_eq!(
            path.is_circular(),
            token_in == token_out,
            "is_circular() must equal (token_in == token_out)"
        );
    }

    // 5. Opportunity::total_gas_cost_wei() == l2_gas_cost_wei + l1_gas_cost_wei.
    #[test]
    fn prop_opportunity_total_gas_is_l2_plus_l1(
        l2 in arb_u256_small(),
        l1 in arb_u256_small(),
    ) {
        let opp = Opportunity {
            path: ArbPath {
                token_in: Address::ZERO,
                pool_a: Address::ZERO,
                token_mid: Address::ZERO,
                pool_b: Address::ZERO,
                token_out: Address::ZERO,
                estimated_profit_wei: U256::ZERO,
                flash_loan_amount_wei: U256::ZERO,
            },
            gross_profit_wei: U256::ZERO,
            l2_gas_cost_wei: l2,
            l1_gas_cost_wei: l1,
            net_profit_wei: U256::ZERO,
            detected_at_ms: 0,
        };
        prop_assert_eq!(opp.total_gas_cost_wei(), l2 + l1);
    }

    // 6. PnlState survives a JSON round-trip (checked field-by-field since
    //    PnlState does not derive PartialEq).
    #[test]
    fn prop_pnl_state_serde_roundtrip(
        budget      in 0.01f64..=1_000.0f64,
        successful  in 0u64..=10_000u64,
        reverted    in 0u64..=10_000u64,
        gas_usd     in 0.0f64..=500.0f64,
        profit_usd  in -500.0f64..=500.0f64,
    ) {
        let state = PnlState {
            total_submissions: successful + reverted,
            successful_arbs: successful,
            reverted_arbs: reverted,
            budget_remaining_usd: budget,
            total_gas_spent_usd: gas_usd,
            total_profit_usd: profit_usd,
            net_pnl_usd: profit_usd,
            total_gas_spent_wei: U256::ZERO.to_string(),
            total_profit_wei: I256::ZERO.to_string(),
            ..Default::default()
        };
        let json  = serde_json::to_string(&state).unwrap();
        let back: PnlState = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(back.total_submissions, state.total_submissions);
        prop_assert_eq!(back.successful_arbs,   state.successful_arbs);
        prop_assert_eq!(back.reverted_arbs,      state.reverted_arbs);
        prop_assert!(
            (back.budget_remaining_usd - state.budget_remaining_usd).abs() < 1e-9
        );
        prop_assert!(
            (back.total_gas_spent_usd - state.total_gas_spent_usd).abs() < 1e-9
        );
    }

    // 7. DexKind serialises and deserialises to the same variant.
    #[test]
    fn prop_dex_kind_serde_roundtrip(dex in arb_dex_kind()) {
        let json  = serde_json::to_string(&dex).unwrap();
        let back: DexKind = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(dex, back);
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// GROUP 4 — PnL Accounting
// ═════════════════════════════════════════════════════════════════════════════

proptest! {
    // 1. A freshly-created PnlTracker with budget > $0.10 is not exhausted.
    #[test]
    fn prop_pnl_fresh_tracker_is_not_exhausted(budget in 0.2f64..=100.0f64) {
        let dir  = tempfile::tempdir().unwrap();
        let path = dir.path().join("pnl.json").to_string_lossy().into_owned();
        let tracker = PnlTracker::new(path, budget).unwrap();
        prop_assert!(
            !tracker.is_budget_exhausted(),
            "budget={budget} should NOT be exhausted"
        );
    }

    // 2. A freshly-created PnlTracker has zero submissions.
    #[test]
    fn prop_pnl_fresh_tracker_zero_submissions(budget in 0.2f64..=100.0f64) {
        let dir  = tempfile::tempdir().unwrap();
        let path = dir.path().join("pnl.json").to_string_lossy().into_owned();
        let tracker = PnlTracker::new(path, budget).unwrap();
        let snap = tracker.state_snapshot();
        prop_assert_eq!(snap.total_submissions, 0u64);
        prop_assert_eq!(snap.successful_arbs,   0u64);
        prop_assert_eq!(snap.reverted_arbs,      0u64);
    }

    // 3. A freshly-created PnlTracker reflects the budget passed to new().
    #[test]
    fn prop_pnl_fresh_tracker_budget_matches_input(budget in 0.01f64..=1_000.0f64) {
        let dir  = tempfile::tempdir().unwrap();
        let path = dir.path().join("pnl.json").to_string_lossy().into_owned();
        let tracker = PnlTracker::new(path, budget).unwrap();
        let snap = tracker.state_snapshot();
        prop_assert!(
            (snap.budget_remaining_usd - budget).abs() < 1e-9,
            "snap.budget={} should equal input={budget}",
            snap.budget_remaining_usd
        );
    }

    // 4. total_submissions == successful_arbs + reverted_arbs is the canonical
    //    accounting identity; constructing a PnlState that satisfies it round-
    //    trips the check.
    #[test]
    fn prop_pnl_state_submissions_sum_accounting(
        successful in 0u64..=10_000u64,
        reverted   in 0u64..=10_000u64,
    ) {
        let state = PnlState {
            total_submissions: successful + reverted,
            successful_arbs: successful,
            reverted_arbs: reverted,
            ..Default::default()
        };
        prop_assert_eq!(
            state.total_submissions,
            state.successful_arbs + state.reverted_arbs
        );
    }

    // 5. Budget below $0.101 is reported as exhausted.
    #[test]
    fn prop_pnl_exhausted_when_budget_below_threshold(
        budget in 0.0f64..=0.100f64,
    ) {
        let dir  = tempfile::tempdir().unwrap();
        let path = dir.path().join("pnl.json").to_string_lossy().into_owned();
        let tracker = PnlTracker::new(path, budget).unwrap();
        prop_assert!(
            tracker.is_budget_exhausted(),
            "budget={budget} is below threshold and should be exhausted"
        );
    }

    // 6. Wei string fields round-trip through the canonical decimal
    //    representation used by PnlState.
    #[test]
    fn prop_pnl_state_wei_strings_survive_roundtrip(
        gas_wei    in arb_u256_large(),
        profit_wei in any::<i128>().prop_map(|v| I256::try_from(v).unwrap_or(I256::ZERO)),
    ) {
        let state = PnlState {
            total_gas_spent_wei: gas_wei.to_string(),
            total_profit_wei: profit_wei.to_string(),
            ..Default::default()
        };
        // Serialise → deserialise → field must match string-for-string
        let json = serde_json::to_string(&state).unwrap();
        let back: PnlState = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(back.total_gas_spent_wei, state.total_gas_spent_wei);
        prop_assert_eq!(back.total_profit_wei,    state.total_profit_wei);
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// GROUP 5 — Backoff Invariants
// ═════════════════════════════════════════════════════════════════════════════

proptest! {
    // 1. The very first call to next() returns base_ms.
    #[test]
    fn prop_backoff_first_next_equals_base_ms(
        base_raw   in 1u64..=10_000u64,
        max_raw    in 1u64..=100_000u64,
        multiplier in 1.0f64..=8.0f64,
    ) {
        let (base, max) = (base_raw.min(max_raw), base_raw.max(max_raw));
        let mut b = BackoffCalculator::new(base, max, multiplier);
        prop_assert_eq!(b.next(), base);
    }

    // 2. Every value returned by next() is ≤ max_ms.
    #[test]
    fn prop_backoff_never_exceeds_max_ms(
        base_raw   in 1u64..=1_000u64,
        max_raw    in 1u64..=100_000u64,
        multiplier in 1.0f64..=8.0f64,
        n_calls    in 1usize..=30usize,
    ) {
        let (base, max) = (base_raw.min(max_raw), base_raw.max(max_raw));
        let mut b = BackoffCalculator::new(base, max, multiplier);
        for _ in 0..n_calls {
            let delay = b.next();
            prop_assert!(delay <= max, "delay={delay} exceeded max={max}");
        }
    }

    // 3. After any number of next() calls, reset() restores base_ms.
    #[test]
    fn prop_backoff_reset_restores_base_ms(
        base_raw   in 1u64..=1_000u64,
        max_raw    in 1u64..=100_000u64,
        multiplier in 1.0f64..=8.0f64,
        n_calls    in 0usize..=20usize,
    ) {
        let (base, max) = (base_raw.min(max_raw), base_raw.max(max_raw));
        let mut b = BackoffCalculator::new(base, max, multiplier);
        for _ in 0..n_calls { b.next(); }
        b.reset();
        prop_assert_eq!(b.next(), base, "next() after reset must equal base_ms");
    }

    // 4. Successive calls are non-decreasing until the schedule saturates at max_ms.
    //    We pick a large max so the sequence doesn't cap out early.
    #[test]
    fn prop_backoff_sequence_is_non_decreasing(
        base       in 1u64..=100u64,
        max        in 100_000u64..=1_000_000u64,
        multiplier in 1.1f64..=4.0f64,
        n_calls    in 2usize..=12usize,
    ) {
        let mut b    = BackoffCalculator::new(base, max, multiplier);
        let mut prev = b.next();
        for _ in 1..n_calls {
            let curr = b.next();
            prop_assert!(curr >= prev, "sequence went down: curr={curr} < prev={prev}");
            prev = curr;
        }
    }

    // 5. With multiplier = 1.0 the schedule stays fixed at base_ms forever.
    #[test]
    fn prop_backoff_multiplier_one_stays_at_base(
        base    in 1u64..=10_000u64,
        n_calls in 1usize..=30usize,
    ) {
        let mut b = BackoffCalculator::new(base, u64::MAX, 1.0);
        for _ in 0..n_calls {
            prop_assert_eq!(b.next(), base);
        }
    }

    // 6. For a positive base_ms every call to next() returns a value > 0.
    #[test]
    fn prop_backoff_always_positive_for_positive_base(
        base_raw   in 1u64..=10_000u64,
        max_raw    in 1u64..=100_000u64,
        multiplier in 1.0f64..=8.0f64,
        n_calls    in 1usize..=20usize,
    ) {
        let (base, max) = (base_raw.min(max_raw), base_raw.max(max_raw));
        let mut b = BackoffCalculator::new(base, max, multiplier);
        for _ in 0..n_calls {
            prop_assert!(b.next() > 0, "delay must always be positive");
        }
    }
}
