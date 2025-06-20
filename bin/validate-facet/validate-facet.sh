#!/usr/bin/env bash
set -euo pipefail

# Determine network from environment variable
L1_NETWORK="${L1_NETWORK:-mainnet}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

if [ "$L1_NETWORK" = "mainnet" ]; then
    ROLLUP_CONFIG_DEFAULT="$SCRIPT_DIR/facet-mainnet-rollup-config.json"
else
    ROLLUP_CONFIG_DEFAULT="$SCRIPT_DIR/facet-sepolia-rollup-config.json"
fi

# Get the block number from the first argument
if [ $# -lt 1 ]; then
    echo "Usage: $0 <block_number> [rollup_config_path] [verbosity]"
    echo ""
    echo "Environment variables:"
    echo "  L1_NETWORK=$L1_NETWORK (mainnet or sepolia)"
    echo "  L1_RPC=$L1_RPC"
    echo "  L1_BEACON_RPC=$L1_BEACON_RPC"  
    echo "  L2_RPC=$L2_RPC"
    echo "  ROLLUP_NODE_RPC=$ROLLUP_NODE_RPC"
    exit 1
fi

BLOCK_NUMBER=$1
# Use network-specific rollup config by default, allow override
ROLLUP_CONFIG_PATH=${2:-"$ROLLUP_CONFIG_DEFAULT"}
VERBOSITY=${3:-''}

# Export for child script
export L1_RPC L1_BEACON_RPC L2_RPC ROLLUP_NODE_RPC

# Pass through to the fixed script with the config path
exec "$SCRIPT_DIR/validate-facet-fixed.sh" "$BLOCK_NUMBER" "$ROLLUP_CONFIG_PATH" "$VERBOSITY"