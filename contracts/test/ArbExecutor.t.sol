// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {Test} from "forge-std/Test.sol";
import {ArbExecutor} from "../src/ArbExecutor.sol";

// Minimal IERC20 interface used by test helpers.
interface IERC20 {
    function transfer(address to, uint256 amount) external returns (bool);
    function balanceOf(address account) external view returns (uint256);
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

/// @notice Placeholder test suite — full fork TDD coverage added in Mini-Phase 2.3.
///         These tests cover constructor invariants and access control without
///         requiring a live RPC or deployed Balancer Vault.
contract ArbExecutorTest is Test {
    ArbExecutor executor;

    /// @dev Balancer V2 Vault address on Arbitrum.
    address constant BALANCER_VAULT = 0xBA12222222228d8Ba445958a75a0704d566BF2C8;

    function setUp() public {
        executor = new ArbExecutor(BALANCER_VAULT, 1e15);
    }

    // ─── Constructor / invariants ─────────────────────────────────────────────

    /// @notice Deployer is recorded as owner.
    function test_owner_is_deployer() public view {
        assertEq(executor.owner(), address(this));
    }

    /// @notice Vault address is stored correctly.
    function test_balancer_vault_stored() public view {
        assertEq(address(executor.balancerVault()), BALANCER_VAULT);
    }

    /// @notice Initial minProfitWei is set from constructor argument.
    function test_initial_min_profit() public view {
        assertEq(executor.minProfitWei(), 1e15);
    }

    /// @notice Constructor reverts on zero vault address.
    function test_constructor_rejects_zero_vault() public {
        vm.expectRevert("Zero vault");
        new ArbExecutor(address(0), 1e15);
    }

    // ─── Access control ───────────────────────────────────────────────────────

    /// @notice Non-owner cannot call setMinProfit.
    function test_only_owner_set_min_profit() public {
        vm.prank(address(0xdead));
        vm.expectRevert("Not owner");
        executor.setMinProfit(1e18);
    }

    /// @notice Owner can update minProfitWei.
    function test_owner_can_set_min_profit() public {
        executor.setMinProfit(2e15);
        assertEq(executor.minProfitWei(), 2e15);
    }

    /// @notice setMinProfit emits MinProfitUpdated event with old and new values.
    function test_set_min_profit_emits_event() public {
        vm.expectEmit(false, false, false, true);
        emit ArbExecutor.MinProfitUpdated(1e15, 2e15);
        executor.setMinProfit(2e15);
    }

    /// @notice Direct call to receiveFlashLoan from non-vault reverts.
    function test_only_vault_receive_flash_loan() public {
        address[] memory tokens = new address[](1);
        uint256[] memory amounts = new uint256[](1);
        uint256[] memory fees = new uint256[](1);
        tokens[0] = address(0x1);
        amounts[0] = 1000e6;
        fees[0] = 0;
        // Use typed call via IReceiver so vm.expectRevert can intercept the revert.
        // address[] is ABI-identical to IERC20[] — same 20-byte address encoding.
        vm.expectRevert("Not Balancer");
        IReceiver(address(executor)).receiveFlashLoan(tokens, amounts, fees, bytes(""));
    }

    /// @notice Non-owner cannot call recoverTokens.
    function test_only_owner_recover_tokens() public {
        vm.prank(address(0xdead));
        vm.expectRevert("Not owner");
        executor.recoverTokens(address(0x1), 1);
    }

    // ─── DEX kind constants ───────────────────────────────────────────────────

    function test_dex_kind_constants() public view {
        assertEq(executor.DEX_UNIV3(), 0);
        assertEq(executor.DEX_CAMELOT(), 1);
        assertEq(executor.DEX_SUSHI(), 2);
        assertEq(executor.DEX_TRADERJOE(), 3);
    }
}
