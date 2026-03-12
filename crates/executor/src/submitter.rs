//! Mini-Phase 6.1 — Transaction Submitter with Mock Provider Tests.
//!
//! Provides a generic [`TransactionSubmitter<S>`] that:
//! * Estimates Arbitrum 2D gas (L2 execution + L1 calldata),
//! * Sends the signed transaction via a [`TransactionSender`] implementation,
//! * Polls for a receipt with a configurable timeout,
//! * Decodes revert data when the tx fails,
//! * Updates all SSOT Prometheus metrics.

use std::{sync::Arc, time::Duration};

use alloy::{
    network::TransactionBuilder,
    network::{Ethereum, EthereumWallet},
    primitives::{Address, Bytes, TxHash, I256, U256},
    providers::{Provider, ProviderBuilder, RootProvider},
    rpc::types::{TransactionReceipt, TransactionRequest},
    signers::local::PrivateKeySigner,
    sol_types::SolValue,
};
use anyhow::Context as _;
use async_trait::async_trait;
use tracing::{debug, info, warn};

use arbx_common::{
    config::ExecutionConfig,
    metrics::Metrics,
    types::{Opportunity, SubmissionResult},
};

// ─── TransactionSender trait ─────────────────────────────────────────────────

/// Async interface over the on-chain transaction lifecycle.
///
/// Annotated with `mockall::automock` in test builds so unit tests can inject
/// a [`MockTransactionSender`] without spinning up a real node.
#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait TransactionSender: Send + Sync {
    /// Sign and broadcast a transaction; return the transaction hash.
    async fn send(
        &self,
        calldata: Bytes,
        to: Address,
        gas_limit: u64,
        gas_price_wei: u128,
    ) -> anyhow::Result<TxHash>;

    /// Fetch the receipt for `tx_hash`, or `None` if not yet mined.
    async fn get_receipt(&self, tx_hash: TxHash) -> anyhow::Result<Option<TransactionReceipt>>;

    /// Return the current L2 base fee + priority fee in wei.
    async fn current_gas_price_wei(&self) -> anyhow::Result<u128>;

    /// Estimate the L1 calldata gas component via Arbitrum's NodeInterface.
    async fn estimate_l1_gas(&self, to: Address, data: Bytes) -> anyhow::Result<u64>;

    /// Estimate the L2 execution gas component via `eth_estimateGas`.
    async fn estimate_l2_gas(&self, to: Address, data: Bytes) -> anyhow::Result<u64>;

    /// Replay the call via `eth_call` and return raw revert bytes on failure.
    async fn call_for_revert(&self, to: Address, data: Bytes) -> anyhow::Result<Bytes>;
}

// ─── AlloyTransactionSender ──────────────────────────────────────────────────

/// Production [`TransactionSender`] backed by an Alloy provider.
pub struct AlloyTransactionSender {
    provider: Arc<RootProvider<Ethereum>>,
    signer: PrivateKeySigner,
    node_interface: Address,
}

impl AlloyTransactionSender {
    /// Construct from an existing provider, a private key signer, and the
    /// Arbitrum NodeInterface precompile address.
    pub fn new(
        provider: Arc<RootProvider<Ethereum>>,
        signer: PrivateKeySigner,
        node_interface: Address,
    ) -> Self {
        Self {
            provider,
            signer,
            node_interface,
        }
    }
}

#[async_trait]
impl TransactionSender for AlloyTransactionSender {
    async fn send(
        &self,
        calldata: Bytes,
        to: Address,
        gas_limit: u64,
        gas_price_wei: u128,
    ) -> anyhow::Result<TxHash> {
        let wallet = EthereumWallet::from(self.signer.clone());
        let url = "http://127.0.0.1:8545".parse().expect("static URL");
        let signing_provider = ProviderBuilder::new().wallet(wallet).connect_http(url);
        let tx = TransactionRequest::default()
            .with_from(self.signer.address())
            .with_to(to)
            .with_input(calldata)
            .with_gas_limit(gas_limit)
            .with_gas_price(gas_price_wei);
        let pending = signing_provider.send_transaction(tx).await?;
        Ok(*pending.tx_hash())
    }

    async fn get_receipt(&self, tx_hash: TxHash) -> anyhow::Result<Option<TransactionReceipt>> {
        Ok(self.provider.get_transaction_receipt(tx_hash).await?)
    }

