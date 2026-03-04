// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {Script, console} from "forge-std/Script.sol";
import {stdJson} from "forge-std/StdJson.sol";

/// @title Verify — post-hoc ArbExecutor verification script
/// @notice Reads deployments/<chainid>.json (written by Deploy.s.sol) and
///         verifies ArbExecutor on Arbiscan via `forge verify-contract`.
///
///         Run Deploy.s.sol first — this script assumes the deployment file exists.
///
///         Usage:
///           forge script script/Verify.s.sol:Verify \
///             --rpc-url $ARBITRUM_RPC_URL
///
///         Required env vars:
///           ARBISCAN_API_KEY   — Arbiscan API key for verification
///
///         Requires:
///           ffi = true in foundry.toml (already configured)
///           fs_permissions = [{ access = "read-write", path = "./" }] (configured)
contract Verify is Script {
    using stdJson for string;

    function run() external {
        string memory chainId = vm.toString(block.chainid);
        string memory path    = string.concat("deployments/", chainId, ".json");

        // ── Read deployment record ────────────────────────────────────────────
        string memory json       = vm.readFile(path);
        address contractAddress  = json.readAddress(".address");
        address vault            = json.readAddress(".vault");
        uint256 minProfit        = json.readUint(".minProfit");
        uint256 deployBlock      = json.readUint(".block");

        // ── Log what we are verifying ─────────────────────────────────────────
        console.log("=== Verifying ArbExecutor ===");
        console.log("Chain ID  :", chainId);
        console.log("Address   :", contractAddress);
        console.log("Vault     :", vault);
        console.log("MinProfit :", minProfit);
        console.log("Block     :", deployBlock);

        // ── ABI-encode constructor args for the verifier ──────────────────────
        // ArbExecutor constructor(address _vault, uint256 _minProfitWei)
        bytes  memory constructorArgs    = abi.encode(vault, minProfit);
        string memory constructorArgsHex = vm.toString(constructorArgs);

        // ── Build forge verify-contract command ───────────────────────────────
        // forge verify-contract <addr> src/ArbExecutor.sol:ArbExecutor \
        //   --chain <chainId>               \
        //   --num-of-optimizations 200      \
        //   --constructor-args <hex>        \
        //   --etherscan-api-key <key>       \
        //   --watch
        string[] memory cmd = new string[](13);
        cmd[0]  = "forge";
        cmd[1]  = "verify-contract";
        cmd[2]  = vm.toString(contractAddress);
        cmd[3]  = "src/ArbExecutor.sol:ArbExecutor";
        cmd[4]  = "--chain";
        cmd[5]  = chainId;
        cmd[6]  = "--num-of-optimizations";
        cmd[7]  = "200";
        cmd[8]  = "--constructor-args";
        cmd[9]  = constructorArgsHex;
        cmd[10] = "--etherscan-api-key";
        cmd[11] = vm.envString("ARBISCAN_API_KEY");
        cmd[12] = "--watch";

        console.log("Running forge verify-contract ...");
        bytes memory result = vm.ffi(cmd);

        if (result.length > 0) {
            console.log("Verification output:", string(result));
        }
        console.log("=== Verification complete ===");
    }
}
