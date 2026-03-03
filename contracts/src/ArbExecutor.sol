// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

// ─────────────────────────────────────────────────────────────────────────────
// Interfaces — all defined inline; no external dependencies required.
// ─────────────────────────────────────────────────────────────────────────────

/// @notice Minimal ERC-20 interface used by this contract.
interface IERC20 {
    function transfer(address to, uint256 amount) external returns (bool);
    function transferFrom(address from, address to, uint256 amount) external returns (bool);
    function balanceOf(address account) external view returns (uint256);
}

/// @notice Balancer V2 Vault — only the flashLoan entry point we need.
interface IVault {
    function flashLoan(
        IFlashLoanRecipient recipient,
        IERC20[] memory tokens,
        uint256[] memory amounts,
        bytes memory userData
    ) external;
}

/// @notice Balancer V2 flash loan callback interface.
interface IFlashLoanRecipient {
    function receiveFlashLoan(
        IERC20[] memory tokens,
        uint256[] memory amounts,
        uint256[] memory feeAmounts,
        bytes memory userData
    ) external;
}

/// @notice Uniswap V3 pool — swap and token0/token1 accessors.
interface IUniswapV3Pool {
    function swap(
        address recipient,
        bool zeroForOne,
        int256 amountSpecified,
        uint160 sqrtPriceLimitX96,
        bytes calldata data
    ) external returns (int256 amount0, int256 amount1);

    function token0() external view returns (address);
    function token1() external view returns (address);
}

/// @notice Uniswap V3 swap callback — must be implemented by the caller of pool.swap().
interface IUniswapV3SwapCallback {
    function uniswapV3SwapCallback(
        int256 amount0Delta,
        int256 amount1Delta,
        bytes calldata data
    ) external;
}

/// @notice Uniswap V2-style pair interface.
///         Used by CamelotV2, SushiSwap, and TraderJoeV1 — all share this ABI.
interface IUniswapV2Pair {
    function swap(uint256 amount0Out, uint256 amount1Out, address to, bytes calldata data)
        external;

    function getReserves()
        external
        view
        returns (uint112 reserve0, uint112 reserve1, uint32 blockTimestampLast);

    function token0() external view returns (address);
    function token1() external view returns (address);
}

// ─────────────────────────────────────────────────────────────────────────────
// SafeERC20 — handles tokens that do not return a bool on transfer (e.g. USDT).
// ─────────────────────────────────────────────────────────────────────────────

