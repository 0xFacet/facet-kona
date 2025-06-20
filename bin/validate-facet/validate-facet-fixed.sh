#!/usr/bin/env bash
set -euo pipefail

# RPC endpoints (must be set via environment variables)
L1_RPC="${L1_RPC:?Error: L1_RPC environment variable not set}"
L1_BEACON_RPC="${L1_BEACON_RPC:?Error: L1_BEACON_RPC environment variable not set}"
L2_RPC="${L2_RPC:?Error: L2_RPC environment variable not set}"
ROLLUP_NODE_RPC="${ROLLUP_NODE_RPC:?Error: ROLLUP_NODE_RPC environment variable not set}"
# Get the block number from the first argument
if [ $# -lt 1 ]; then
    echo "Usage: $0 <block_number> [rollup_config_path] [verbosity]"
    echo ""
    echo "Environment variables (with defaults):"
    echo "  L1_RPC=$L1_RPC"
    echo "  L1_BEACON_RPC=$L1_BEACON_RPC"
    echo "  L2_RPC=$L2_RPC"
    echo "  ROLLUP_NODE_RPC=$ROLLUP_NODE_RPC"
    exit 1
fi

BLOCK_NUMBER=$1
# Use Facet rollup config by default
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROLLUP_CONFIG_PATH=${2:-"$SCRIPT_DIR/facet-mainnet-rollup-config.json"}
VERBOSITY=${3:-''}

# Move to the workspace root
cd "$(git rev-parse --show-toplevel)"

# Skip build - assume it's already built by Ruby script
# echo "Building kona-host..."
# cargo build --bin kona-host --release

# Get L2 chain ID from the rollup config
L2_CHAIN_ID=$(jq -r '.l2_chain_id' < "$ROLLUP_CONFIG_PATH")
echo "Using rollup config: $ROLLUP_CONFIG_PATH (L2 chain ID: $L2_CHAIN_ID)"

# Minimal output during bulk operations
# echo ""
# echo "Validating Facet block #$BLOCK_NUMBER..."

# Get output root for block
CLAIMED_L2_OUTPUT_ROOT=$(curl -s -X POST "$ROLLUP_NODE_RPC" -H "Content-Type: application/json" \
    -d "{\"jsonrpc\":\"2.0\",\"method\":\"optimism_outputAtBlock\",\"params\":[\"$(printf "0x%x" $BLOCK_NUMBER)\"],\"id\":1}" \
    | jq -r .result.outputRoot)

# Get the info for the previous block
PREV_BLOCK=$((BLOCK_NUMBER - 1))
AGREED_L2_OUTPUT_ROOT=$(curl -s -X POST "$ROLLUP_NODE_RPC" -H "Content-Type: application/json" \
    -d "{\"jsonrpc\":\"2.0\",\"method\":\"optimism_outputAtBlock\",\"params\":[\"$(printf "0x%x" $PREV_BLOCK)\"],\"id\":1}" \
    | jq -r .result.outputRoot)

AGREED_L2_HEAD_HASH=$(curl -s -X POST "$L2_RPC" -H "Content-Type: application/json" \
    -d "{\"jsonrpc\":\"2.0\",\"method\":\"eth_getBlockByNumber\",\"params\":[\"$(printf "0x%x" $PREV_BLOCK)\", false],\"id\":1}" \
    | jq -r .result.hash)

L1_ORIGIN_NUM=$(curl -s -X POST "$ROLLUP_NODE_RPC" -H "Content-Type: application/json" \
    -d "{\"jsonrpc\":\"2.0\",\"method\":\"optimism_outputAtBlock\",\"params\":[\"$(printf "0x%x" $PREV_BLOCK)\"],\"id\":1}" \
    | jq -r .result.blockRef.l1origin.number)

L1_HEAD=$(curl -s -X POST "$L1_RPC" -H "Content-Type: application/json" \
    -d "{\"jsonrpc\":\"2.0\",\"method\":\"eth_getBlockByNumber\",\"params\":[\"$(printf "0x%x" $((L1_ORIGIN_NUM + 30)))\", false],\"id\":1}" \
    | jq -r .result.hash)

# Minimal output
# echo "Running host program with native client program..."

# Set verbosity - minimal by default for bulk operations
if [ -z "$VERBOSITY" ]; then
    VERBOSITY="-vvv"  # Warn level only
fi

# Set environment variable for logging level
export RUST_LOG=${RUST_LOG:-info}

./target/release/kona-host $VERBOSITY single \
    --l1-head "$L1_HEAD" \
    --agreed-l2-head-hash "$AGREED_L2_HEAD_HASH" \
    --claimed-l2-output-root "$CLAIMED_L2_OUTPUT_ROOT" \
    --agreed-l2-output-root "$AGREED_L2_OUTPUT_ROOT" \
    --claimed-l2-block-number "$BLOCK_NUMBER" \
    --rollup-config-path "$ROLLUP_CONFIG_PATH" \
    --l1-node-address "$L1_RPC" \
    --l1-beacon-address "$L1_BEACON_RPC" \
    --l2-node-address "$L2_RPC" \
    --native \
    --data-dir "${DATA_DIR:-/tmp/data_block_${BLOCK_NUMBER}}"