// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {Script, console} from "forge-std/Script.sol";
import {ArbExecutor} from "../src/ArbExecutor.sol";

/// @title Deploy — ArbExecutor deployment script
/// @notice Deploys ArbExecutor and records the deployment to deployments/<chainid>.json.
///
///         Usage (Sepolia — use scripts/deploy-sepolia.sh for convenience):
///           forge script script/Deploy.s.sol:Deploy \
///             --rpc-url $ARBITRUM_SEPOLIA_RPC_URL \
///             --broadcast --verify \
///             --etherscan-api-key $ARBISCAN_API_KEY \
///             -vvvv
///
///         Required env vars:
///           PRIVATE_KEY       — deployer private key (hex, 0x-prefixed)
///           BALANCER_VAULT    — Balancer V2 Vault address
///
///         Optional env vars:
///           MIN_PROFIT_WEI    — minimum profit threshold (default: 1e15 = 0.001 ETH)
contract Deploy is Script {
    function run() external {
        // ── Load deployment parameters ────────────────────────────────────────
        uint256 privateKey = vm.envUint("PRIVATE_KEY");
        address vault      = vm.envAddress("BALANCER_VAULT");
        uint256 minProfit  = vm.envOr("MIN_PROFIT_WEI", uint256(1e15));

        // ── Deploy ────────────────────────────────────────────────────────────
        vm.startBroadcast(privateKey);
        ArbExecutor executor = new ArbExecutor(vault, minProfit);
        vm.stopBroadcast();

        // ── Log deployment details ────────────────────────────────────────────
        console.log("=== ArbExecutor Deployed ===");
        console.log("Address  :", address(executor));
        console.log("Owner    :", executor.owner());
        console.log("Vault    :", address(executor.balancerVault()));
        console.log("MinProfit:", executor.minProfitWei());
        console.log("Block    :", block.number);
        console.log("Chain ID :", block.chainid);

        // ── Write deployment record to JSON ───────────────────────────────────
        // Uses vm.serialize* for spec-compliant JSON.
        // Path is relative to contracts/ (where foundry.toml lives).
        // Requires fs_permissions = [{ access = "read-write", path = "./" }]
        // in foundry.toml (already configured).
        string memory obj = "deployment";
        vm.serializeAddress(obj, "address",   address(executor));
        vm.serializeAddress(obj, "vault",     vault);
        vm.serializeUint(obj,    "minProfit", minProfit);
        vm.serializeUint(obj,    "block",     block.number);
        string memory finalJson = vm.serializeUint(obj, "chainId", block.chainid);

        string memory chainId      = vm.toString(block.chainid);
        string memory outPath      = string.concat("deployments/", chainId, ".json");
        vm.writeJson(finalJson, outPath);

        console.log("Deployment record written to:", outPath);
        console.log("=== Deployment complete ===");
    }
}
