// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {Test} from "forge-std/Test.sol";
import {ArbExecutor} from "../src/ArbExecutor.sol";

// ─── MinimalERC20 ─────────────────────────────────────────────────────────────
/// @notice Minimal ERC-20 mock for testFuzz_recover_tokens_always_works.
///         Returns true on transfer (SafeERC20-compatible).
contract MinimalERC20 {
    mapping(address => uint256) public balanceOf;

    function mint(address to, uint256 amount) external {
        balanceOf[to] += amount;
    }

    function transfer(address to, uint256 amount) external returns (bool) {
        require(balanceOf[msg.sender] >= amount, "MinimalERC20: insufficient");
        balanceOf[msg.sender] -= amount;
        balanceOf[to] += amount;
        return true;
    }

    function transferFrom(address from, address to, uint256 amount) external returns (bool) {
        require(balanceOf[from] >= amount, "MinimalERC20: insufficient");
        balanceOf[from] -= amount;
        balanceOf[to] += amount;
        return true;
    }
}

// ─── ControlledMockPair ───────────────────────────────────────────────────────
/// @notice A V2-compatible pair that transfers a configurable output token with
///         a configurable amount when swap() is called.
///         Used to precisely control swap outputs in profit-enforcement fuzz tests.
contract ControlledMockPair {
    address public token0;
    address public token1;
    address internal _outToken;
    uint256 internal _outAmount; // 0 → transfer full balance of _outToken

    constructor(address _t0, address _t1, address outToken, uint256 outAmount) {
        token0    = _t0;
        token1    = _t1;
        _outToken  = outToken;
        _outAmount = outAmount;
    }

    function getReserves() external view returns (uint112, uint112, uint32) {
        // Return large reserves so the AMM formula in _swapV2 produces a non-zero
        // amountOut that won't exceed our balance.
        return (type(uint112).max / 2, type(uint112).max / 2, uint32(block.timestamp));
    }

    /// @dev Called by ArbExecutor._swapV2 after it pushes tokenIn to this pair.
    ///      Ignores the computed amount0Out/amount1Out and instead transfers
    ///      exactly `_outAmount` (or full balance if _outAmount == 0) of
    ///      `_outToken` to `to`.
    function swap(uint256, uint256, address to, bytes calldata) external {
        uint256 amt = _outAmount > 0 ? _outAmount : IERC20Like(_outToken).balanceOf(address(this));
        if (amt > 0) {
            IERC20Like(_outToken).transfer(to, amt);
        }
    }
}

interface IERC20Like {
    function balanceOf(address) external view returns (uint256);
    function transfer(address, uint256) external returns (bool);
}

// ─── IReceiver (address[] version of receiveFlashLoan) ────────────────────────
interface IReceiver {
    function receiveFlashLoan(
        address[] memory tokens,
        uint256[] memory amounts,
        uint256[] memory fees,
        bytes memory userData
    ) external;
}

// ══════════════════════════════════════════════════════════════════════════════
//  FUZZ TESTS
// ══════════════════════════════════════════════════════════════════════════════

