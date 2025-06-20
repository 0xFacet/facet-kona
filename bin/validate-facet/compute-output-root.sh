#!/usr/bin/env bash
# Compute output root for a Facet block without rollup node

set -euo pipefail

BLOCK_NUMBER=$1
L2_RPC="${L2_RPC:?Error: L2_RPC environment variable not set}"

# Get block info
BLOCK_INFO=$(curl -s -X POST "$L2_RPC" -H "Content-Type: application/json" \
    -d "{\"jsonrpc\":\"2.0\",\"method\":\"eth_getBlockByNumber\",\"params\":[\"$(printf "0x%x" $BLOCK_NUMBER)\", false],\"id\":1}")

STATE_ROOT=$(echo "$BLOCK_INFO" | jq -r .result.stateRoot)
BLOCK_HASH=$(echo "$BLOCK_INFO" | jq -r .result.hash)

# For Facet, withdrawal storage root is always empty (no withdrawals)
WITHDRAWAL_STORAGE_ROOT="0x56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421"

# Output root = keccak256(version || stateRoot || withdrawalStorageRoot || blockHash)
# Version 0 = 0x0000000000000000000000000000000000000000000000000000000000000000

# For now, return a placeholder - we'd need to implement the actual computation
echo "{\"stateRoot\": \"$STATE_ROOT\", \"blockHash\": \"$BLOCK_HASH\", \"withdrawalStorageRoot\": \"$WITHDRAWAL_STORAGE_ROOT\"}"