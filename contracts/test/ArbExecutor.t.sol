// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import "forge-std/Test.sol";
import "../src/ArbExecutor.sol";

/// @notice Placeholder test suite — full TDD coverage added in Mini-Phase 2.3.
contract ArbExecutorTest is Test {
    ArbExecutor executor;

    function setUp() public {
        executor = new ArbExecutor();
    }

    /// @notice Deployer is recorded as owner.
    function test_owner_is_deployer() public view {
        assertEq(executor.owner(), address(this));
    }
}
