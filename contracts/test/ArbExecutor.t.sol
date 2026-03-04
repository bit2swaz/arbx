// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {Test} from "forge-std/Test.sol";
import {ArbExecutor} from "../src/ArbExecutor.sol";

// ─── IVault helper for flash loan ────────────────────────────────────────────

interface IVault {
    function flashLoan(
        address recipient,
        address[] memory tokens,
        uint256[] memory amounts,
        bytes memory userData
    ) external;
}

// ─── FeeCapture — records Balancer feeAmounts[0] and repays ──────────────────

/// @notice Minimal IFlashLoanRecipient that records feeAmounts[0] and repays.
contract FeeCapture {
    uint256 public capturedFee;
    bool    public wasCalled;

    function receiveFlashLoan(
        address[] memory tokens,
        uint256[] memory amounts,
        uint256[] memory feeAmounts,
        bytes memory
    ) external {
        wasCalled   = true;
        capturedFee = feeAmounts[0];
        // Repay principal + fee (fee == 0 on Balancer V2).
        // forge-lint: disable-next-line(erc20-unchecked-transfer)
        // safe: USDC.e transfer return value intentionally ignored in this minimal test helper.
        bool ok = IERC20(tokens[0]).transfer(msg.sender, amounts[0] + feeAmounts[0]);
        require(ok, "FeeCapture: transfer failed");
    }
}

// Minimal IERC20 interface used by test helpers.
interface IERC20 {
    function transfer(address to, uint256 amount) external returns (bool);
    function balanceOf(address account) external view returns (uint256);
    function approve(address spender, uint256 amount) external returns (bool);
}

// Wrapper that lets us call receiveFlashLoan with address[] (ABI-identical to IERC20[]).
// vm.expectRevert only intercepts typed calls, not raw .call() bytes.
interface IReceiver {
    function receiveFlashLoan(
        address[] memory tokens,
        uint256[] memory amounts,
        uint256[] memory fees,
        bytes memory userData
    ) external;
}

/// @dev Lets tests call executeArb with address[] instead of IERC20[]
///      (address[] is ABI-identical to IERC20[] — same selector and encoding).
interface IExecutor {
    struct ArbParamsExt {
        address tokenIn;
        address poolA;
        address tokenMid;
        address poolB;
        uint256 flashLoanAmount;
        uint256 minProfit;
        uint8 poolAKind;
        uint8 poolBKind;
    }
    function executeArb(
        address[] calldata tokens,
        uint256[] calldata amounts,
        ArbParamsExt calldata params
    ) external;
}

// ─── MockV2Pair — self-contained UniswapV2-compatible pair for TraderJoe tests ──

/// @notice Minimal Uniswap V2-compatible pair.
///         Implements getReserves(), token0(), token1(), and swap() with the
///         standard AMM formula (0.3% fee).  Used for the TraderJoe tests
///         because the on-chain TJ pair at the fork block uses a broken proxy.
contract MockV2Pair {
    address public token0;
    address public token1;
    uint112 private _reserve0;
    uint112 private _reserve1;

    constructor(address _t0, address _t1, uint112 r0, uint112 r1) {
        token0 = _t0;
        token1 = _t1;
        _reserve0 = r0;
        _reserve1 = r1;
    }

    function getReserves() external view returns (uint112 r0, uint112 r1, uint32 ts) {
        r0 = _reserve0;
        r1 = _reserve1;
        ts = uint32(block.timestamp);
    }

    /// @dev Standard V2 swap: caller must have already pushed tokenIn tokens to this contract.
    ///      Delivers amount0Out / amount1Out to `to`.
    function swap(uint256 amount0Out, uint256 amount1Out, address to, bytes calldata) external {
        require(amount0Out == 0 || amount1Out == 0, "MockV2Pair: invalid amounts");

        if (amount0Out > 0) {
            // Update reserves (simplified — no full K-invariant check for test helper).
            _reserve1 += uint112(IERC20(token1).balanceOf(address(this)) - _reserve1);
            // safe: amount0Out is bounded by _reserve0 (uint112), which was just read.
            // forge-lint: disable-next-line(unsafe-typecast)
            _reserve0 -= uint112(amount0Out);
            bool ok = IERC20(token0).transfer(to, amount0Out);
            require(ok, "MockV2Pair: transfer0 failed");
        } else {
            _reserve0 += uint112(IERC20(token0).balanceOf(address(this)) - _reserve0);
            // safe: amount1Out is bounded by _reserve1 (uint112), which was just read.
            // forge-lint: disable-next-line(unsafe-typecast)
            _reserve1 -= uint112(amount1Out);
            bool ok = IERC20(token1).transfer(to, amount1Out);
            require(ok, "MockV2Pair: transfer1 failed");
        }
    }
}

