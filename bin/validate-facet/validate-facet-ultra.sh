#!/usr/bin/env bash
set -euo pipefail

# Ultra performance mode
export RUST_LOG=error
export RUST_BACKTRACE=0

# Cache rollup config in memory
if [ -z "${ROLLUP_CONFIG_CACHED:-}" ]; then
    SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
    export ROLLUP_CONFIG_CACHED="$SCRIPT_DIR/facet-rollup-config.json"
    export L2_CHAIN_ID_CACHED=$(jq -r '.l2_chain_id' < "$ROLLUP_CONFIG_CACHED")
fi

BLOCK_NUMBER=$1

# RPC endpoints (must be set via environment variables)
L1_RPC="${L1_RPC:?Error: L1_RPC environment variable not set}"
L1_BEACON_RPC="${L1_BEACON_RPC:?Error: L1_BEACON_RPC environment variable not set}"
L2_RPC="${L2_RPC:?Error: L2_RPC environment variable not set}"
ROLLUP_NODE_RPC="${ROLLUP_NODE_RPC:?Error: ROLLUP_NODE_RPC environment variable not set}"

# Binary path from environment or default
KONA_BIN="${KONA_HOST_BIN:-./target/release/kona-host}"

# Data directory
DATA_DIR="${DATA_DIR:-/tmp/kona_$$_${BLOCK_NUMBER}}"
trap "rm -rf $DATA_DIR" EXIT

# Batch ALL RPC calls into one
PREV_BLOCK=$((BLOCK_NUMBER - 1))
BATCH='[
  {"jsonrpc":"2.0","id":1,"method":"optimism_outputAtBlock","params":["'$(printf "0x%x" $BLOCK_NUMBER)'"]},
  {"jsonrpc":"2.0","id":2,"method":"optimism_outputAtBlock","params":["'$(printf "0x%x" $PREV_BLOCK)'"]},
  {"jsonrpc":"2.0","id":3,"method":"eth_getBlockByNumber","params":["'$(printf "0x%x" $PREV_BLOCK)'",false]}
]'

# Single batch call
RESPONSE=$(curl -s --compressed -X POST "$ROLLUP_NODE_RPC" \
  -H "Content-Type: application/json" \
  -H "Connection: keep-alive" \
  -d "$BATCH")

# Parse all values at once
eval $(echo "$RESPONSE" | jq -r '
  def find_by_id(id): map(select(.id == id)) | .[0];
  "CLAIMED_L2_OUTPUT_ROOT=" + (find_by_id(1).result.outputRoot // "error") + "\n" +
  "L1_ORIGIN_NUM=" + (find_by_id(2).result.blockRef.l1origin.number | tostring) + "\n" +
  "AGREED_L2_OUTPUT_ROOT=" + (find_by_id(2).result.outputRoot // "error") + "\n" +
  "AGREED_L2_HEAD_HASH=" + (find_by_id(3).result.hash // "error")
')

# Get L1 head
L1_TARGET=$((L1_ORIGIN_NUM + 30))
L1_HEAD=$(curl -s --compressed -X POST "$L1_RPC" \
  -H "Content-Type: application/json" \
  -H "Connection: keep-alive" \
  -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"eth_getBlockByNumber\",\"params\":[\"$(printf "0x%x" $L1_TARGET)\",false]}" \
  | jq -r .result.hash)

# Direct execution, no output unless error
exec "$KONA_BIN" -vv single \
  --l1-head "$L1_HEAD" \
  --agreed-l2-head-hash "$AGREED_L2_HEAD_HASH" \
  --claimed-l2-output-root "$CLAIMED_L2_OUTPUT_ROOT" \
  --agreed-l2-output-root "$AGREED_L2_OUTPUT_ROOT" \
  --claimed-l2-block-number "$BLOCK_NUMBER" \
  --rollup-config-path "$ROLLUP_CONFIG_CACHED" \
  --l1-node-address "$L1_RPC" \
  --l1-beacon-address "$L1_BEACON_RPC" \
  --l2-node-address "$L2_RPC" \
  --native \
  --data-dir "$DATA_DIR" 2>&1