//! Mini-Phase 8.3 — Hot-path benchmarks.
//!
//! Run with:  `cargo bench --bench hot_paths`
//! Profile:   `cargo bench --bench hot_paths -- --profile-time 10`
//!
//! # BASELINE (recorded: 2026-03-08)
//! path_scan_100_pools:     479 µs
//! v2_compute/1000:          17 ns
//! v2_compute/10000:         17 ns
//! v2_compute/100000:        17 ns
//! v2_compute/1000000:       17 ns
//! profit_threshold:         21 ns
//! calldata_encode:         125 ns
//! pool_state_lookup:        51 ns

use std::hint::black_box;

use alloy::primitives::{Address, Bytes, U256};
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};

use arbx_common::types::{ArbPath, DexKind, PoolState};
use arbx_detector::opportunity::{compute_v2, PathScanner};
use arbx_detector::profit::compute_min_profit_pure;
use arbx_ingestion::pool_state::PoolStateStore;
use arbx_simulator::revm_sim::CallDataEncoder;

// ── Address helpers ───────────────────────────────────────────────────────────

/// Build a unique `Address` from a `u16` seed (bytes 18-19).
fn addr(seed: u16) -> Address {
    let mut b = [0u8; 20];
    b[18] = (seed >> 8) as u8;
    b[19] = (seed & 0xFF) as u8;
    Address::from(b)
}

/// Two shared "hub" tokens used to wire pools into two-hop cycles.
const TOKEN_A: fn() -> Address = || addr(0xF0_00);
const TOKEN_B: fn() -> Address = || addr(0xF0_01);

// ── Store builders ────────────────────────────────────────────────────────────

/// Build a `PoolStateStore` where every pool holds (TOKEN_A, TOKEN_B).
///
/// Because all pools share the same token pair the path scanner finds two-hop
/// cycles on every call — realistic for a hub-and-spoke DEX topology.
fn build_store(n: u16) -> PoolStateStore {
    let store = PoolStateStore::new();
    let token_a = TOKEN_A();
    let token_b = TOKEN_B();
    for i in 1..=n {
        store.upsert(PoolState {
            address: addr(i),
            token0: token_a,
            token1: token_b,
            reserve0: U256::from(10_000_000_u128 + u128::from(i) * 1_337),
            reserve1: U256::from(15_000_000_u128 + u128::from(i) * 997),
            fee_tier: 3000,
            last_updated_block: 1,
            dex: DexKind::CamelotV2,
        });
    }
    store
}

// ── ArbPath fixture ───────────────────────────────────────────────────────────

fn arb_path() -> ArbPath {
    ArbPath {
        token_in: TOKEN_A(),
        pool_a: addr(1),
        token_mid: TOKEN_B(),
        pool_b: addr(2),
        token_out: TOKEN_A(),
        estimated_profit_wei: U256::from(1_000_000_u128),
        flash_loan_amount_wei: U256::from(100_000_000_u128),
    }
}

// ── Benchmark 1: Path scanning ────────────────────────────────────────────────
//
// Creates a PathScanner backed by 100 pools and measures the cost of
// `scan(affected_pool)`.  Because all pools share TOKEN_A/TOKEN_B the
// scanner must walk through ~100 candidate pools per token direction, giving
// a realistic worst-case.  Target: < 1 ms.

fn bench_path_scan(c: &mut Criterion) {
    let store = build_store(100);
    let scanner = PathScanner::new(store);
    let affected = addr(1); // first pool

    c.bench_function("path_scan_100_pools", |b| {
        b.iter(|| scanner.scan(black_box(affected)))
    });
}

// ── Benchmark 2: AMM output calculation ──────────────────────────────────────
//
// Measures the constant-product output formula across four input sizes that
// span three orders of magnitude.  Target: < 1 µs (ideally < 100 ns).

fn bench_v2_output(c: &mut Criterion) {
    let reserve_in = U256::from(10_000_000_u128);
    let reserve_out = U256::from(15_000_000_u128);
    let fee_tier: u32 = 3000;

    let mut group = c.benchmark_group("amm_output");
    for &amount in &[1_000u64, 10_000, 100_000, 1_000_000] {
        group.bench_with_input(
            BenchmarkId::new("v2_compute", amount),
            &amount,
            |b, &amount| {
                b.iter(|| {
                    compute_v2(
                        black_box(U256::from(amount)),
                        black_box(reserve_in),
                        black_box(reserve_out),
                        black_box(fee_tier),
                    )
                })
            },
        );
    }
    group.finish();
}

// ── Benchmark 3: Profit threshold calculation ─────────────────────────────────
//
// `ProfitCalculator::compute_min_profit_wei` delegates entirely to the
// exported pure function `compute_min_profit_pure`.  We benchmark the pure
// function directly (no I/O, no mock required).
//
// Inputs represent a realistic $0.15 total gas cost at ETH = $3,000 with a
// 10 % buffer and a $0.50 floor.  Target: < 10 µs.

fn bench_profit_threshold(c: &mut Criterion) {
    // ~$0.15 at ETH = $3,000: (0.15 / 3000) * 1e18 ≈ 5e13 wei
    let total_cost_wei = U256::from(50_000_000_000_000_u128);
    let gas_buffer = 1.1_f64;
    let min_floor_usd = 0.50_f64;
    let eth_price = 3_000.0_f64;

    c.bench_function("profit_threshold", |b| {
        b.iter(|| {
            compute_min_profit_pure(
                black_box(total_cost_wei),
                black_box(gas_buffer),
                black_box(min_floor_usd),
                black_box(eth_price),
            )
        })
    });
}

// ── Benchmark 4: Calldata encoding ───────────────────────────────────────────
//
// `CallDataEncoder::encode_execute_arb` ABI-encodes the `executeArb` call
// including the 4-byte selector, two dynamic arrays, and the `ArbParams`
// struct.  This is on the critical path between simulation and submission.
// Target: < 100 µs (should comfortably beat that; it is pure in-memory work).

fn bench_calldata_encode(c: &mut Criterion) {
    let path = arb_path();
    let min_profit = U256::from(500_000_u128);

    c.bench_function("calldata_encode", |b| {
        b.iter(|| CallDataEncoder::encode_execute_arb(black_box(&path), black_box(min_profit)))
    });
}

// ── Benchmark 5: Pool state DashMap lookup ────────────────────────────────────
//
// Measures a single `PoolStateStore::get` on a store with 1 000 pools —
// representing peak production map size.  DashMap provides lock-free reads
// under concurrent access; this benchmark confirms sub-100-ns performance
// for the single-reader case.  Target: < 100 ns.

fn bench_pool_state_lookup(c: &mut Criterion) {
    let store = build_store(1_000);
    let target = addr(500); // near the middle

    c.bench_function("pool_state_lookup", |b| {
        b.iter(|| store.get(black_box(&target)))
    });
}

// ── Criterion wiring ──────────────────────────────────────────────────────────

criterion_group!(
    benches,
    bench_path_scan,
    bench_v2_output,
    bench_profit_threshold,
    bench_calldata_encode,
    bench_pool_state_lookup
);
criterion_main!(benches);

// ── Suppress unused-import warnings for Bytes (used by CallDataEncoder ret) ──
#[allow(dead_code)]
fn _use_bytes(_: Bytes) {}
