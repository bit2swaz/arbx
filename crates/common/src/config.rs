//! Configuration loading with environment variable expansion.
//!
//! `Config::load(path)` reads a TOML file, expands every `${VAR_NAME}` token
//! using the current process environment, then deserializes into the typed
//! struct hierarchy defined below.

use std::fs;

use anyhow::{anyhow, Context};
use serde::Deserialize;

// ── Public config structs ────────────────────────────────────────────────────

/// Top-level configuration — mirrors `config/default.toml` exactly.
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub network: NetworkConfig,
    pub strategy: StrategyConfig,
    pub pools: PoolsConfig,
    pub execution: ExecutionConfig,
    pub observability: ObservabilityConfig,
}

/// `[network]` section.
#[derive(Debug, Clone, Deserialize)]
pub struct NetworkConfig {
    pub rpc_url: String,
    pub sequencer_feed_url: String,
    pub chain_id: u64,
}

/// `[strategy]` section.
#[derive(Debug, Clone, Deserialize)]
pub struct StrategyConfig {
    pub min_profit_floor_usd: f64,
    pub gas_buffer_multiplier: f64,
    pub max_gas_gwei: f64,
    /// Balancer V2 flash-loan fee in basis points (always 0).
    pub flash_loan_fee_bps: u64,
}

/// `[pools]` section — verified on-chain addresses.
#[derive(Debug, Clone, Deserialize)]
pub struct PoolsConfig {
    pub balancer_vault: String,
    pub uniswap_v3_factory: String,
    pub camelot_factory: String,
    pub sushiswap_factory: String,
    pub traderjoe_factory: String,
}

/// `[execution]` section.
#[derive(Debug, Clone, Deserialize)]
pub struct ExecutionConfig {
    pub contract_address: String,
    pub private_key: String,
    pub max_concurrent_simulations: usize,
    pub gas_estimate_buffer: f64,
    /// Arbitrum NodeInterface precompile address for L1 calldata gas estimation.
    pub node_interface_address: String,
}

/// `[observability]` section.
#[derive(Debug, Clone, Deserialize)]
pub struct ObservabilityConfig {
    pub log_level: String,
    pub metrics_port: u16,
}

// ── Config impl ──────────────────────────────────────────────────────────────

impl Config {
    /// Load configuration from a TOML file at `path`.
    ///
    /// All `${VAR_NAME}` tokens are expanded from the process environment
    /// before parsing. Returns `Err` if the file is missing, any referenced
    /// env var is unset, or the TOML is malformed.
    pub fn load(path: &str) -> anyhow::Result<Self> {
        let raw = fs::read_to_string(path)
            .with_context(|| format!("Failed to read config file: {path}"))?;
        Self::load_str(&raw)
    }

    /// Load configuration from a TOML string.
    ///
    /// Identical to [`load`][Self::load] but reads from a `&str` instead of
    /// the filesystem. Used in tests to avoid touching disk.
    pub fn load_str(toml_str: &str) -> anyhow::Result<Self> {
        let expanded = Self::expand_env_vars(toml_str)?;
        toml::from_str(&expanded).context("Failed to parse TOML config")
    }

