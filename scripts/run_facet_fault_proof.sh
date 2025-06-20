#!/usr/bin/env bash
set -euo pipefail

# Script to run Facet fault proofs with proper configuration

# RPC endpoints (must be set via environment variables)
L1_RPC="${L1_RPC:?Error: L1_RPC environment variable not set}"
L1_BEACON_RPC="${L1_BEACON_RPC:?Error: L1_BEACON_RPC environment variable not set}"
L2_RPC="${L2_RPC:?Error: L2_RPC environment variable not set}"
ROLLUP_NODE_RPC="${ROLLUP_NODE_RPC:?Error: ROLLUP_NODE_RPC environment variable not set}"

# Get the block number from the first argument
if [ $# -lt 1 ]; then
    echo "Usage: $0 <block_number> [verbosity]"
    echo ""
    echo "Environment variables (with defaults):"
    echo "  L1_RPC=$L1_RPC"
    echo "  L1_BEACON_RPC=$L1_BEACON_RPC"
    echo "  L2_RPC=$L2_RPC"
    echo "  ROLLUP_NODE_RPC=$ROLLUP_NODE_RPC"
    echo ""
    echo "Verbosity levels:"
    echo "  (empty) - Normal output"
    echo "  -v      - Info level"
    echo "  -vv     - Debug level"
    echo "  -vvv    - Trace level"
    exit 1
fi

BLOCK_NUMBER=$1
VERBOSITY=${2:-''}

# Use Facet rollup config
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROLLUP_CONFIG_PATH="$SCRIPT_DIR/../bin/validate-facet/facet-rollup-config.json"

# Move to the workspace root
cd "$(git rev-parse --show-toplevel)"

# Ensure rollup config exists
if [ ! -f "$ROLLUP_CONFIG_PATH" ]; then
    echo "Error: Rollup config not found at $ROLLUP_CONFIG_PATH"
    exit 1
fi

# Extract chain ID from rollup config
CHAIN_ID=$(jq -r '.l2_chain_id' "$ROLLUP_CONFIG_PATH")
echo "Using Facet chain ID: $CHAIN_ID"

echo "Building kona-host..."
cargo build --bin kona-host --release

# First check if we can connect to the rollup node
echo ""
echo "Checking rollup node connectivity..."
if ! curl -s "$ROLLUP_NODE_RPC" > /dev/null 2>&1; then
    echo "Error: Cannot connect to rollup node at $ROLLUP_NODE_RPC"
    echo "Please ensure a Facet rollup node is running with the optimism_outputAtBlock RPC method enabled"
    exit 1
fi

echo ""
echo "Fetching block information..."

# Get output root for the target block
CLAIMED_OUTPUT=$(cast rpc --rpc-url $ROLLUP_NODE_RPC "optimism_outputAtBlock" $(cast 2h $BLOCK_NUMBER) 2>/dev/null | jq -r '.outputRoot' 2>/dev/null || echo "")
if [ -z "$CLAIMED_OUTPUT" ] || [ "$CLAIMED_OUTPUT" = "null" ]; then
    echo "Error: Failed to get output root for block $BLOCK_NUMBER from rollup node"
    echo "Make sure the rollup node supports the optimism_outputAtBlock RPC method"
    exit 1
fi

# Get output root for the previous block
PREV_BLOCK=$((BLOCK_NUMBER - 1))
AGREED_OUTPUT=$(cast rpc --rpc-url $ROLLUP_NODE_RPC "optimism_outputAtBlock" $(cast 2h $PREV_BLOCK) 2>/dev/null | jq -r '.outputRoot' 2>/dev/null || echo "")
if [ -z "$AGREED_OUTPUT" ] || [ "$AGREED_OUTPUT" = "null" ]; then
    echo "Error: Failed to get output root for block $PREV_BLOCK from rollup node"
    exit 1
fi

# Get other required information
AGREED_L2_HEAD_HASH=$(cast block --rpc-url $L2_RPC $PREV_BLOCK --json | jq -r .hash)
L1_ORIGIN_NUM=$(cast rpc --rpc-url $ROLLUP_NODE_RPC "optimism_outputAtBlock" $(cast 2h $PREV_BLOCK) | jq -r .blockRef.l1origin.number)
L1_HEAD=$(cast block --rpc-url $L1_RPC $((L1_ORIGIN_NUM + 30)) --json | jq -r .hash)

echo ""
echo "Configuration:"
echo "  L2 Block:           $BLOCK_NUMBER"
echo "  Claimed Output:     $CLAIMED_OUTPUT"
echo "  Agreed L2 Block:    $PREV_BLOCK"
echo "  Agreed Output:      $AGREED_OUTPUT"
echo "  Agreed L2 Head:     $AGREED_L2_HEAD_HASH"
echo "  L1 Origin:          $L1_ORIGIN_NUM"
echo "  L1 Head:            $L1_HEAD"
echo "  Rollup Config:      $ROLLUP_CONFIG_PATH"

echo ""
echo "Running fault proof..."

# Set up logging based on verbosity
case "$VERBOSITY" in
    "-v")
        export RUST_LOG="info"
        ;;
    "-vv")
        export RUST_LOG="debug"
        ;;
    "-vvv")
        export RUST_LOG="trace"
        ;;
    *)
        export RUST_LOG="warn,kona_host=info,kona_client=info"
        ;;
esac

# Run the fault proof
# We try using the rollup config path to provide all chain-specific configuration
exec cargo run --bin kona-host --release -- \
    single \
    --l1-head "$L1_HEAD" \
    --agreed-l2-head-hash "$AGREED_L2_HEAD_HASH" \
    --agreed-l2-output-root "$AGREED_OUTPUT" \
    --claimed-l2-output-root "$CLAIMED_OUTPUT" \
    --claimed-l2-block-number "$BLOCK_NUMBER" \
    --l1-node-address "$L1_RPC" \
    --l1-beacon-address "$L1_BEACON_RPC" \
    --l2-node-address "$L2_RPC" \
    --rollup-config-path "$ROLLUP_CONFIG_PATH" \
    --native \
    --data-dir ./data \
    $VERBOSITY