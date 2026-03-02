//! Shared types for the arbx arbitrage engine.
//!
//! All types that cross crate boundaries live here so every layer speaks
//! the same language.

use alloy::primitives::{Address, TxHash, I256, U256};
use serde::{Deserialize, Serialize};

// ─── DexKind ─────────────────────────────────────────────────────────────────

/// The DEX that hosts a particular liquidity pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DexKind {
    UniswapV3,
    CamelotV2,
    SushiSwap,
    TraderJoeV1,
}

// ─── PoolState ───────────────────────────────────────────────────────────────

/// Snapshot of a single DEX liquidity pool, held in the in-memory DashMap.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PoolState {
    pub address: Address,
    pub token0: Address,
    pub token1: Address,
    pub reserve0: U256,
    pub reserve1: U256,
    /// Fee in basis points, e.g. 3000 = 0.3 %.
    pub fee_tier: u32,
    pub last_updated_block: u64,
    pub dex: DexKind,
}

// ─── ArbPath ─────────────────────────────────────────────────────────────────

/// A two-hop arbitrage path: token_in → pool_a → token_mid → pool_b → token_out.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArbPath {
    pub token_in: Address,
    pub pool_a: Address,
    pub token_mid: Address,
    pub pool_b: Address,
    pub token_out: Address,
    pub estimated_profit_wei: U256,
    pub flash_loan_amount_wei: U256,
}

impl ArbPath {
    /// Returns `true` when the path closes back to the input token.
    #[inline]
    pub fn is_circular(&self) -> bool {
        self.token_out == self.token_in
    }
}

// ─── Opportunity ─────────────────────────────────────────────────────────────

/// A flagged price dislocation that has cleared the dynamic profit threshold.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Opportunity {
    pub path: ArbPath,
    pub gross_profit_wei: U256,
    pub l2_gas_cost_wei: U256,
    pub l1_gas_cost_wei: U256,
    pub net_profit_wei: U256,
    /// Unix timestamp in milliseconds at which this opportunity was detected.
    pub detected_at_ms: u64,
}

impl Opportunity {
    /// Combined L2 execution + L1 calldata gas cost.
    #[inline]
    pub fn total_gas_cost_wei(&self) -> U256 {
        self.l2_gas_cost_wei + self.l1_gas_cost_wei
    }
}

// ─── SimulationResult ────────────────────────────────────────────────────────

/// Outcome of an in-process revm simulation.
#[derive(Debug, Clone, PartialEq)]
pub enum SimulationResult {
    Success { net_profit_wei: U256, gas_used: u64 },
    Failure { reason: String },
}

impl SimulationResult {
    /// Returns `true` for the `Success` variant.
    #[inline]
    pub fn is_success(&self) -> bool {
        matches!(self, Self::Success { .. })
    }

    /// Returns the profit when successful, `None` on failure.
    #[inline]
    pub fn profit(&self) -> Option<U256> {
        match self {
            Self::Success { net_profit_wei, .. } => Some(*net_profit_wei),
            Self::Failure { .. } => None,
        }
    }
}

// ─── SubmissionResult ────────────────────────────────────────────────────────

/// Outcome of an on-chain transaction submission.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmissionResult {
    pub tx_hash: TxHash,
    pub success: bool,
    pub revert_reason: Option<String>,
    pub gas_used: u64,
    pub l2_gas_cost_wei: U256,
    pub l1_gas_cost_wei: U256,
    /// Signed PnL: negative when total gas cost exceeds gross profit.
    pub net_pnl_wei: I256,
}

impl SubmissionResult {
    /// Returns `true` only when `net_pnl_wei` is strictly positive.
    #[inline]
    pub fn is_profitable(&self) -> bool {
        self.net_pnl_wei > I256::ZERO
    }
}

// ─── GasEstimate ─────────────────────────────────────────────────────────────