/// @dev Minimal SafeERC20 library. Wraps transfer/transferFrom and reverts on
///      any failure or false return value.
library SafeERC20 {
    /// @dev Safe transfer — reverts if the call fails or returns false.
    function safeTransfer(IERC20 token, address to, uint256 value) internal {
        _callOptionalReturn(token, abi.encodeCall(token.transfer, (to, value)));
    }

    /// @dev Safe transferFrom — reverts if the call fails or returns false.
    function safeTransferFrom(IERC20 token, address from, address to, uint256 value) internal {
        _callOptionalReturn(token, abi.encodeCall(token.transferFrom, (from, to, value)));
    }

    function _callOptionalReturn(IERC20 token, bytes memory data) private {
        (bool success, bytes memory returndata) = address(token).call(data);
        require(success, "SafeERC20: call failed");
        if (returndata.length > 0) {
            require(abi.decode(returndata, (bool)), "SafeERC20: op failed");
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ArbExecutor
// ─────────────────────────────────────────────────────────────────────────────

/// @title  ArbExecutor
/// @author arbx
/// @notice Atomic two-hop arbitrage executor powered by Balancer V2 flash loans.
///
///         Supported DEX venues
///         ─────────────────────
///         Kind 0  UniswapV3    (IUniswapV3Pool swap-callback model)
///         Kind 1  CamelotV2   (IUniswapV2Pair — Arbitrum native)
///         Kind 2  SushiSwap   (IUniswapV2Pair)
///         Kind 3  TraderJoeV1 (IUniswapV2Pair)
///
///         Execution flow
///         ──────────────
///         1. Owner calls executeArb() — triggers Balancer V2 flash loan.
///         2. Balancer Vault delivers tokens and calls receiveFlashLoan().
///         3. Swap A: tokenIn → tokenMid  (via poolA / poolAKind).
///         4. Swap B: tokenMid → tokenIn  (via poolB / poolBKind).
///         5. Profit check: balanceAfter ≥ flashLoanAmount + minProfit. Reverts if not met.
///         6. Principal repaid to Vault (Balancer V2 fee is structurally 0).
///         7. Net profit forwarded to owner.
///
///         Core invariants (from SSOT.md)
///         ────────────────────────────────
///         • Never holds inventory — every position opens and closes in one callback.
///         • Never executes at a loss — contract-level profit require is the backstop.
///         • Double protection — off-chain simulation (revm) + on-chain require.
contract ArbExecutor is IFlashLoanRecipient, IUniswapV3SwapCallback {
    using SafeERC20 for IERC20;

    // ─── DEX kind constants ───────────────────────────────────────────────────

    /// @notice Uniswap V3 pool kind identifier.
    uint8 public constant DEX_UNIV3 = 0;
    /// @notice Camelot V2 pair kind identifier (Uniswap V2-compatible, Arbitrum native).
    uint8 public constant DEX_CAMELOT = 1;
    /// @notice SushiSwap pair kind identifier (Uniswap V2-compatible).
    uint8 public constant DEX_SUSHI = 2;
    /// @notice Trader Joe V1 pair kind identifier (Uniswap V2-compatible).
    uint8 public constant DEX_TRADERJOE = 3;

    // ─── Uniswap V3 price limit constants ────────────────────────────────────

    /// @dev TickMath.MIN_SQRT_RATIO + 1.  Used as sqrtPriceLimitX96 when
    ///      selling token0 (zeroForOne = true) with no effective price limit.
    uint160 private constant _MIN_SQRT = 4_295_128_740;

    /// @dev TickMath.MAX_SQRT_RATIO - 1.  Used as sqrtPriceLimitX96 when
    ///      selling token1 (zeroForOne = false) with no effective price limit.
    uint160 private constant _MAX_SQRT =
        1_461_446_703_485_210_103_287_273_052_203_988_822_378_723_970_341;

    // ─── Immutables ───────────────────────────────────────────────────────────

    /// @notice Balancer V2 Vault address (0xBA12222222228d8Ba445958a75a0704d566BF2C8 on Arbitrum).
    IVault public immutable balancerVault;

    /// @notice Contract owner — the only address that can trigger arbs and receive profit.
    address public immutable owner;

    // ─── Storage ──────────────────────────────────────────────────────────────

    /// @notice Minimum profit required in tokenIn wei.
    ///         Updatable by owner to track gas cost changes.
    ///         Recommended: total_gas_cost × 1.1 + $0.50 equivalent in wei.
    uint256 public minProfitWei;

    /// @dev Simple reentrancy lock.  Set on entry to receiveFlashLoan, cleared on exit.
    bool private _executing;

    /// @dev Address of the V3 pool that initiated the current swap.
    ///      Set immediately before pool.swap(); cleared immediately after.
    ///      Used to authenticate the uniswapV3SwapCallback.
    address private _activeV3Pool;

    // ─── Structs ──────────────────────────────────────────────────────────────

    /// @notice Parameters for a two-hop arbitrage, ABI-encoded into Balancer userData.
    struct ArbParams {
        /// @dev Token to borrow via flash loan; also the final output token (circular path).
        address tokenIn;
        /// @dev First swap pool address (tokenIn → tokenMid).
        address poolA;
        /// @dev Intermediate token.
        address tokenMid;
        /// @dev Second swap pool address (tokenMid → tokenIn).
        address poolB;
        /// @dev Flash loan amount — must equal amounts[0] passed to executeArb.
        uint256 flashLoanAmount;
        /// @dev Per-trade minimum profit override (wei).  0 → use contract's minProfitWei.
        uint256 minProfit;
        /// @dev DEX kind for pool A (0 = UniswapV3, 1 = CamelotV2, 2 = SushiSwap, 3 = TraderJoe).
        uint8 poolAKind;
        /// @dev DEX kind for pool B.
        uint8 poolBKind;
    }

    // ─── Events ───────────────────────────────────────────────────────────────

    /// @notice Emitted on every successful arb execution.
    /// @param tokenIn Token that was borrowed and profited in.
    /// @param profit  Net profit forwarded to owner (wei).
    event ArbExecuted(address indexed tokenIn, uint256 profit);

    /// @notice Emitted when owner updates the minimum profit threshold.
    /// @param oldValue Previous minimum profit (wei).
    /// @param newValue New minimum profit (wei).
    event MinProfitUpdated(uint256 oldValue, uint256 newValue);

    // ─── Modifiers ────────────────────────────────────────────────────────────

    /// @dev Reverts with "Not owner" if caller is not the contract owner.
    modifier onlyOwner() {
        require(msg.sender == owner, "Not owner");
        _;
    }

    /// @dev Reverts with "Not Balancer" if caller is not the Balancer V2 Vault.
    modifier onlyVault() {
        require(msg.sender == address(balancerVault), "Not Balancer");
        _;
    }

    // ─── Constructor ──────────────────────────────────────────────────────────

    /// @notice Deploy ArbExecutor.
    /// @param _vault        Balancer V2 Vault address.
    ///                      Mainnet/Arbitrum: 0xBA12222222228d8Ba445958a75a0704d566BF2C8
    /// @param _minProfitWei Initial minimum profit threshold in wei.
    constructor(address _vault, uint256 _minProfitWei) {
        require(_vault != address(0), "Zero vault");
        balancerVault = IVault(_vault);
        owner = msg.sender;
        minProfitWei = _minProfitWei;
    }

    // ─── Owner functions ──────────────────────────────────────────────────────

    /// @notice Update the minimum profit threshold.
    /// @dev    Should be set to: total_gas_cost × 1.1 + $0.50 equivalent in wei.
    ///         Lower value → more opportunities but higher loss risk on gas spikes.
    ///         Higher value → fewer opportunities but safer margin.
    /// @param _minProfitWei New threshold in wei.
    function setMinProfit(uint256 _minProfitWei) external onlyOwner {
        emit MinProfitUpdated(minProfitWei, _minProfitWei);
        minProfitWei = _minProfitWei;
    }

    /// @notice Initiate an atomic two-hop arbitrage via Balancer V2 flash loan.
    /// @dev    Encodes `params` into Balancer userData and calls Vault.flashLoan().
    ///         Balancer immediately calls back receiveFlashLoan() on this contract.
    ///         This call reverts if the arb does not clear the profit threshold.
    /// @param tokens   Flash-loan token array.  Single element: [tokenIn].
    /// @param amounts  Flash-loan amount array.  Single element: [flashLoanAmount].
    /// @param params   Two-hop arb parameters.
    function executeArb(
        IERC20[] calldata tokens,
        uint256[] calldata amounts,
        ArbParams calldata params
    ) external onlyOwner {
        balancerVault.flashLoan(
            IFlashLoanRecipient(address(this)),
            tokens,
            amounts,
            abi.encode(params)
        );
    }

    /// @notice Recover any ERC-20 token accidentally stuck in this contract.
    /// @dev    Emergency use only — profits are swept automatically in receiveFlashLoan.
    /// @param token  Token address to recover.
    /// @param amount Amount to transfer to owner.
    function recoverTokens(address token, uint256 amount) external onlyOwner {
        IERC20(token).safeTransfer(owner, amount);
    }

    /// @notice Accept ETH (e.g., from WETH unwraps during emergency recovery).
    receive() external payable {}

    // ─── Balancer V2 callback ─────────────────────────────────────────────────

    /// @notice Balancer V2 flash loan callback.
    ///         Executes swap A and swap B, enforces minimum profit, repays the
    ///         flash loan principal (fee is always 0 on Balancer V2), and
    ///         forwards the net profit to the owner.
    ///
    ///         Balancer V2 flash loan fees are structurally 0 — this is a
    ///         protocol-level guarantee.  The require below catches any future
    ///         protocol change before we unknowingly absorb an unexpected cost.
    ///
    /// @param tokens      Flash-loaned tokens (tokens[0] = tokenIn).
    /// @param amounts     Flash-loaned amounts (amounts[0] = flashLoanAmount).
    /// @param feeAmounts  Fees charged by Balancer — always zero on V2.
    /// @param userData    ABI-encoded ArbParams.
    function receiveFlashLoan(
        IERC20[] memory tokens,
        uint256[] memory amounts,
        uint256[] memory feeAmounts,
        bytes memory userData
    ) external override onlyVault {
        require(!_executing, "Reentrancy");
        _executing = true;

        require(feeAmounts[0] == 0, "Unexpected flash loan fee");

        ArbParams memory params = abi.decode(userData, (ArbParams));

        // Resolve the effective minimum profit: per-trade override takes priority.
        uint256 minProfit = params.minProfit > 0 ? params.minProfit : minProfitWei;

        // ── Swap A: tokenIn → tokenMid ────────────────────────────────────────
        _executeSwap(
            params.poolA, params.poolAKind, params.tokenIn, params.tokenMid, params.flashLoanAmount
        );

        // ── Swap B: tokenMid → tokenIn ────────────────────────────────────────
        // Use the actual post-swap balance — more accurate than a computed amountOut.
        uint256 midBalance = IERC20(params.tokenMid).balanceOf(address(this));
        _executeSwap(params.poolB, params.poolBKind, params.tokenMid, params.tokenIn, midBalance);

        // ── Enforce profit BEFORE repaying flash loan ─────────────────────────
        // This is the on-chain correctness backstop.  Off-chain simulation (revm)
        // is the primary filter; this require catches any state change that occurred
        // between simulation and execution.
        uint256 balanceAfter = IERC20(params.tokenIn).balanceOf(address(this));
        require(balanceAfter >= amounts[0] + minProfit, "No profit");

        // ── Repay flash loan (principal only — fee is 0) ─────────────────────
        tokens[0].safeTransfer(address(balancerVault), amounts[0]);

        // ── Forward net profit to owner ───────────────────────────────────────
        uint256 profit = IERC20(params.tokenIn).balanceOf(address(this));
        if (profit > 0) {
            IERC20(params.tokenIn).safeTransfer(owner, profit);
        }

        emit ArbExecuted(params.tokenIn, profit);
        _executing = false;
    }

    // ─── Uniswap V3 callback ──────────────────────────────────────────────────

    /// @notice Uniswap V3 swap callback.
    ///         Called by the pool during pool.swap().  Transfers the owed input
    ///         token amount back to the pool.
    ///
    ///         Authentication: only the address stored in `_activeV3Pool` may
    ///         call this.  `_activeV3Pool` is set before and cleared after each
    ///         pool.swap() call inside _swapV3.
    ///
    /// @param amount0Delta Token0 owed to pool if positive; received from pool if negative.
    /// @param amount1Delta Token1 owed to pool if positive; received from pool if negative.
    /// @param data         ABI-encoded (address tokenIn) — the token we owe the pool.
    function uniswapV3SwapCallback(
        int256 amount0Delta,
        int256 amount1Delta,
        bytes calldata data
    ) external override {
        require(msg.sender == _activeV3Pool, "Unauthorized V3 callback");

        address tokenIn = abi.decode(data, (address));

        // Exactly one delta is positive — that is the amount we owe the pool.
        uint256 amountOwed;
        if (amount0Delta > 0) {
            // safe: amount0Delta > 0 is asserted by the branch condition above.
            // forge-lint: disable-next-line(unsafe-typecast)
            amountOwed = uint256(amount0Delta);
        } else {
            require(amount1Delta > 0, "Invalid V3 deltas");
            // safe: amount1Delta > 0 is asserted by the require above.
            // forge-lint: disable-next-line(unsafe-typecast)
            amountOwed = uint256(amount1Delta);
        }

        IERC20(tokenIn).safeTransfer(msg.sender, amountOwed);
    }

    // ─── Internal swap dispatcher ─────────────────────────────────────────────

    /// @dev Route a single swap to the correct DEX implementation.
    /// @param pool     Pool or pair address.
    /// @param kind     DEX kind constant (DEX_UNIV3 / DEX_CAMELOT / DEX_SUSHI / DEX_TRADERJOE).
    /// @param tokenIn  Token sold.
    /// @param tokenOut Token bought.
    /// @param amountIn Exact amount of tokenIn to sell.
    function _executeSwap(
        address pool,
        uint8 kind,
        address tokenIn,
        address tokenOut,
        uint256 amountIn
    ) internal {
        if (kind == DEX_UNIV3) {
            _swapV3(pool, tokenIn, tokenOut, amountIn);
        } else {
            // CamelotV2 (1), SushiSwap (2), and TraderJoeV1 (3) all implement
            // the standard Uniswap V2 pair ABI.
            _swapV2(pool, tokenIn, amountIn);
        }
    }

    // ─── Uniswap V3 swap ──────────────────────────────────────────────────────

    /// @dev Execute an exact-input swap on a Uniswap V3-compatible pool.
    ///
    ///      zeroForOne is derived from address ordering: Uniswap V3 sorts tokens
    ///      so that token0.address < token1.address.  If tokenIn < tokenOut,
    ///      tokenIn is token0 → zeroForOne = true.  This avoids an extra
    ///      pool.token0() external call.
    ///
    ///      sqrtPriceLimitX96 is set to the boundary value (MIN or MAX) so that
    ///      no price limit is imposed.  The on-chain profit require is the
    ///      economic safety check.
    ///
    ///      amountSpecified < 0 signals exact-input mode to the Uniswap V3 pool.
    ///
    /// @param pool     V3 pool address.
    /// @param tokenIn  Token sold (used for callback authentication data).
    /// @param tokenOut Token bought (used only to compute zeroForOne).
    /// @param amountIn Exact input amount.
    function _swapV3(address pool, address tokenIn, address tokenOut, uint256 amountIn)
        internal
    {
        bool zeroForOne = tokenIn < tokenOut;
        uint160 sqrtLimit = zeroForOne ? _MIN_SQRT : _MAX_SQRT;

        // Authenticate the callback from this specific pool.
        _activeV3Pool = pool;

        // safe: amountIn is a flash-loan amount bounded by pool reserves, which
        //       are capped at uint112 max (~5.19 × 10^33) << int256 max (~5.79 × 10^76).
        // forge-lint: disable-next-line(unsafe-typecast)
        int256 amountSpecified = -int256(amountIn);

        // amountSpecified < 0 → exact-input swap.
        IUniswapV3Pool(pool).swap(
            address(this),
            zeroForOne,
            amountSpecified,
            sqrtLimit,
            abi.encode(tokenIn)
        );

        // Clear the lock after the external call (and its callback) complete.
        _activeV3Pool = address(0);
    }

    // ─── Uniswap V2-style swap ────────────────────────────────────────────────

    /// @dev Execute a swap on a Uniswap V2-compatible pair.
    ///      Used for CamelotV2, SushiSwap, and TraderJoeV1.
    ///
    ///      V2 swap mechanics:
    ///       1. Push amountIn of tokenIn to the pair (pair reads its balance delta).
    ///       2. Compute amountOut with the standard 0.3% fee AMM formula.
    ///       3. Call pair.swap() to deliver tokenOut to this contract.
    ///
    ///      AMM formula (Uniswap V2 whitepaper §3.2.1, 0.3% fee ≡ 997/1000):
    ///        amountInWithFee = amountIn × 997
    ///        amountOut = (amountInWithFee × reserveOut)
    ///                    / (reserveIn × 1000 + amountInWithFee)
    ///
    ///      Token ordering is determined by comparing tokenIn against pair.token0()
    ///      — no need to pass tokenOut.
    ///
    /// @param pool     V2 pair address.
    /// @param tokenIn  Token sold.
    /// @param amountIn Exact input amount.
    function _swapV2(address pool, address tokenIn, uint256 amountIn) internal {
        IUniswapV2Pair pair = IUniswapV2Pair(pool);
        (uint112 reserve0, uint112 reserve1,) = pair.getReserves();
        bool isZeroForOne = tokenIn == pair.token0();

        (uint256 reserveIn, uint256 reserveOut) = isZeroForOne
            ? (uint256(reserve0), uint256(reserve1))
            : (uint256(reserve1), uint256(reserve0));

        // Push tokenIn to the pair (V2 balance-delta pull model).
        IERC20(tokenIn).safeTransfer(pool, amountIn);

        // Standard Uniswap V2 amountOut formula (0.3% fee).
        uint256 amountInWithFee = amountIn * 997;
        uint256 amountOut =
            (amountInWithFee * reserveOut) / (reserveIn * 1000 + amountInWithFee);

        (uint256 amount0Out, uint256 amount1Out) =
            isZeroForOne ? (uint256(0), amountOut) : (amountOut, uint256(0));

        pair.swap(amount0Out, amount1Out, address(this), "");
    }
}
