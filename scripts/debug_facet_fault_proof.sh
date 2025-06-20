#!/usr/bin/env bash
set -euo pipefail

# Debug script for Facet fault proofs
# This script helps debug why fault proofs are failing for Facet

# RPC endpoints (must be set via environment variables)
L1_RPC="${L1_RPC:?Error: L1_RPC environment variable not set}"
L1_BEACON_RPC="${L1_BEACON_RPC:?Error: L1_BEACON_RPC environment variable not set}"
L2_RPC="${L2_RPC:?Error: L2_RPC environment variable not set}"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

echo -e "${YELLOW}Facet Fault Proof Debugger${NC}"
echo "================================"

# Get the block number from the first argument
BLOCK_NUMBER=${1:-2}
echo -e "Debugging block: ${GREEN}$BLOCK_NUMBER${NC}"

# First, let's check the rollup node status
echo -e "\n${YELLOW}1. Checking Rollup Node Status${NC}"
if command -v cast &> /dev/null; then
    # Get chain ID
    CHAIN_ID=$(cast chain-id --rpc-url $L2_RPC)
    echo "L2 Chain ID: $CHAIN_ID (hex: $(printf '0x%x' $CHAIN_ID))"
    
    # Get latest block
    LATEST_BLOCK=$(cast block-number --rpc-url $L2_RPC)
    echo "Latest L2 block: $LATEST_BLOCK"
    
    # Check if we have a local rollup node running
    if curl -s http://localhost:9545 > /dev/null 2>&1; then
        echo -e "${GREEN}Local rollup node is running${NC}"
        
        # Try to get output at block
        echo -e "\n${YELLOW}2. Checking Output Roots${NC}"
        
        # Get output for block 1
        echo "Output root for block 1:"
        cast rpc --rpc-url http://localhost:9545 "optimism_outputAtBlock" $(cast 2h 1) | jq '.' || echo -e "${RED}Failed to get output root for block 1${NC}"
        
        # Get output for target block
        echo -e "\nOutput root for block $BLOCK_NUMBER:"
        cast rpc --rpc-url http://localhost:9545 "optimism_outputAtBlock" $(cast 2h $BLOCK_NUMBER) | jq '.' || echo -e "${RED}Failed to get output root for block $BLOCK_NUMBER${NC}"
    else
        echo -e "${RED}No local rollup node found at localhost:9545${NC}"
        echo "To run a Facet rollup node, you'll need to set one up"
    fi
    
    # Check predeploy contracts
    echo -e "\n${YELLOW}3. Checking Predeploy Contracts${NC}"
    
    # L2ToL1MessagePasser (0x4200000000000000000000000000000000000016)
    echo "L2ToL1MessagePasser contract:"
    MESSAGE_PASSER="0x4200000000000000000000000000000000000016"
    CODE=$(cast code $MESSAGE_PASSER --rpc-url $L2_RPC --block 1)
    if [ "$CODE" = "0x" ]; then
        echo -e "${RED}WARNING: L2ToL1MessagePasser has no code at block 1!${NC}"
    else
        echo -e "${GREEN}L2ToL1MessagePasser has code: ${CODE:0:10}...${NC}"
    fi
    
    # Check storage root (this is harder without direct access)
    echo "Checking if contract has storage..."
    # Try to read the first storage slot
    STORAGE=$(cast storage $MESSAGE_PASSER 0x0 --rpc-url $L2_RPC --block 1)
    echo "Storage slot 0: $STORAGE"
    
    # Check block details
    echo -e "\n${YELLOW}4. Block Details${NC}"
    echo "Block 0 (Genesis):"
    cast block 0 --rpc-url $L2_RPC --json | jq '{number, hash, stateRoot, timestamp}'
    
    echo -e "\nBlock 1:"
    cast block 1 --rpc-url $L2_RPC --json | jq '{number, hash, stateRoot, timestamp, transactions}'
    
    if [ "$BLOCK_NUMBER" != "1" ]; then
        echo -e "\nBlock $BLOCK_NUMBER:"
        cast block $BLOCK_NUMBER --rpc-url $L2_RPC --json | jq '{number, hash, stateRoot, timestamp}'
    fi
else
    echo -e "${RED}cast command not found. Please install Foundry.${NC}"
fi

echo -e "\n${YELLOW}5. Recommendations${NC}"
echo "1. Ensure you have a Facet rollup node running locally at localhost:9545"
echo "2. The rollup node must support the optimism_outputAtBlock RPC method"
echo "3. Check that the L2ToL1MessagePasser predeploy is properly initialized"
echo "4. Verify the rollup config has the correct genesis configuration"

echo -e "\n${YELLOW}6. Next Steps${NC}"
echo "To run the fault proof with verbose logging:"
echo "RUST_LOG=info,client=debug,host=debug ./scripts/run_facet_fault_proof.sh $BLOCK_NUMBER"