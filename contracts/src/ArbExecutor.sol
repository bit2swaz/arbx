// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

/// @title  ArbExecutor
/// @notice Minimal stub for Mini-Phase 2.1.
///         Full implementation (Balancer V2 flash loan + two-hop atomic arb)
///         is written in Mini-Phase 2.2.
contract ArbExecutor {
    address public immutable owner;

    constructor() {
        owner = msg.sender;
    }
}