    async fn current_gas_price_wei(&self) -> anyhow::Result<u128> {
        Ok(self.provider.get_gas_price().await?)
    }

    async fn estimate_l1_gas(&self, _to: Address, _data: Bytes) -> anyhow::Result<u64> {
        // TODO: call NodeInterface.gasEstimateL1Component (Phase 3).
        let _ = self.node_interface;
        Ok(0)
    }

    async fn estimate_l2_gas(&self, to: Address, data: Bytes) -> anyhow::Result<u64> {
        let tx = TransactionRequest::default().with_to(to).with_input(data);
        Ok(self.provider.estimate_gas(tx).await?)
    }

    async fn call_for_revert(&self, to: Address, data: Bytes) -> anyhow::Result<Bytes> {
        let tx = TransactionRequest::default().with_to(to).with_input(data);
        // `eth_call` returns an error containing the revert data when the call reverts.
        // We capture raw error text here; a more robust impl would decode the transport error.
        match self.provider.call(tx).await {
            Ok(_) => Ok(Bytes::new()),
            Err(e) => {
                // Best-effort: return empty; the error message may contain hex-encoded revert data.
                debug!("call_for_revert error: {e}");
                Ok(Bytes::new())
            }
        }
    }
}

// ─── decode_revert_reason ────────────────────────────────────────────────────

