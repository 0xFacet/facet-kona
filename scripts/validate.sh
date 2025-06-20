#!/bin/bash

# Main validation entry point
# Automatically selects the best validation approach based on block count

set -euo pipefail

if [ $# -lt 1 ]; then
    cat <<EOF
Kona Block Validation Suite

Usage:
  $0 <block>                    # Validate single block
  $0 <start> <end>              # Validate block range
  $0 <start> <end> [options]    # Validate with options

Examples:
  $0 100                        # Validate block 100
  $0 100 200                    # Validate blocks 100-200
  $0 100 1000 -j 16             # Use 16 parallel jobs
  $0 1000000 2000000 -j 32      # Validate 1M blocks with 32 jobs

For ranges over 100 blocks, the scale validator will be used automatically.

Options:
  -j, --jobs N       Number of parallel jobs
  -r, --resume DIR   Resume from previous run
  -h, --help         Show detailed help for scale validator

EOF
    exit 1
fi

# Determine validation mode
if [ $# -eq 1 ]; then
    # Single block validation
    echo "🎯 Validating single block: $1"
    exec ./scripts/validate_block.sh "$1"
else
    START=$1
    END=$2
    BLOCK_COUNT=$((END - START + 1))
    
    if [ $BLOCK_COUNT -le 0 ]; then
        echo "❌ Error: End block must be greater than start block"
        exit 1
    fi
    
    if [ $BLOCK_COUNT -eq 1 ]; then
        # Single block
        exec ./scripts/validate_block.sh "$START"
    elif [ $BLOCK_COUNT -le 100 ]; then
        # Small range - use simple bulk validator
        echo "📦 Using bulk validator for $BLOCK_COUNT blocks"
        shift 2
        JOBS=4
        
        # Parse -j option if present
        while [[ $# -gt 0 ]]; do
            case $1 in
                -j|--jobs) JOBS="$2"; shift 2 ;;
                *) shift ;;
            esac
        done
        
        exec ./scripts/validate_blocks_bulk.sh "$START" "$END" "$JOBS"
    else
        # Medium to large range - use scale validator
        echo "🚀 Using scale validator for $BLOCK_COUNT blocks"
        shift 2
        exec ./scripts/validate_blocks_scale.sh "$START" "$END" "$@"
    fi
fi