/// @notice Full fork TDD suite for ArbExecutor (Mini-Phase 2.3).
///         Run with: forge test -vvv --fork-url $ARBITRUM_RPC_URL
///
///         Test inventory:
///           Access control : 4 tests
///           Flash loan flow: 3 tests
///           Profit enforce : 4 tests
///           UniswapV3 swaps: 2 tests
///           CamelotV2 swaps: 2 tests
///           SushiSwap swaps: 2 tests
///           TraderJoe swaps: 2 tests
///           Edge cases     : 4 tests
///           ─────────────────────────
///           Total          : 23 tests
contract ArbExecutorTest is Test {
    ArbExecutor executor;

    // ── Balancer Vault ──────────────────────────────────────────────────────
    address constant BALANCER_VAULT     = 0xBA12222222228d8Ba445958a75a0704d566BF2C8;

    // ── Tokens ──────────────────────────────────────────────────────────────
    address constant USDC               = 0xFF970A61A04b1cA14834A43f5dE4533eBDDB5CC8; // USDC.e
    address constant WETH               = 0x82aF49447D8a07e3bd95BD0d56f35241523fBab1;
    address constant ARB                = 0x912CE59144191C1204E64559FE8253a0e49E6548;
    address constant WBTC               = 0x2f2a2543B76A4166549F7aaB2e75Bef0aefC5B0f;

    // ── UniswapV3 WETH/USDC.e 0.05% — $1.4M TVL (confirmed)
    address constant UNIV3_USDC_WETH    = 0xC31E54c7a869B9FcBEcc14363CF510d1c41fa443;

    // ── Camelot V2 WETH/USDC.e classic AMM — $155K TVL (confirmed)
    address constant CAMELOT_WETH_USDC  = 0x84652bb2539513BAf36e225c930Fdd8eaa63CE27;

    // ── SushiSwap V2 WETH/USDC.e — $163K TVL (confirmed)
    address constant SUSHI_WETH_USDC    = 0x905dfCD5649217c42684f23958568e533C711Aa3;

    // ── Trader Joe V1 WETH/USDC.e — minimal on-chain liquidity; seeded via vm.store
    address constant TJ_WETH_USDC       = 0x7eC3717f70894F6d9BA0be00774610394Ce006eE;

    // ── Pinned fork block: June 29 2023 — all pools active, stable state
    uint256 constant FORK_BLOCK         = 105_949_098;

    // ── Common amounts
    uint256 constant USDC_1000          = 1_000e6;  // 1000 USDC.e (6 decimals)
    uint256 constant WETH_1             = 1e18;     // 1 WETH (18 decimals)

    function setUp() public {
        vm.createSelectFork(vm.envString("ARBITRUM_RPC_URL"), FORK_BLOCK);
        executor = new ArbExecutor(BALANCER_VAULT, 1e15);
        vm.deal(address(executor), 0.01 ether);
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // HELPER
    // ═══════════════════════════════════════════════════════════════════════════

    /// @dev Seed a UniswapV2-style pair's reserves via vm.store.
    ///      UniswapV2Pair storage slot 8 packs:
    ///        bits   0..111 = reserve0 (uint112)
    ///        bits 112..223 = reserve1 (uint112)
    ///        bits 224..255 = blockTimestampLast (uint32)
    function _seedV2Reserves(address pair, uint112 reserve0, uint112 reserve1) internal {
        uint256 packed =
            uint256(reserve0) |
            (uint256(reserve1) << 112) |
            (uint256(uint32(block.timestamp)) << 224);
        vm.store(pair, bytes32(uint256(8)), bytes32(packed));
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // ACCESS CONTROL (4 tests)
    // ═══════════════════════════════════════════════════════════════════════════

    /// @notice Non-owner cannot call executeArb.
    function test_only_owner_execute_arb() public {
        address[] memory tokens  = new address[](1);
        uint256[] memory amounts = new uint256[](1);
        tokens[0]  = USDC;
        amounts[0] = USDC_1000;

        IExecutor.ArbParamsExt memory params = IExecutor.ArbParamsExt({
            tokenIn:         USDC,
            poolA:           UNIV3_USDC_WETH,
            tokenMid:        WETH,
            poolB:           SUSHI_WETH_USDC,
            flashLoanAmount: USDC_1000,
            minProfit:       1,
            poolAKind:       executor.DEX_UNIV3(),
            poolBKind:       executor.DEX_SUSHI()
        });

        vm.prank(address(0xdead));
        vm.expectRevert("Not owner");
        IExecutor(address(executor)).executeArb(tokens, amounts, params);
    }

    /// @notice Non-owner cannot call setMinProfit.
    function test_only_owner_set_min_profit() public {
        vm.prank(address(0xdead));
        vm.expectRevert("Not owner");
        executor.setMinProfit(1e18);
    }

    /// @notice Non-owner cannot call recoverTokens.
    function test_only_owner_recover_tokens() public {
        vm.prank(address(0xdead));
        vm.expectRevert("Not owner");
        executor.recoverTokens(USDC, 1);
    }

    /// @notice Direct call to receiveFlashLoan from a non-vault address reverts.
    function test_only_vault_receive_flash_loan() public {
        address[] memory tokens  = new address[](1);
        uint256[] memory amounts = new uint256[](1);
        uint256[] memory fees    = new uint256[](1);
        tokens[0]  = USDC;
        amounts[0] = USDC_1000;
        fees[0]    = 0;
        vm.expectRevert("Not Balancer");
        IReceiver(address(executor)).receiveFlashLoan(tokens, amounts, fees, bytes(""));
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // FLASH LOAN FLOW (3 tests)
    // ═══════════════════════════════════════════════════════════════════════════

    /// @notice Balancer V2 flash loan fee is always exactly zero.
    ///         Initiates a real 1000 USDC flash loan via the live Balancer Vault
    ///         and captures feeAmounts[0] inside the callback.
    function test_flash_loan_fee_is_zero() public {
        FeeCapture capturer = new FeeCapture();
        deal(USDC, address(capturer), USDC_1000); // repayment funds

        address[] memory tokens  = new address[](1);
        uint256[] memory amounts = new uint256[](1);
        tokens[0]  = USDC;
        amounts[0] = USDC_1000;

        IVault(BALANCER_VAULT).flashLoan(address(capturer), tokens, amounts, "");

        assertTrue(capturer.wasCalled(), "receiveFlashLoan must have been called");
        uint256 fee = capturer.capturedFee();
        assertEq(fee, 0, "Balancer V2 flash loan fee must be 0");
    }

    /// @notice After a complete arb the Balancer Vault's USDC balance is unchanged.
    ///         Pre-funds executor with loan + 50 USDC surplus (covers DEX fees),
    ///         sets minProfit=0, executes two-hop swap, asserts vault balance intact.
    function test_flash_loan_repays_principal() public {
        uint256 loanAmount = USDC_1000;
        // +50 USDC absorbs round-trip DEX fees (~0.3% UniV3 + ~0.3% Sushi ≈ 6 USDC).
        deal(USDC, address(executor), loanAmount + 50e6);
        executor.setMinProfit(0);

        uint256 vaultBefore = IERC20(USDC).balanceOf(BALANCER_VAULT);

        ArbExecutor.ArbParams memory params = ArbExecutor.ArbParams({
            tokenIn:         USDC,
            poolA:           UNIV3_USDC_WETH,
            tokenMid:        WETH,
            poolB:           SUSHI_WETH_USDC,
            flashLoanAmount: loanAmount,
            minProfit:       0,
            poolAKind:       executor.DEX_UNIV3(),
            poolBKind:       executor.DEX_SUSHI()
        });

        address[] memory tokens  = new address[](1);
        uint256[] memory amounts = new uint256[](1);
        uint256[] memory fees    = new uint256[](1);
        tokens[0]  = USDC;
        amounts[0] = loanAmount;
        fees[0]    = 0;

        vm.prank(BALANCER_VAULT);
        IReceiver(address(executor)).receiveFlashLoan(
            tokens, amounts, fees, abi.encode(params)
        );

        uint256 vaultAfter = IERC20(USDC).balanceOf(BALANCER_VAULT);
        // When using vm.prank, no tokens are actually lent from the vault.
        // The executor repays by safeTransfer(vault, loanAmount), so vault gains exactly loanAmount.
        assertEq(vaultAfter, vaultBefore + loanAmount, "Vault received exact principal repayment");
    }

    /// @notice receiveFlashLoan reverts with "No profit" when the round-trip
    ///         swap yields less than loanAmount + minProfit (which it always does
    ///         when executor holds exactly the loan amount and minProfit > 0).
    function test_flash_loan_reverts_if_not_repaid() public {
        uint256 loanAmount = USDC_1000;
        // Executor holds exactly loanAmount — round-trip DEX fees eat into this,
        // so balanceAfter < loanAmount < loanAmount + 1.
        deal(USDC, address(executor), loanAmount);

        ArbExecutor.ArbParams memory params = ArbExecutor.ArbParams({
            tokenIn:         USDC,
            poolA:           UNIV3_USDC_WETH,
            tokenMid:        WETH,
            poolB:           SUSHI_WETH_USDC,
            flashLoanAmount: loanAmount,
            minProfit:       1, // require at least 1 wei profit — will not be met
            poolAKind:       executor.DEX_UNIV3(),
            poolBKind:       executor.DEX_SUSHI()
        });

        address[] memory tokens  = new address[](1);
        uint256[] memory amounts = new uint256[](1);
        uint256[] memory fees    = new uint256[](1);
        tokens[0]  = USDC;
        amounts[0] = loanAmount;
        fees[0]    = 0;

        vm.prank(BALANCER_VAULT);
        vm.expectRevert("No profit");
        IReceiver(address(executor)).receiveFlashLoan(
            tokens, amounts, fees, abi.encode(params)
        );
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // PROFIT ENFORCEMENT (4 tests)
    // ═══════════════════════════════════════════════════════════════════════════

    /// @notice Profit require triggers when output < input + 1.
    ///         DEX round-trip fees ensure output < input; require("No profit") fires.
    function test_profit_require_triggers() public {
        uint256 loanAmount = USDC_1000;
        deal(USDC, address(executor), loanAmount);
        executor.setMinProfit(0); // contract min = 0 but per-trade = 1

        ArbExecutor.ArbParams memory params = ArbExecutor.ArbParams({
            tokenIn:         USDC,
            poolA:           UNIV3_USDC_WETH,
            tokenMid:        WETH,
            poolB:           SUSHI_WETH_USDC,
            flashLoanAmount: loanAmount,
            minProfit:       1,
            poolAKind:       executor.DEX_UNIV3(),
            poolBKind:       executor.DEX_SUSHI()
        });

        address[] memory tokens  = new address[](1);
        uint256[] memory amounts = new uint256[](1);
        uint256[] memory fees    = new uint256[](1);
        tokens[0]  = USDC;
        amounts[0] = loanAmount;
        fees[0]    = 0;

        vm.prank(BALANCER_VAULT);
        vm.expectRevert("No profit");
        IReceiver(address(executor)).receiveFlashLoan(
            tokens, amounts, fees, abi.encode(params)
        );
    }

    /// @notice Profit require passes when executor holds input + 2 × minProfit.
    ///         Pre-fund with large surplus (50 USDC) to absorb swap fees and satisfy minProfit.
    function test_profit_require_passes() public {
        uint256 loanAmount = USDC_1000;
        uint256 minProfit  = 1e6; // 1 USDC
        // 50 USDC surplus: ~6 USDC covers round-trip fees, ~44 USDC > minProfit.
        deal(USDC, address(executor), loanAmount + 50e6);
        executor.setMinProfit(0);

        ArbExecutor.ArbParams memory params = ArbExecutor.ArbParams({
            tokenIn:         USDC,
            poolA:           UNIV3_USDC_WETH,
            tokenMid:        WETH,
            poolB:           SUSHI_WETH_USDC,
            flashLoanAmount: loanAmount,
            minProfit:       minProfit,
            poolAKind:       executor.DEX_UNIV3(),
            poolBKind:       executor.DEX_SUSHI()
        });

        address[] memory tokens  = new address[](1);
        uint256[] memory amounts = new uint256[](1);
        uint256[] memory fees    = new uint256[](1);
        tokens[0]  = USDC;
        amounts[0] = loanAmount;
        fees[0]    = 0;

        uint256 ownerBefore = IERC20(USDC).balanceOf(address(this));

        vm.prank(BALANCER_VAULT);
        IReceiver(address(executor)).receiveFlashLoan(
            tokens, amounts, fees, abi.encode(params)
        );
        // Reaching this line means the profit require passed.
        // Owner must have received the surplus USDC after loan repayment.
        uint256 ownerAfter = IERC20(USDC).balanceOf(address(this));
        assertGt(ownerAfter - ownerBefore, 0, "Owner must have received profit after successful arb");
    }

    /// @notice Owner sets new minProfitWei; value is correctly stored.
    function test_set_min_profit_updates() public {
        assertEq(executor.minProfitWei(), 1e15);
        executor.setMinProfit(2e15);
        assertEq(executor.minProfitWei(), 2e15);
    }

    /// @notice Profit require blocks a losing arb:
    ///         executor starts with LESS than loanAmount → swaps eat further
    ///         → balanceAfter << amounts[0] → revert "No profit" before repay.
    function test_profit_prevents_loss() public {
        uint256 loanAmount = USDC_1000;
        // One USDC short — swaps will reduce further.
        deal(USDC, address(executor), loanAmount - 1);

        ArbExecutor.ArbParams memory params = ArbExecutor.ArbParams({
            tokenIn:         USDC,
            poolA:           UNIV3_USDC_WETH,
            tokenMid:        WETH,
            poolB:           SUSHI_WETH_USDC,
            flashLoanAmount: loanAmount,
            minProfit:       0,
            poolAKind:       executor.DEX_UNIV3(),
            poolBKind:       executor.DEX_SUSHI()
        });

        address[] memory tokens  = new address[](1);
        uint256[] memory amounts = new uint256[](1);
        uint256[] memory fees    = new uint256[](1);
        tokens[0]  = USDC;
        amounts[0] = loanAmount;
        fees[0]    = 0;

        vm.prank(BALANCER_VAULT);
        vm.expectRevert("No profit");
        IReceiver(address(executor)).receiveFlashLoan(
            tokens, amounts, fees, abi.encode(params)
        );
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // SWAP EXECUTION — UniswapV3 (2 tests)
    // ═══════════════════════════════════════════════════════════════════════════

    /// @notice USDC→WETH via UniswapV3 (Swap A), WETH→USDC via Camelot V2 (Swap B).
    ///         Tests the DEX_UNIV3 path: zeroForOne=false (USDC is token1, WETH is token0).
    ///         Using different pools avoids the same-pool exact-output accounting mismatch.
    function test_univ3_swap_usdc_to_weth() public {
        uint256 loanAmount = USDC_1000;
        // +100 USDC buffer: absorbs round-trip fees (UniV3 0.05% + Camelot ~0.3%).
        deal(USDC, address(executor), loanAmount + 100e6);
        executor.setMinProfit(0);

        ArbExecutor.ArbParams memory params = ArbExecutor.ArbParams({
            tokenIn:         USDC,
            poolA:           UNIV3_USDC_WETH,   // USDC→WETH via UniV3
            tokenMid:        WETH,
            poolB:           CAMELOT_WETH_USDC,  // WETH→USDC via Camelot V2
            flashLoanAmount: loanAmount,
            minProfit:       0,
            poolAKind:       executor.DEX_UNIV3(),
            poolBKind:       executor.DEX_CAMELOT()
        });

        address[] memory tokens  = new address[](1);
        uint256[] memory amounts = new uint256[](1);
        uint256[] memory fees    = new uint256[](1);
        tokens[0]  = USDC;
        amounts[0] = loanAmount;
        fees[0]    = 0;

        uint256 wethBefore = IERC20(WETH).balanceOf(address(executor));

        vm.prank(BALANCER_VAULT);
        IReceiver(address(executor)).receiveFlashLoan(
            tokens, amounts, fees, abi.encode(params)
        );

        // After the round-trip, WETH was received in Swap A and fully spent in Swap B.
        uint256 wethAfter = IERC20(WETH).balanceOf(address(executor));
        assertEq(wethAfter, wethBefore, "WETH must be fully consumed in Swap B (UniV3+Camelot)");
    }

    /// @notice WETH→USDC via UniswapV3 (Swap A), USDC→WETH via Camelot V2 (Swap B).
    ///         Tests the DEX_UNIV3 path: zeroForOne=true (WETH is token0).
    ///
    ///         Amount semantics: _swapV3 uses amountSpecified = -int256(flashLoanAmount),
    ///         which the V3 pool interprets as exact-output in OUTPUT token units.
    ///         With flashLoanAmount = USDC_1000 and tokenIn = WETH, the pool delivers
    ///         exactly 1000 USDC and bills the executor ~0.547 WETH (at 1830 USDC/WETH).
    ///         We pre-fund executor with 2 WETH so the V3 callback can pay the ~0.547 WETH
    ///         tab while still holding WETH_1 to repay the flash loan.
    function test_univ3_swap_weth_to_usdc() public {
        // Flash loan amount expressed in USDC units — V3 exact-output will deliver
        // exactly USDC_1000 USDC and charge ~0.547 WETH from the executor.
        uint256 loanAmount = USDC_1000; // = 1000e6 — used as Balancer loan & V3 exact-output
        // Pre-fund 2 WETH: ~0.547 WETH covers V3 Swap A cost, remainder repays flash loan.
        deal(WETH, address(executor), 2 * WETH_1);
        executor.setMinProfit(0);

        ArbExecutor.ArbParams memory params = ArbExecutor.ArbParams({
            tokenIn:         WETH,
            poolA:           UNIV3_USDC_WETH,   // WETH→USDC via UniV3 (zeroForOne=true)
            tokenMid:        USDC,
            poolB:           CAMELOT_WETH_USDC,  // USDC→WETH via Camelot V2
            flashLoanAmount: loanAmount,
            minProfit:       0,
            poolAKind:       executor.DEX_UNIV3(),
            poolBKind:       executor.DEX_CAMELOT()
        });

        address[] memory tokens  = new address[](1);
        uint256[] memory amounts = new uint256[](1);
        uint256[] memory fees    = new uint256[](1);
        tokens[0]  = WETH;
        amounts[0] = loanAmount;
        fees[0]    = 0;

        uint256 usdcBefore = IERC20(USDC).balanceOf(address(executor));

        vm.prank(BALANCER_VAULT);
        IReceiver(address(executor)).receiveFlashLoan(
            tokens, amounts, fees, abi.encode(params)
        );

        uint256 usdcAfter = IERC20(USDC).balanceOf(address(executor));
        assertEq(usdcAfter, usdcBefore, "USDC must be fully consumed in Swap B (UniV3+Camelot)");
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // SWAP EXECUTION — CamelotV2 (2 tests)
    // ═══════════════════════════════════════════════════════════════════════════

    /// @notice WETH→USDC→WETH via Camelot V2 AMM (CAMELOT_WETH_USDC).
    function test_camelot_swap_weth_to_usdc() public {
        uint256 loanAmount = WETH_1;
        deal(WETH, address(executor), loanAmount + loanAmount / 10);
        executor.setMinProfit(0);

        ArbExecutor.ArbParams memory params = ArbExecutor.ArbParams({
            tokenIn:         WETH,
            poolA:           CAMELOT_WETH_USDC,
            tokenMid:        USDC,
            poolB:           CAMELOT_WETH_USDC,
            flashLoanAmount: loanAmount,
            minProfit:       0,
            poolAKind:       executor.DEX_CAMELOT(),
            poolBKind:       executor.DEX_CAMELOT()
        });

        address[] memory tokens  = new address[](1);
        uint256[] memory amounts = new uint256[](1);
        uint256[] memory fees    = new uint256[](1);
        tokens[0]  = WETH;
        amounts[0] = loanAmount;
        fees[0]    = 0;

        uint256 usdcBefore = IERC20(USDC).balanceOf(address(executor));

        vm.prank(BALANCER_VAULT);
        IReceiver(address(executor)).receiveFlashLoan(
            tokens, amounts, fees, abi.encode(params)
        );

        uint256 usdcAfter = IERC20(USDC).balanceOf(address(executor));
        assertEq(usdcAfter, usdcBefore, "USDC must be fully consumed in Swap B (Camelot)");
    }

    /// @notice USDC→WETH→USDC via Camelot V2 AMM (reverse direction).
    function test_camelot_swap_usdc_to_weth() public {
        uint256 loanAmount = USDC_1000;
        deal(USDC, address(executor), loanAmount + 50e6);
        executor.setMinProfit(0);

        ArbExecutor.ArbParams memory params = ArbExecutor.ArbParams({
            tokenIn:         USDC,
            poolA:           CAMELOT_WETH_USDC,
            tokenMid:        WETH,
            poolB:           CAMELOT_WETH_USDC,
            flashLoanAmount: loanAmount,
            minProfit:       0,
            poolAKind:       executor.DEX_CAMELOT(),
            poolBKind:       executor.DEX_CAMELOT()
        });

        address[] memory tokens  = new address[](1);
        uint256[] memory amounts = new uint256[](1);
        uint256[] memory fees    = new uint256[](1);
        tokens[0]  = USDC;
        amounts[0] = loanAmount;
        fees[0]    = 0;

        uint256 wethBefore = IERC20(WETH).balanceOf(address(executor));

        vm.prank(BALANCER_VAULT);
        IReceiver(address(executor)).receiveFlashLoan(
            tokens, amounts, fees, abi.encode(params)
        );

        uint256 wethAfter = IERC20(WETH).balanceOf(address(executor));
        assertEq(wethAfter, wethBefore, "WETH must be fully consumed in Swap B (Camelot reverse)");
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // SWAP EXECUTION — SushiSwap (2 tests)
    // ═══════════════════════════════════════════════════════════════════════════

    /// @notice WETH→USDC→WETH via SushiSwap V2 AMM.
    function test_sushi_swap_weth_to_usdc() public {
        uint256 loanAmount = WETH_1;
        deal(WETH, address(executor), loanAmount + loanAmount / 10);
        executor.setMinProfit(0);

        ArbExecutor.ArbParams memory params = ArbExecutor.ArbParams({
            tokenIn:         WETH,
            poolA:           SUSHI_WETH_USDC,
            tokenMid:        USDC,
            poolB:           SUSHI_WETH_USDC,
            flashLoanAmount: loanAmount,
            minProfit:       0,
            poolAKind:       executor.DEX_SUSHI(),
            poolBKind:       executor.DEX_SUSHI()
        });

        address[] memory tokens  = new address[](1);
        uint256[] memory amounts = new uint256[](1);
        uint256[] memory fees    = new uint256[](1);
        tokens[0]  = WETH;
        amounts[0] = loanAmount;
        fees[0]    = 0;

        uint256 usdcBefore = IERC20(USDC).balanceOf(address(executor));

        vm.prank(BALANCER_VAULT);
        IReceiver(address(executor)).receiveFlashLoan(
            tokens, amounts, fees, abi.encode(params)
        );

        uint256 usdcAfter = IERC20(USDC).balanceOf(address(executor));
        assertEq(usdcAfter, usdcBefore, "USDC must be fully consumed in Swap B (Sushi)");
    }

    /// @notice USDC→WETH→USDC via SushiSwap V2 AMM (reverse direction).
    function test_sushi_swap_usdc_to_weth() public {
        uint256 loanAmount = USDC_1000;
        deal(USDC, address(executor), loanAmount + 50e6);
        executor.setMinProfit(0);

        ArbExecutor.ArbParams memory params = ArbExecutor.ArbParams({
            tokenIn:         USDC,
            poolA:           SUSHI_WETH_USDC,
            tokenMid:        WETH,
            poolB:           SUSHI_WETH_USDC,
            flashLoanAmount: loanAmount,
            minProfit:       0,
            poolAKind:       executor.DEX_SUSHI(),
            poolBKind:       executor.DEX_SUSHI()
        });

        address[] memory tokens  = new address[](1);
        uint256[] memory amounts = new uint256[](1);
        uint256[] memory fees    = new uint256[](1);
        tokens[0]  = USDC;
        amounts[0] = loanAmount;
        fees[0]    = 0;

        uint256 wethBefore = IERC20(WETH).balanceOf(address(executor));

        vm.prank(BALANCER_VAULT);
        IReceiver(address(executor)).receiveFlashLoan(
            tokens, amounts, fees, abi.encode(params)
        );

        uint256 wethAfter = IERC20(WETH).balanceOf(address(executor));
        assertEq(wethAfter, wethBefore, "WETH must be fully consumed in Swap B (Sushi reverse)");
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // SWAP EXECUTION — TraderJoe V1 (2 tests)
    // ═══════════════════════════════════════════════════════════════════════════
    //
    // The Trader Joe V1 pair at TJ_WETH_USDC on Arbitrum is a proxy whose
    // implementation reverts on getReserves() at this fork block.
    // We therefore deploy a self-contained MockV2Pair that implements the full
    // IUniswapV2Pair ABI.  DEX_TRADERJOE uses the same _swapV2 code path as
    // DEX_CAMELOT and DEX_SUSHI, so testing against MockV2Pair fully exercises
    // the TraderJoe branch.

    /// @notice WETH→USDC→WETH via Trader Joe V1 code path (MockV2Pair).
    function test_traderjoe_swap_weth_to_usdc() public {
        // Deploy a fresh MockV2Pair: token0=WETH, token1=USDC, reserves seeded.
        uint112 r0 = 1_000e18;     // 1000 WETH
        uint112 r1 = 2_000_000e6;  // 2,000,000 USDC → implied 2000 USDC/WETH
        MockV2Pair mockPair = new MockV2Pair(WETH, USDC, r0, r1);
        deal(WETH, address(mockPair), uint256(r0) + WETH_1 + WETH_1 / 10);
        deal(USDC, address(mockPair), uint256(r1));

        uint256 loanAmount = WETH_1;
        deal(WETH, address(executor), loanAmount + loanAmount / 10);
        executor.setMinProfit(0);

        ArbExecutor.ArbParams memory params = ArbExecutor.ArbParams({
            tokenIn:         WETH,
            poolA:           address(mockPair), // WETH→USDC
            tokenMid:        USDC,
            poolB:           address(mockPair), // USDC→WETH
            flashLoanAmount: loanAmount,
            minProfit:       0,
            poolAKind:       executor.DEX_TRADERJOE(),
            poolBKind:       executor.DEX_TRADERJOE()
        });

        address[] memory tokens  = new address[](1);
        uint256[] memory amounts = new uint256[](1);
        uint256[] memory fees    = new uint256[](1);
        tokens[0]  = WETH;
        amounts[0] = loanAmount;
        fees[0]    = 0;

        uint256 usdcBefore = IERC20(USDC).balanceOf(address(executor));

        vm.prank(BALANCER_VAULT);
        IReceiver(address(executor)).receiveFlashLoan(
            tokens, amounts, fees, abi.encode(params)
        );

        uint256 usdcAfter = IERC20(USDC).balanceOf(address(executor));
        assertEq(usdcAfter, usdcBefore, "USDC must be fully consumed in Swap B (TraderJoe)");
    }

    /// @notice USDC→WETH→USDC via Trader Joe V1 code path (MockV2Pair, reverse direction).
    function test_traderjoe_swap_usdc_to_weth() public {
        uint112 r0 = 1_000e18;
        uint112 r1 = 2_000_000e6;
        MockV2Pair mockPair = new MockV2Pair(WETH, USDC, r0, r1);
        deal(WETH, address(mockPair), uint256(r0));
        deal(USDC, address(mockPair), uint256(r1) + USDC_1000 + 50e6);

        uint256 loanAmount = USDC_1000;
        deal(USDC, address(executor), loanAmount + 50e6);
        executor.setMinProfit(0);

        ArbExecutor.ArbParams memory params = ArbExecutor.ArbParams({
            tokenIn:         USDC,
            poolA:           address(mockPair), // USDC→WETH
            tokenMid:        WETH,
            poolB:           address(mockPair), // WETH→USDC
            flashLoanAmount: loanAmount,
            minProfit:       0,
            poolAKind:       executor.DEX_TRADERJOE(),
            poolBKind:       executor.DEX_TRADERJOE()
        });

        address[] memory tokens  = new address[](1);
        uint256[] memory amounts = new uint256[](1);
        uint256[] memory fees    = new uint256[](1);
        tokens[0]  = USDC;
        amounts[0] = loanAmount;
        fees[0]    = 0;

        uint256 wethBefore = IERC20(WETH).balanceOf(address(executor));

        vm.prank(BALANCER_VAULT);
        IReceiver(address(executor)).receiveFlashLoan(
            tokens, amounts, fees, abi.encode(params)
        );

        uint256 wethAfter = IERC20(WETH).balanceOf(address(executor));
        assertEq(wethAfter, wethBefore, "WETH must be fully consumed in Swap B (TraderJoe reverse)");
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // EDGE CASES (4 tests)
    // ═══════════════════════════════════════════════════════════════════════════

    /// @notice Reentrancy guard blocks a second call to receiveFlashLoan
    ///         while the executor is already inside a flash loan callback.
    ///         We simulate mid-execution state by writing _executing=true into
    ///         ArbExecutor's storage slot 1 via vm.store.
    function test_reentrancy_guard_blocks() public {
        // ArbExecutor storage layout:
        //   slot 0: minProfitWei (uint256)
        //   slot 1: _executing   (bool)
        //   slot 2: _activeV3Pool (address, packed)
        vm.store(address(executor), bytes32(uint256(1)), bytes32(uint256(1)));

        address[] memory tokens  = new address[](1);
        uint256[] memory amounts = new uint256[](1);
        uint256[] memory fees    = new uint256[](1);
        tokens[0]  = USDC;
        amounts[0] = USDC_1000;
        fees[0]    = 0;

        ArbExecutor.ArbParams memory params = ArbExecutor.ArbParams({
            tokenIn:         USDC,
            poolA:           UNIV3_USDC_WETH,
            tokenMid:        WETH,
            poolB:           SUSHI_WETH_USDC,
            flashLoanAmount: USDC_1000,
            minProfit:       0,
            poolAKind:       executor.DEX_UNIV3(),
            poolBKind:       executor.DEX_SUSHI()
        });

        vm.prank(BALANCER_VAULT);
        vm.expectRevert("Reentrancy");
        IReceiver(address(executor)).receiveFlashLoan(
            tokens, amounts, fees, abi.encode(params)
        );
    }

    /// @notice Owner can recover ERC-20 tokens accidentally sent to the contract.
    function test_recover_tokens_works() public {
        deal(USDC, address(executor), 500e6);

        uint256 ownerBefore = IERC20(USDC).balanceOf(address(this));
        executor.recoverTokens(USDC, 500e6);
        uint256 ownerAfter = IERC20(USDC).balanceOf(address(this));

        assertEq(ownerAfter - ownerBefore, 500e6, "Owner must receive the recovered USDC");
        assertEq(IERC20(USDC).balanceOf(address(executor)), 0, "Executor USDC must be zero after recovery");
    }

    /// @notice ETH sent to the contract via receive() is accepted and balance increases.
    function test_receive_eth() public {
        uint256 balanceBefore = address(executor).balance;
        (bool ok,) = address(executor).call{value: 0.5 ether}("");
        assertTrue(ok, "ETH transfer must succeed");
        assertEq(
            address(executor).balance,
            balanceBefore + 0.5 ether,
            "Executor ETH balance must increase by exactly 0.5 ether"
        );
    }

    /// @notice Full end-to-end arb: exploit an artificial price dislocation between
    ///         SushiSwap and Camelot V2 WETH/USDC pools, executed via a real
    ///         Balancer V2 flash loan.
    ///
    ///         Method:
    ///           1. Seed SushiSwap reserves at 190 USDC/WETH (10× below market).
    ///           2. Camelot V2 remains at live market price (~1900 USDC/WETH).
    ///           3. Flash-loan 1000 USDC → buy ~5.2 WETH cheap on Sushi
    ///              → sell at market on Camelot → net ~8965 USDC profit.
    ///           4. Assert profit > 1000 USDC after repaying the flash loan.
    function test_full_arb_end_to_end() public {
        // ── 1. Create price dislocation on SushiSwap ──────────────────────────
        //
        // SUSHI_WETH_USDC token ordering:
        //   WETH (0x82..) < USDC (0xFF..) → token0=WETH, token1=USDC
        //
        // Seeded state: 10 WETH / 1900 USDC.e → implied price = 190 USDC/WETH.
        // Camelot V2 stays at live ~1900 USDC/WETH.
        uint112 sushi_r0 = 10e18;       // 10 WETH in SushiSwap pool
        uint112 sushi_r1 = 1_900e6;     // 1900 USDC.e (implied 190 USDC/WETH)
        _seedV2Reserves(SUSHI_WETH_USDC, sushi_r0, sushi_r1);
        deal(WETH, SUSHI_WETH_USDC, sushi_r0);
        deal(USDC, SUSHI_WETH_USDC, sushi_r1);

        // ── 2. Configure the arb ──────────────────────────────────────────────
        uint256 loanAmount = USDC_1000;
        uint256 minProfit  = 1_000e6;   // require >1000 USDC net profit
        executor.setMinProfit(0);       // use per-trade override from params

        ArbExecutor.ArbParams memory params = ArbExecutor.ArbParams({
            tokenIn:         USDC,
            poolA:           SUSHI_WETH_USDC,   // buy cheap WETH here (190 USDC/WETH)
            tokenMid:        WETH,
            poolB:           CAMELOT_WETH_USDC, // sell at market price (~1900 USDC/WETH)
            flashLoanAmount: loanAmount,
            minProfit:       minProfit,
            poolAKind:       executor.DEX_SUSHI(),
            poolBKind:       executor.DEX_CAMELOT()
        });

        // ── 3. Execute via real Balancer V2 flash loan ────────────────────────
        address[] memory flTokens  = new address[](1);
        uint256[] memory flAmounts = new uint256[](1);
        flTokens[0]  = USDC;
        flAmounts[0] = loanAmount;

        uint256 ownerBefore = IERC20(USDC).balanceOf(address(this));

        // executeArb is onlyOwner; test contract is the deployer/owner.
        // IExecutor uses address[] which is ABI-identical to IERC20[].
        IExecutor.ArbParamsExt memory extParams = IExecutor.ArbParamsExt({
            tokenIn:         params.tokenIn,
            poolA:           params.poolA,
            tokenMid:        params.tokenMid,
            poolB:           params.poolB,
            flashLoanAmount: params.flashLoanAmount,
            minProfit:       params.minProfit,
            poolAKind:       params.poolAKind,
            poolBKind:       params.poolBKind
        });
        IExecutor(address(executor)).executeArb(flTokens, flAmounts, extParams);

        uint256 ownerAfter = IERC20(USDC).balanceOf(address(this));
        uint256 profit = ownerAfter - ownerBefore;

        assertGt(profit, minProfit, "End-to-end arb profit must exceed 1000 USDC minProfit");
    }
}