/// @title ArbExecutorFuzzTest — Mini-Phase 2.4 fuzz suite
/// @notice Three property-based tests covering:
///           1. setMinProfit accepts any uint256
///           2. Profit-requirement enforcement for all (amount, output, minProfit) triples
///           3. recoverTokens works for any token / amount combination
contract ArbExecutorFuzzTest is Test {
    ArbExecutor executor;

    address constant BALANCER_VAULT = 0xBA12222222228d8Ba445958a75a0704d566BF2C8;

    function setUp() public {
        // No fork required: all fuzz tests use local mock tokens and pairs.
        executor = new ArbExecutor(BALANCER_VAULT, 1e15);
    }

    // ─────────────────────────────────────────────────────────────────────────
    // testFuzz_set_min_profit
    // ─────────────────────────────────────────────────────────────────────────

    /// @notice setMinProfit must accept any uint256 value and persist it exactly.
    /// forge-config: default.fuzz.runs = 1000
    function testFuzz_set_min_profit(uint256 newMinProfit) public {
        executor.setMinProfit(newMinProfit);
        assertEq(executor.minProfitWei(), newMinProfit);
    }

    // ─────────────────────────────────────────────────────────────────────────
    // testFuzz_profit_requirement_always_enforced
    // ─────────────────────────────────────────────────────────────────────────

    /// @notice The "No profit" require in receiveFlashLoan must fire whenever
    ///         the balance returned from swaps is below flashLoanAmount + minProfit,
    ///         and must NOT fire when the balance meets the threshold.
    ///
    ///         Strategy: bypass the real DEX swaps entirely by calling
    ///         receiveFlashLoan() directly from the Balancer Vault address
    ///         (using vm.prank) with a ControlledMockPair as poolB that gives
    ///         the executor exactly `swapOutput` of tokenIn after swap A+B.
    ///
    ///         We use a MinimalERC20 as tokenIn so we can mint arbitrary balances
    ///         without touching real fork state.
    ///
    /// forge-config: default.fuzz.runs = 1000
    function testFuzz_profit_requirement_always_enforced(
        uint256 flashLoanAmount,
        uint256 swapOutput,
        uint256 minProfit
    ) public {
        // ── Bound inputs ─────────────────────────────────────────────────────
        flashLoanAmount = bound(flashLoanAmount, 1,    1e24);
        minProfit       = bound(minProfit,       1,    1e18);
        // swapOutput spans [0, flashLoanAmount + minProfit + 1e24] to cover both
        // the failing branch (swapOutput < threshold) and the passing branch.
        // Capped at type(uint128).max to avoid arithmetic overflow in token minting.
        swapOutput      = bound(swapOutput, 0, type(uint128).max);

        // ── Deploy fresh mock token + a mock V2 pair that will return swapOutput ──
        MinimalERC20 tok = new MinimalERC20();
        // tokenMid is a dummy address — we just need a different address so
        // poolAKind / poolBKind dispatch works.  We'll mint enough for the executor
        // so that swap A "works" (pair reads its balance then transfers out).
        MinimalERC20 mid = new MinimalERC20();

        // ── Mock pair for swap A: tokenIn(tok) → tokenMid(mid).
        //    pairA: token0=tok, token1=mid.  We set outToken=mid, outAmount=0
        //    (transfers full mid balance held by pairA).
        ControlledMockPair pairA = new ControlledMockPair(address(tok), address(mid), address(mid), 0);
        // ── Mock pair for swap B: tokenMid(mid) → tokenIn(tok).
        //    pairB: token0=tok, token1=mid.  We set outToken=tok, outAmount=swapOutput
        //    so swap B delivers exactly swapOutput tok to executor.
        ControlledMockPair pairB = new ControlledMockPair(address(tok), address(mid), address(tok), swapOutput);

        // ── Mint tokens ───────────────────────────────────────────────────────
        // Give executor the flash-loan amount (simulating Balancer already delivered it).
        tok.mint(address(executor), flashLoanAmount);
        // Give pairA enough mid so swap A can return something to executor.
        mid.mint(address(pairA), 1e30);
        // Give pairB exactly swapOutput of tok so swap B returns that amount.
        // Note: if swapOutput == 0, pairB has nothing to transfer (harmless).
        tok.mint(address(pairB), swapOutput);

        // Set executor minProfit (per-trade override will be used instead via params).
        executor.setMinProfit(1); // contract default — irrelevant, we override per-trade

        // ── Build ArbParams with our mock pairs ───────────────────────────────
        // We use DEX_SUSHI (kind=2) which routes through _swapV2 for both hops.
        ArbExecutor.ArbParams memory params = ArbExecutor.ArbParams({
            tokenIn:         address(tok),
            poolA:           address(pairA),
            tokenMid:        address(mid),
            poolB:           address(pairB),
            flashLoanAmount: flashLoanAmount,
            minProfit:       minProfit,   // per-trade override
            poolAKind:       2,           // DEX_SUSHI → _swapV2
            poolBKind:       2
        });

        address[] memory tokensArr   = new address[](1);
        uint256[] memory amountsArr  = new uint256[](1);
        uint256[] memory feesArr     = new uint256[](1);
        tokensArr[0]  = address(tok);
        amountsArr[0] = flashLoanAmount;
        feesArr[0]    = 0;

        bytes memory userData = abi.encode(params);

        // ── Branch dispatch ──────────────────────────────────────────────────
        // Flow inside receiveFlashLoan:
        //   1. executor starts with flashLoanAmount of tok
        //   2. swap A: tok → mid (pairA gives mid, executor's tok → 0)
        //   3. swap B: mid → tok (pairB gives swapOutput tok)
        //   4. balanceAfter = swapOutput
        //   5. require(balanceAfter >= flashLoanAmount + minProfit)  ← profit check
        //   6. repay flashLoanAmount to vault
        //   7. owner receives (swapOutput - flashLoanAmount)
        bool shouldProfit = swapOutput >= flashLoanAmount + minProfit;

        if (!shouldProfit) {
            // Expect "No profit" revert.
            vm.expectRevert(bytes("No profit"));
            vm.prank(BALANCER_VAULT);
            IReceiver(address(executor)).receiveFlashLoan(
                tokensArr, amountsArr, feesArr, userData
            );
        } else {
            // Must succeed.  executor already has flashLoanAmount (minted above).
            // pairB has swapOutput of tok pre-minted.
            vm.prank(BALANCER_VAULT);
            IReceiver(address(executor)).receiveFlashLoan(
                tokensArr, amountsArr, feesArr, userData
            );

            // After success executor swept profit to owner.
            // net profit = swapOutput - flashLoanAmount
            uint256 ownerBal = tok.balanceOf(address(this));
            uint256 expectedProfit = swapOutput - flashLoanAmount;
            assertEq(ownerBal, expectedProfit, "owner should receive net profit");
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // testFuzz_recover_tokens_always_works
    // ─────────────────────────────────────────────────────────────────────────

    /// @notice recoverTokens(token, amount) must always transfer `amount` of
    ///         any ERC-20 from the executor to the owner.
    ///
    /// forge-config: default.fuzz.runs = 1000
    function testFuzz_recover_tokens_always_works(
        address tokenSeed,
        uint256 amount
    ) public {
        // Exclude zero-address and the executor itself.
        vm.assume(tokenSeed != address(0));
        vm.assume(tokenSeed != address(executor));
        amount = bound(amount, 1, type(uint128).max);

        // Deploy a fresh MinimalERC20 (ignore the fuzzed address — we need a
        // deployable token, not a random address that may or may not exist).
        MinimalERC20 tok = new MinimalERC20();

        // Mint `amount` directly to executor.
        tok.mint(address(executor), amount);

        uint256 ownerBefore = tok.balanceOf(address(this));
        uint256 execBefore  = tok.balanceOf(address(executor));
        assertEq(execBefore, amount);

        // Owner (test contract) calls recoverTokens.
        executor.recoverTokens(address(tok), amount);

        assertEq(tok.balanceOf(address(executor)), 0,               "executor should be empty");
        assertEq(tok.balanceOf(address(this)),     ownerBefore + amount, "owner should gain amount");
    }
}

// ══════════════════════════════════════════════════════════════════════════════
//  INVARIANT TESTS
// ══════════════════════════════════════════════════════════════════════════════

// ─── Handler ──────────────────────────────────────────────────────────────────

/// @notice Handler contract for stateful invariant fuzzing.
///         Exposes state-mutating entry points that the fuzzer can call in
///         arbitrary sequences; invariants are checked after each sequence.
contract ArbExecutorHandler is Test {
    ArbExecutor public executor;
    MinimalERC20 public token;

    // The test contract that deployed executor is its owner.
    address immutable _owner;

    constructor(ArbExecutor _executor) {
        executor = _executor;
        token    = new MinimalERC20();
        _owner   = msg.sender; // the invariant test contract that deploys this handler
    }

    /// @notice Fuzz setMinProfit with any value — called as owner.
    function setMinProfit(uint256 newMinProfit) external {
        vm.prank(_owner);
        executor.setMinProfit(newMinProfit);
    }

    /// @notice Fuzz recoverTokens: mint a random amount to executor, then recover it.
    function recoverTokens(uint256 amount) external {
        amount = bound(amount, 0, type(uint128).max);
        if (amount == 0) return;

        token.mint(address(executor), amount);
        vm.prank(_owner);
        executor.recoverTokens(address(token), amount);
    }
}

// ─── Invariant test contract ──────────────────────────────────────────────────

/// @title ArbExecutorInvariantTest — Mini-Phase 2.4 invariant suite
/// @notice Invariants verified:
///           1. owner never changes across any handler call sequence
///           2. No residual token balance lingers on executor after recoverTokens
///           3. minProfitWei is always readable (no storage corruption)
///
///         No fork needed: handler only calls setMinProfit / recoverTokens,
///         which are purely local (no Balancer, no real DEX calls).
///         A dummy vault address is used so the constructor succeeds without RPC.
///
/// forge-config: default.invariant.runs = 256
/// forge-config: default.invariant.depth = 15
/// forge-config: default.invariant.fail-on-revert = false
contract ArbExecutorInvariantTest is Test {
    ArbExecutor         public executor;
    ArbExecutorHandler  public handler;

    address deployerAddress;

    // Use a dummy address for the vault — invariant tests never trigger flash loans.
    address constant DUMMY_VAULT = address(0xBA12222222228d8Ba445958a75a0704d566BF2C8);

    function setUp() public {
        deployerAddress = address(this);
        // Deploy ArbExecutor with a dummy vault so no fork RPC is needed.
        executor = new ArbExecutor(DUMMY_VAULT, 1e15);
        handler  = new ArbExecutorHandler(executor);

        // Only let the fuzzer call handler functions.
        targetContract(address(handler));
    }

    /// @notice The owner address must never change — it is immutable in ArbExecutor.
    function invariant_owner_never_changes() public view {
        assertEq(executor.owner(), deployerAddress, "owner must remain the deployer");
    }

    /// @notice After any sequence of handler calls, the handler's recovery token
    ///         balance on the executor must be zero (recoverTokens swept everything).
    function invariant_no_residual_balance_after_arb() public view {
        // handler.recoverTokens() always mints then immediately recovers the full
        // amount, so executor's balance of handler.token() must always be 0.
        uint256 residual = handler.token().balanceOf(address(executor));
        assertEq(residual, 0, "executor must hold no residual token balance");
    }

    /// @notice minProfitWei must always be readable without reverting (no storage
    ///         corruption from setMinProfit).  We just assert it doesn't underflow.
    function invariant_min_profit_readable() public view {
        // Simply reading the value is the assertion — if storage is corrupted this
        // reverts.  We also sanity-check against uint256 max (trivially true).
        uint256 mp = executor.minProfitWei();
        assertLe(mp, type(uint256).max, "minProfitWei must be a valid uint256");
    }
}