    /// Replace every `${VAR_NAME}` token in `input` with the value of the
    /// corresponding environment variable.
    ///
    /// Returns `Err("Missing env var: VAR_NAME")` if any referenced variable
    /// is not present in the process environment.
    fn expand_env_vars(input: &str) -> anyhow::Result<String> {
        let mut result = String::with_capacity(input.len());
        let mut remaining = input;

        while let Some(start) = remaining.find("${") {
            // Append everything before the `${`
            result.push_str(&remaining[..start]);
            remaining = &remaining[start + 2..];

            // Find the matching `}`
            let end = remaining
                .find('}')
                .ok_or_else(|| anyhow!("Unclosed '${{...}}' in config string"))?;

            let var_name = &remaining[..end];
            let value =
                std::env::var(var_name).map_err(|_| anyhow!("Missing env var: {var_name}"))?;

            result.push_str(&value);
            remaining = &remaining[end + 1..];
        }

        // Append any trailing content after the last `}`
        result.push_str(remaining);
        Ok(result)
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::env;

    use super::Config;

    /// A fully-substituted minimal valid config — no `${...}` placeholders.
    /// Used for tests that verify struct values rather than env-var expansion.
    const VALID_CONFIG: &str = r#"
[network]
rpc_url            = "https://arb-mainnet.g.alchemy.com/v2/TEST_KEY"
sequencer_feed_url = "wss://arb1.arbitrum.io/feed"
chain_id           = 42161

[strategy]
min_profit_floor_usd  = 0.50
gas_buffer_multiplier = 1.1
max_gas_gwei          = 0.1
flash_loan_fee_bps    = 0

[pools]
balancer_vault     = "0xBA12222222228d8Ba445958a75a0704d566BF2C8"
uniswap_v3_factory = "0x1F98431c8aD98523631AE4a59f267346ea31F984"
camelot_factory    = "0x6EcCab422D763aC031210895C81787E87B43A652"
sushiswap_factory  = "0xc35DADB65012eC5796536bD9864eD8773aBc74C4"
traderjoe_factory  = "0x9Ad6C38BE94206cA50bb0d90783181662f0CfA10"

[execution]
contract_address           = "0x0000000000000000000000000000000000000001"
private_key                = "0xdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef"
max_concurrent_simulations = 10
gas_estimate_buffer        = 1.2
node_interface_address     = "0x00000000000000000000000000000000000000C8"

[observability]
log_level    = "info"
metrics_port = 9090
"#;

    // ── load_str / TOML ──────────────────────────────────────────────────────

    #[test]
    fn test_load_str_valid_config() {
        let cfg = Config::load_str(VALID_CONFIG).expect("valid config must load");
        assert_eq!(cfg.network.chain_id, 42161);
        assert_eq!(cfg.strategy.flash_loan_fee_bps, 0);
    }

    #[test]
    fn test_load_str_invalid_toml() {
        let err = Config::load_str("!!! not valid toml @@@").unwrap_err();
        assert!(
            !err.to_string().is_empty(),
            "error message must not be empty"
        );
    }

    // ── expand_env_vars ──────────────────────────────────────────────────────

    #[test]
    fn test_expand_env_vars_substitutes_correctly() {
        env::set_var("ARBX_TEST_SUBST_1", "hello");
        let out = Config::expand_env_vars("${ARBX_TEST_SUBST_1}").unwrap();
        assert_eq!(out, "hello");
    }

    #[test]
    fn test_expand_env_vars_multiple() {
        env::set_var("ARBX_TEST_MULTI_A", "foo");
        env::set_var("ARBX_TEST_MULTI_B", "bar");
        let out =
            Config::expand_env_vars("prefix_${ARBX_TEST_MULTI_A}_mid_${ARBX_TEST_MULTI_B}_suffix")
                .unwrap();
        assert_eq!(out, "prefix_foo_mid_bar_suffix");
    }

    #[test]
    fn test_expand_env_vars_missing_var() {
        // Remove the variable to guarantee it is unset.
        env::remove_var("ARBX_TEST_MISSING_XYZ_42");
        let err = Config::expand_env_vars("${ARBX_TEST_MISSING_XYZ_42}").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("ARBX_TEST_MISSING_XYZ_42"),
            "error must name the missing var; got: {msg}"
        );
    }

    #[test]
    fn test_expand_env_vars_no_vars() {
        let input = "plain string with no dollar braces";
        let out = Config::expand_env_vars(input).unwrap();
        assert_eq!(out, input);
    }

    // ── Address / value correctness ──────────────────────────────────────────

    #[test]
    fn test_balancer_vault_address() {
        let cfg = Config::load_str(VALID_CONFIG).expect("valid config must load");
        assert_eq!(
            cfg.pools.balancer_vault,
            "0xBA12222222228d8Ba445958a75a0704d566BF2C8",
        );
    }

    #[test]
    fn test_node_interface_address() {
        let cfg = Config::load_str(VALID_CONFIG).expect("valid config must load");
        assert_eq!(
            cfg.execution.node_interface_address,
            "0x00000000000000000000000000000000000000C8",
        );
    }

    #[test]
    fn test_flash_loan_fee_is_zero() {
        let cfg = Config::load_str(VALID_CONFIG).expect("valid config must load");
        assert_eq!(cfg.strategy.flash_loan_fee_bps, 0);
    }

    #[test]
    fn test_chain_id_arbitrum() {
        let cfg = Config::load_str(VALID_CONFIG).expect("valid config must load");
        assert_eq!(cfg.network.chain_id, 42161);
    }
}