/// Decode ABI-encoded revert bytes into a human-readable string.
///
/// | Input                                     | Output                   |
/// |-------------------------------------------|--------------------------|
/// | Empty slice                               | `""` (empty string)      |
/// | `0x08c379a0` prefix (`Error(string)`)     | decoded reason string    |
/// | Anything else                             | hex-encoded bytes        |
pub fn decode_revert_reason(output: &[u8]) -> String {
    if output.is_empty() {
        return String::new();
    }
    if output.len() > 4 && output[..4] == [0x08_u8, 0xc3, 0x79, 0xa0] {
        if let Ok(reason) = String::abi_decode(&output[4..]) {
            return reason;
        }
    }
    alloy::hex::encode(output)
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Convert `I256` to `f64` for Prometheus gauges.
///
/// Uses `unsigned_abs()` to avoid overflow, then negates for negative values.
#[inline]
fn i256_to_f64(v: I256) -> f64 {
    let abs = v.unsigned_abs().to::<u128>() as f64;
    if v.is_negative() {
        -abs
    } else {
        abs
    }
}

// ─── TransactionSubmitter ────────────────────────────────────────────────────

/// Orchestrates the full submission lifecycle for one arb opportunity.
///
/// Generic over `S: TransactionSender` so the real alloy backend and the
/// mock are interchangeable.
pub struct TransactionSubmitter<S: TransactionSender> {
    sender: S,
    contract: Address,
    config: ExecutionConfig,
    metrics: Arc<Metrics>,
    receipt_timeout: Duration,
}

impl<S: TransactionSender> TransactionSubmitter<S> {
    /// Construct with the production 30 s receipt timeout.
    pub fn new(
        sender: S,
        contract: Address,
        config: ExecutionConfig,
        metrics: Arc<Metrics>,
    ) -> Self {
        let timeout_secs = config.receipt_timeout_secs;
        Self {
            sender,
            contract,
            config,
            metrics,
            receipt_timeout: Duration::from_secs(timeout_secs),
        }
    }

    /// Override the receipt-polling timeout — useful in tests outside this crate.
    pub fn with_receipt_timeout(mut self, timeout: Duration) -> Self {
        self.receipt_timeout = timeout;
        self
    }

    /// Submit `calldata` for `opportunity`:
    ///
    /// 1. Estimate L1 + L2 gas, compute buffered `gas_limit`.
    /// 2. Send the transaction.
    /// 3. Poll for the receipt, respecting `receipt_timeout`.
    /// 4. Decode revert reason if the tx failed.
    /// 5. Compute signed `net_pnl_wei` and update Prometheus metrics.
    pub async fn submit(
        &self,
        opportunity: &Opportunity,
        calldata: Bytes,
    ) -> anyhow::Result<SubmissionResult> {
        // ── 1. Gas estimation ─────────────────────────────────────────────
        let l1_gas = self
            .sender
            .estimate_l1_gas(self.contract, calldata.clone())
            .await
            .context("estimate_l1_gas failed")?;

        let l2_gas = self
            .sender
            .estimate_l2_gas(self.contract, calldata.clone())
            .await
            .context("estimate_l2_gas failed")?;

        let gas_price_wei = self
            .sender
            .current_gas_price_wei()
            .await
            .context("current_gas_price_wei failed")?;

        // ── 2. Buffered gas limit ─────────────────────────────────────────
        let combined_gas = l1_gas.saturating_add(l2_gas);
        let gas_limit = (combined_gas as f64 * self.config.gas_estimate_buffer).ceil() as u64;

        debug!(
            l1_gas,
            l2_gas, gas_limit, gas_price_wei, "submitting arb tx"
        );

        // ── 3. Send ───────────────────────────────────────────────────────
        let tx_hash = self
            .sender
            .send(calldata.clone(), self.contract, gas_limit, gas_price_wei)
            .await
            .context("send failed")?;

        info!(%tx_hash, "arb transaction submitted");
        self.metrics.transactions_submitted.inc();

        // ── 4. Poll for receipt ───────────────────────────────────────────
        let poll_interval = Duration::from_millis(200);
        let receipt = tokio::time::timeout(self.receipt_timeout, async {
            loop {
                match self.sender.get_receipt(tx_hash).await? {
                    Some(r) => return Ok::<_, anyhow::Error>(r),
                    None => tokio::time::sleep(poll_interval).await,
                }
            }
        })
        .await
        .context(format!(
            "receipt timeout: tx {} not mined within {}s",
            tx_hash,
            self.receipt_timeout.as_secs()
        ))??;

        // ── 5. Compute gas costs ──────────────────────────────────────────
        let l2_gas_cost_wei =
            U256::from(receipt.gas_used) * U256::from(receipt.effective_gas_price);
        let l1_gas_cost_wei = U256::from(l1_gas) * U256::from(gas_price_wei);
        let total_gas_cost_wei = l2_gas_cost_wei.saturating_add(l1_gas_cost_wei);

        self.metrics
            .gas_spent_wei
            .inc_by(total_gas_cost_wei.to::<u128>() as f64);

        // ── 6. Build result ───────────────────────────────────────────────
        if receipt.status() {
            let gross = I256::from_raw(opportunity.gross_profit_wei);
            let cost = I256::from_raw(total_gas_cost_wei);
            let net_pnl_wei = gross - cost;

            info!(%tx_hash, ?net_pnl_wei, "arb transaction succeeded");
            self.metrics.transactions_succeeded.inc();
            let pnl_f64 = i256_to_f64(net_pnl_wei);
            self.metrics.net_pnl_wei.add(pnl_f64);

            Ok(SubmissionResult {
                tx_hash,
                success: true,
                revert_reason: None,
                gas_used: receipt.gas_used,
                l2_gas_cost_wei,
                l1_gas_cost_wei,
                net_pnl_wei,
            })
        } else {
            // Replay the call to extract the revert string.
            let revert_bytes = self
                .sender
                .call_for_revert(self.contract, calldata)
                .await
                .unwrap_or_default();
            let reason = decode_revert_reason(&revert_bytes);

            let net_pnl_wei = -I256::from_raw(total_gas_cost_wei);

            warn!(%tx_hash, %reason, "arb transaction reverted");
            self.metrics
                .transactions_reverted
                .with_label_values(&[&reason])
                .inc();
            self.metrics.net_pnl_wei.add(i256_to_f64(net_pnl_wei));

            Ok(SubmissionResult {
                tx_hash,
                success: false,
                revert_reason: Some(reason),
                gas_used: receipt.gas_used,
                l2_gas_cost_wei,
                l1_gas_cost_wei,
                net_pnl_wei,
            })
        }
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use alloy::{
        consensus::{Eip658Value, Receipt, ReceiptEnvelope, ReceiptWithBloom},
        primitives::{Address, Bytes, TxHash, I256, U256},
        rpc::types::TransactionReceipt,
        sol_types::SolValue,
    };

    use arbx_common::{
        config::ExecutionConfig,
        metrics::Metrics,
        types::{ArbPath, Opportunity},
    };

    use super::{decode_revert_reason, MockTransactionSender, TransactionSubmitter};

    // ── Helpers ───────────────────────────────────────────────────────────────

    fn exec_config_with(l2_gas_units: u64, buffer: f64, timeout_secs: u64) -> ExecutionConfig {
        ExecutionConfig {
            contract_address: Address::ZERO.to_string(),
            private_key: "0xdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef"
                .to_string(),
            max_concurrent_simulations: 4,
            gas_estimate_buffer: buffer,
            node_interface_address: Address::ZERO.to_string(),
            l2_gas_units,
            receipt_timeout_secs: timeout_secs,
            dry_run: false,
        }
    }

    fn default_config() -> ExecutionConfig {
        exec_config_with(500_000, 1.2, 30)
    }

    fn metrics() -> Arc<Metrics> {
        Arc::new(Metrics::new().expect("metrics"))
    }

    fn make_opportunity(gross_profit_wei: U256) -> Opportunity {
        Opportunity {
            path: ArbPath {
                token_in: Address::ZERO,
                pool_a: Address::ZERO,
                token_mid: Address::ZERO,
                pool_b: Address::ZERO,
                token_out: Address::ZERO,
                estimated_profit_wei: gross_profit_wei,
                flash_loan_amount_wei: U256::ZERO,
            },
            gross_profit_wei,
            l2_gas_cost_wei: U256::from(1_000_000_u64),
            l1_gas_cost_wei: U256::from(100_000_u64),
            net_profit_wei: gross_profit_wei.saturating_sub(U256::from(1_100_000_u64)),
            detected_at_ms: 0,
        }
    }

    /// Build a [`TransactionReceipt`] with the given status, gas_used, and
    /// effective_gas_price. Uses an EIP-1559 envelope.
    fn make_receipt(success: bool, gas_used: u64, effective_gas_price: u128) -> TransactionReceipt {
        let inner = ReceiptEnvelope::Eip1559(ReceiptWithBloom {
            receipt: Receipt {
                status: Eip658Value::Eip658(success),
                cumulative_gas_used: gas_used,
                logs: vec![],
            },
            logs_bloom: Default::default(),
        });
        TransactionReceipt {
            inner,
            transaction_hash: TxHash::ZERO,
            transaction_index: Some(0),
            block_hash: None,
            block_number: Some(1),
            gas_used,
            effective_gas_price,
            blob_gas_used: None,
            blob_gas_price: None,
            from: Address::ZERO,
            to: Some(Address::ZERO),
            contract_address: None,
        }
    }

    /// Encode an `Error(string)` ABI revert payload.
    fn encode_error_string(msg: &str) -> Bytes {
        let mut out = vec![0x08_u8, 0xc3, 0x79, 0xa0];
        out.extend_from_slice(&msg.to_string().abi_encode());
        Bytes::from(out)
    }

    fn mock_with_receipt(success: bool) -> MockTransactionSender {
        let receipt = make_receipt(success, 21_000, 1_000_000_000);
        let mut mock = MockTransactionSender::new();
        mock.expect_estimate_l1_gas().returning(|_, _| Ok(10_000));
        mock.expect_estimate_l2_gas().returning(|_, _| Ok(21_000));
        mock.expect_current_gas_price_wei()
            .returning(|| Ok(1_000_000_000));
        mock.expect_send().returning(|_, _, _, _| Ok(TxHash::ZERO));
        mock.expect_get_receipt()
            .return_once(move |_| Ok(Some(receipt)));
        if !success {
            mock.expect_call_for_revert()
                .returning(|_, _| Ok(Bytes::new()));
        }
        mock
    }

    // ── Submit tests ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_submit_successful_tx() {
        let mock = mock_with_receipt(true);
        let sub = TransactionSubmitter::new(mock, Address::ZERO, default_config(), metrics());
        let result = sub
            .submit(
                &make_opportunity(U256::from(1_000_000_000_u64)),
                Bytes::new(),
            )
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.revert_reason.is_none());
        assert_eq!(result.tx_hash, TxHash::ZERO);
    }

    #[tokio::test]
    async fn test_submit_failed_tx_no_profit() {
        let receipt = make_receipt(false, 21_000, 1_000_000_000);
        let revert_bytes = encode_error_string("No profit");

        let mut mock = MockTransactionSender::new();
        mock.expect_estimate_l1_gas().returning(|_, _| Ok(0));
        mock.expect_estimate_l2_gas().returning(|_, _| Ok(21_000));
        mock.expect_current_gas_price_wei()
            .returning(|| Ok(1_000_000_000));
        mock.expect_send().returning(|_, _, _, _| Ok(TxHash::ZERO));
        mock.expect_get_receipt()
            .return_once(move |_| Ok(Some(receipt)));
        mock.expect_call_for_revert()
            .return_once(move |_, _| Ok(revert_bytes));

        let sub = TransactionSubmitter::new(mock, Address::ZERO, default_config(), metrics());
        let result = sub
            .submit(&make_opportunity(U256::ZERO), Bytes::new())
            .await
            .unwrap();

        assert!(!result.success);
        assert_eq!(result.revert_reason.as_deref(), Some("No profit"));
    }

    #[tokio::test]
    async fn test_submit_failed_tx_unknown_revert() {
        let receipt = make_receipt(false, 21_000, 1_000_000_000);
        let bad_bytes = Bytes::from(vec![0xde, 0xad, 0xbe, 0xef]);

        let mut mock = MockTransactionSender::new();
        mock.expect_estimate_l1_gas().returning(|_, _| Ok(0));
        mock.expect_estimate_l2_gas().returning(|_, _| Ok(21_000));
        mock.expect_current_gas_price_wei()
            .returning(|| Ok(1_000_000_000));
        mock.expect_send().returning(|_, _, _, _| Ok(TxHash::ZERO));
        mock.expect_get_receipt()
            .return_once(move |_| Ok(Some(receipt)));
        mock.expect_call_for_revert()
            .return_once(move |_, _| Ok(bad_bytes));

        let sub = TransactionSubmitter::new(mock, Address::ZERO, default_config(), metrics());
        let result = sub
            .submit(&make_opportunity(U256::ZERO), Bytes::new())
            .await
            .unwrap();

        assert!(!result.success);
        let reason = result.revert_reason.unwrap();
        assert!(reason.contains("deadbeef"), "expected hex, got: {reason}");
    }

    #[tokio::test]
    async fn test_gas_limit_includes_l1_component() {
        // l1=50_000, l2=200_000, buffer=1.2 → gas_limit = ceil(250_000 * 1.2) = 300_000
        let receipt = make_receipt(true, 250_000, 1_000_000_000);

        let mut mock = MockTransactionSender::new();
        mock.expect_estimate_l1_gas().returning(|_, _| Ok(50_000));
        mock.expect_estimate_l2_gas().returning(|_, _| Ok(200_000));
        mock.expect_current_gas_price_wei()
            .returning(|| Ok(1_000_000_000));
        mock.expect_send()
            .withf(|_, _, gas_limit, _| *gas_limit == 300_000)
            .return_once(|_, _, _, _| Ok(TxHash::ZERO));
        mock.expect_get_receipt()
            .return_once(move |_| Ok(Some(receipt)));

        let config = exec_config_with(200_000, 1.2, 30);
        let sub = TransactionSubmitter::new(mock, Address::ZERO, config, metrics());
        sub.submit(
            &make_opportunity(U256::from(1_000_000_000_u64)),
            Bytes::new(),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn test_metrics_incremented_on_success() {
        let mock = mock_with_receipt(true);
        let m = metrics();
        let sub = TransactionSubmitter::new(mock, Address::ZERO, default_config(), Arc::clone(&m));
        sub.submit(
            &make_opportunity(U256::from(1_000_000_000_u64)),
            Bytes::new(),
        )
        .await
        .unwrap();

        assert_eq!(m.transactions_submitted.get(), 1);
        assert_eq!(m.transactions_succeeded.get(), 1);
    }

    #[tokio::test]
    async fn test_metrics_incremented_on_revert() {
        let revert_bytes = encode_error_string("No profit");
        let receipt = make_receipt(false, 21_000, 1_000_000_000);

        let mut mock = MockTransactionSender::new();
        mock.expect_estimate_l1_gas().returning(|_, _| Ok(0));
        mock.expect_estimate_l2_gas().returning(|_, _| Ok(21_000));
        mock.expect_current_gas_price_wei()
            .returning(|| Ok(1_000_000_000));
        mock.expect_send().returning(|_, _, _, _| Ok(TxHash::ZERO));
        mock.expect_get_receipt()
            .return_once(move |_| Ok(Some(receipt)));
        mock.expect_call_for_revert()
            .return_once(move |_, _| Ok(revert_bytes));

        let m = metrics();
        let sub = TransactionSubmitter::new(mock, Address::ZERO, default_config(), Arc::clone(&m));
        sub.submit(&make_opportunity(U256::ZERO), Bytes::new())
            .await
            .unwrap();

        assert_eq!(m.transactions_submitted.get(), 1);
        assert_eq!(
            m.transactions_reverted
                .with_label_values(&["No profit"])
                .get(),
            1
        );
    }

    #[tokio::test]
    async fn test_receipt_timeout_returns_error() {
        let mut mock = MockTransactionSender::new();
        mock.expect_estimate_l1_gas().returning(|_, _| Ok(0));
        mock.expect_estimate_l2_gas().returning(|_, _| Ok(21_000));
        mock.expect_current_gas_price_wei()
            .returning(|| Ok(1_000_000_000));
        mock.expect_send().returning(|_, _, _, _| Ok(TxHash::ZERO));
        // Always returns None — never mined.
        mock.expect_get_receipt().returning(|_| Ok(None));

        let config = exec_config_with(500_000, 1.2, 0);
        let sub = TransactionSubmitter::new(mock, Address::ZERO, config, metrics())
            .with_receipt_timeout(std::time::Duration::from_millis(50));

        let err = sub
            .submit(&make_opportunity(U256::ZERO), Bytes::new())
            .await;
        assert!(err.is_err(), "expected timeout error");
        let msg = err.unwrap_err().to_string();
        assert!(
            msg.contains("timeout") || msg.contains("receipt"),
            "unexpected error: {msg}"
        );
    }

    #[tokio::test]
    async fn test_net_pnl_positive_on_success() {
        // Large profit: 0.01 ETH = 10^16 wei; gas: 21_000 * 10^9 ≈ 2.1×10^13 wei
        let receipt = make_receipt(true, 21_000, 1_000_000_000);

        let mut mock = MockTransactionSender::new();
        mock.expect_estimate_l1_gas().returning(|_, _| Ok(0));
        mock.expect_estimate_l2_gas().returning(|_, _| Ok(21_000));
        mock.expect_current_gas_price_wei()
            .returning(|| Ok(1_000_000_000));
        mock.expect_send().returning(|_, _, _, _| Ok(TxHash::ZERO));
        mock.expect_get_receipt()
            .return_once(move |_| Ok(Some(receipt)));

        let gross = U256::from(10_000_000_000_000_000_u64); // 0.01 ETH
        let sub = TransactionSubmitter::new(mock, Address::ZERO, default_config(), metrics());
        let result = sub
            .submit(&make_opportunity(gross), Bytes::new())
            .await
            .unwrap();

        assert!(
            result.net_pnl_wei > I256::ZERO,
            "expected positive PnL, got {:?}",
            result.net_pnl_wei
        );
    }

    #[tokio::test]
    async fn test_net_pnl_negative_on_revert() {
        let receipt = make_receipt(false, 21_000, 1_000_000_000);

        let mut mock = MockTransactionSender::new();
        mock.expect_estimate_l1_gas().returning(|_, _| Ok(0));
        mock.expect_estimate_l2_gas().returning(|_, _| Ok(21_000));
        mock.expect_current_gas_price_wei()
            .returning(|| Ok(1_000_000_000));
        mock.expect_send().returning(|_, _, _, _| Ok(TxHash::ZERO));
        mock.expect_get_receipt()
            .return_once(move |_| Ok(Some(receipt)));
        mock.expect_call_for_revert()
            .returning(|_, _| Ok(Bytes::new()));

        let sub = TransactionSubmitter::new(mock, Address::ZERO, default_config(), metrics());
        let result = sub
            .submit(&make_opportunity(U256::ZERO), Bytes::new())
            .await
            .unwrap();

        assert!(
            result.net_pnl_wei < I256::ZERO,
            "expected negative PnL on revert, got {:?}",
            result.net_pnl_wei
        );
    }

    // ── decode_revert_reason tests ────────────────────────────────────────────

    #[test]
    fn test_decode_revert_standard_error() {
        let payload = encode_error_string("No profit");
        let reason = decode_revert_reason(&payload);
        assert_eq!(reason, "No profit");
    }

    #[test]
    fn test_decode_revert_empty_returns_empty() {
        assert_eq!(decode_revert_reason(&[]), "");
    }

    #[test]
    fn test_decode_revert_non_abi_returns_hex() {
        let bytes = [0xde_u8, 0xad, 0xbe, 0xef];
        let reason = decode_revert_reason(&bytes);
        assert!(
            reason.contains("deadbeef"),
            "expected hex-encoded output, got: {reason}"
        );
    }
}