/// Full Arbitrum 2-D gas estimate (L2 execution + L1 calldata).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GasEstimate {
    pub l2_gas_units: u64,
    pub l2_gas_price_wei: u128,
    pub l2_cost_wei: U256,
    pub l1_calldata_gas: u64,
    pub l1_base_fee_wei: u128,
    pub l1_cost_wei: U256,
    pub total_cost_wei: U256,
    pub total_cost_usd: f64,
}

impl GasEstimate {
    /// Returns the pre-computed total cost field.
    #[inline]
    pub fn total_cost_wei(&self) -> U256 {
        self.total_cost_wei
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::address;

    // ── helpers ────────────────────────────────────────────────────────────

    /// Construct an Address with a single-byte suffix for readability in tests.
    fn addr(n: u8) -> Address {
        let mut bytes = [0u8; 20];
        bytes[19] = n;
        Address::from(bytes)
    }

    fn sample_pool_state() -> PoolState {
        PoolState {
            address: addr(1),
            token0: addr(2),
            token1: addr(3),
            reserve0: U256::from(1_000_000u64),
            reserve1: U256::from(2_000_000u64),
            fee_tier: 3000,
            last_updated_block: 42,
            dex: DexKind::CamelotV2,
        }
    }

    fn sample_arb_path(circular: bool) -> ArbPath {
        ArbPath {
            token_in: addr(1),
            pool_a: addr(2),
            token_mid: addr(3),
            pool_b: addr(4),
            token_out: if circular { addr(1) } else { addr(5) },
            estimated_profit_wei: U256::from(500u64),
            flash_loan_amount_wei: U256::from(10_000u64),
        }
    }

    fn sample_opportunity() -> Opportunity {
        Opportunity {
            path: sample_arb_path(true),
            gross_profit_wei: U256::from(200u64),
            l2_gas_cost_wei: U256::from(100u64),
            l1_gas_cost_wei: U256::from(50u64),
            net_profit_wei: U256::from(50u64),
            detected_at_ms: 1_700_000_000_000,
        }
    }

    // ── PoolState tests ────────────────────────────────────────────────────

    #[test]
    fn test_pool_state_construction() {
        let ps = sample_pool_state();
        assert_eq!(ps.address, addr(1));
        assert_eq!(ps.token0, addr(2));
        assert_eq!(ps.token1, addr(3));
        assert_eq!(ps.reserve0, U256::from(1_000_000u64));
        assert_eq!(ps.reserve1, U256::from(2_000_000u64));
        assert_eq!(ps.fee_tier, 3000);
        assert_eq!(ps.last_updated_block, 42);
        assert_eq!(ps.dex, DexKind::CamelotV2);
    }

    #[test]
    fn test_pool_state_serde_roundtrip() {
        let original = sample_pool_state();
        let json = serde_json::to_string(&original).unwrap();
        let decoded: PoolState = serde_json::from_str(&json).unwrap();
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_dex_kind_serde_snake_case() {
        let json = serde_json::to_string(&DexKind::UniswapV3).unwrap();
        assert_eq!(json, r#""uniswap_v3""#);
    }

    #[test]
    fn test_dex_kind_all_variants() {
        let cases = [
            (DexKind::UniswapV3, "uniswap_v3"),
            (DexKind::CamelotV2, "camelot_v2"),
            (DexKind::SushiSwap, "sushi_swap"),
            (DexKind::TraderJoeV1, "trader_joe_v1"),
        ];
        for (variant, expected) in cases {
            let json = serde_json::to_string(&variant).unwrap();
            assert_eq!(
                json,
                format!(r#""{expected}""#),
                "wrong JSON for {variant:?}"
            );
            let decoded: DexKind = serde_json::from_str(&json).unwrap();
            assert_eq!(decoded, variant);
        }
    }

    // ── ArbPath tests ──────────────────────────────────────────────────────

    #[test]
    fn test_arb_path_circular_true() {
        assert!(sample_arb_path(true).is_circular());
    }

    #[test]
    fn test_arb_path_circular_false() {
        assert!(!sample_arb_path(false).is_circular());
    }

    #[test]
    fn test_arb_path_serde_roundtrip() {
        let original = sample_arb_path(true);
        let json = serde_json::to_string(&original).unwrap();
        let decoded: ArbPath = serde_json::from_str(&json).unwrap();
        assert_eq!(original, decoded);
    }

    // ── Opportunity tests ──────────────────────────────────────────────────

    #[test]
    fn test_opportunity_total_gas() {
        // l2 = 100, l1 = 50  →  total = 150
        let opp = sample_opportunity();
        assert_eq!(opp.total_gas_cost_wei(), U256::from(150u64));
    }

    #[test]
    fn test_opportunity_serde_roundtrip() {
        let original = sample_opportunity();
        let json = serde_json::to_string(&original).unwrap();
        let decoded: Opportunity = serde_json::from_str(&json).unwrap();
        assert_eq!(original, decoded);
    }

    // ── SimulationResult tests ─────────────────────────────────────────────

    #[test]
    fn test_simulation_success_is_success() {
        let r = SimulationResult::Success {
            net_profit_wei: U256::from(1000u64),
            gas_used: 100_000,
        };
        assert!(r.is_success());
    }

    #[test]
    fn test_simulation_failure_is_not_success() {
        let r = SimulationResult::Failure {
            reason: "no profit".to_string(),
        };
        assert!(!r.is_success());
    }

    #[test]
    fn test_simulation_success_profit() {
        let r = SimulationResult::Success {
            net_profit_wei: U256::from(1000u64),
            gas_used: 100_000,
        };
        assert_eq!(r.profit(), Some(U256::from(1000u64)));
    }

    #[test]
    fn test_simulation_failure_profit() {
        let r = SimulationResult::Failure {
            reason: "stale reserves".to_string(),
        };
        assert_eq!(r.profit(), None);
    }

    // ── SubmissionResult tests ─────────────────────────────────────────────

    #[test]
    fn test_submission_profitable() {
        let r = SubmissionResult {
            tx_hash: TxHash::ZERO,
            success: true,
            revert_reason: None,
            gas_used: 200_000,
            l2_gas_cost_wei: U256::from(100u64),
            l1_gas_cost_wei: U256::from(50u64),
            net_pnl_wei: I256::from_dec_str("500").unwrap(),
        };
        assert!(r.is_profitable());
    }

    #[test]
    fn test_submission_unprofitable() {
        let r = SubmissionResult {
            tx_hash: TxHash::ZERO,
            success: false,
            revert_reason: Some("No profit".to_string()),
            gas_used: 200_000,
            l2_gas_cost_wei: U256::from(100u64),
            l1_gas_cost_wei: U256::from(50u64),
            net_pnl_wei: I256::from_dec_str("-150").unwrap(),
        };
        assert!(!r.is_profitable());
    }

    #[test]
    fn test_submission_serde_roundtrip() {
        let original = SubmissionResult {
            tx_hash: TxHash::ZERO,
            success: true,
            revert_reason: None,
            gas_used: 150_000,
            l2_gas_cost_wei: U256::from(80u64),
            l1_gas_cost_wei: U256::from(40u64),
            net_pnl_wei: I256::from_dec_str("200").unwrap(),
        };
        let json = serde_json::to_string(&original).unwrap();
        let decoded: SubmissionResult = serde_json::from_str(&json).unwrap();
        // SubmissionResult intentionally omits PartialEq — compare field-by-field.
        assert_eq!(original.tx_hash, decoded.tx_hash);
        assert_eq!(original.success, decoded.success);
        assert_eq!(original.revert_reason, decoded.revert_reason);
        assert_eq!(original.gas_used, decoded.gas_used);
        assert_eq!(original.l2_gas_cost_wei, decoded.l2_gas_cost_wei);
        assert_eq!(original.l1_gas_cost_wei, decoded.l1_gas_cost_wei);
        assert_eq!(original.net_pnl_wei, decoded.net_pnl_wei);
    }

    // ── address! macro sanity ───────────────────────────────────────────────

    #[test]
    fn test_address_macro_zero() {
        let z = address!("0000000000000000000000000000000000000000");
        assert_eq!(z, Address::ZERO);
    }
}
