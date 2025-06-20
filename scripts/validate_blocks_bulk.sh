#!/bin/bash

# Bulk validate blocks in parallel
# Usage: ./scripts/validate_blocks_bulk.sh <start_block> <end_block> [parallel_jobs]

if [ $# -lt 2 ]; then
    echo "Usage: $0 <start_block> <end_block> [parallel_jobs]"
    echo "Example: $0 1000 2000 8"
    echo "Default parallel jobs: 4"
    exit 1
fi

START_BLOCK=$1
END_BLOCK=$2
PARALLEL_JOBS=${3:-4}

echo "🎯 Bulk validating blocks $START_BLOCK to $END_BLOCK"
echo "🔧 Using $PARALLEL_JOBS parallel jobs"

# Create results directory
RESULTS_DIR="bulk_validation_results_$(date +%Y%m%d_%H%M%S)"
mkdir -p "$RESULTS_DIR"

# macOS specific: ensure Cargo build scripts can locate libclang.dylib (required by bindgen / librocksdb-sys)
# We do this once so every subsequent Cargo invocation (build or test) inherits the variables.
if [[ "$OSTYPE" == "darwin"* ]]; then
    # Prefer existing value, otherwise fall back to Homebrew LLVM location
    export LIBCLANG_PATH="${LIBCLANG_PATH:-$(brew --prefix llvm)/lib}"
    # DYLD_LIBRARY_PATH is ignored for many system-signed binaries; DYLD_FALLBACK_LIBRARY_PATH is honoured.
    export DYLD_FALLBACK_LIBRARY_PATH="$LIBCLANG_PATH:${DYLD_FALLBACK_LIBRARY_PATH}"
fi

# Build execution-fixture once
echo "🔨 Building execution-fixture..."
# Build with the environment set above (no extra per-command prefixes needed)
cargo build -p execution-fixture

# Function to validate a single block
validate_block() {
    local block=$1
    local results_dir=$2
    local temp_dir=$(mktemp -d)
    local log_file="$results_dir/block_${block}.log"
    local status_file="$results_dir/block_${block}.status"
    
    echo "Starting block $block" > "$log_file"
    
    # Create fixture from Geth
    ./target/debug/execution-fixture --l2-rpc http://127.0.0.1:8545 --block-number $block --output-dir $temp_dir >> "$log_file" 2>&1
    
    if [ ! -f "$temp_dir/block-${block}.tar.gz" ]; then
        echo "FAILED: Fixture creation" > "$status_file"
        rm -rf $temp_dir
        return 1
    fi
    
    # Run validation test (inherits env so libclang is found)
    FIXTURE_PATH="$temp_dir/block-${block}.tar.gz" cargo test -p kona-executor test_validate_single_fixture -- --nocapture >> "$log_file" 2>&1
    local test_result=$?
    
    # Clean up
    rm -rf $temp_dir
    
    if [ $test_result -eq 0 ]; then
        echo "PASSED" > "$status_file"
        echo "✅ Block $block: PASSED"
    else
        echo "FAILED: Validation" > "$status_file"
        echo "❌ Block $block: FAILED"
    fi
    
    return $test_result
}

export -f validate_block

# Create list of blocks to validate
seq $START_BLOCK $END_BLOCK > "$RESULTS_DIR/blocks_to_validate.txt"

# Run validations in parallel
echo "📊 Starting parallel validation..."
cat "$RESULTS_DIR/blocks_to_validate.txt" | \
    xargs -P $PARALLEL_JOBS -I {} bash -c 'validate_block {} '"$RESULTS_DIR"

# Generate summary report
echo ""
echo "📈 Validation Summary:"
echo "====================="

TOTAL_BLOCKS=$((END_BLOCK - START_BLOCK + 1))
PASSED_BLOCKS=$(grep -l "PASSED" "$RESULTS_DIR"/*.status 2>/dev/null | wc -l | tr -d ' ')
FAILED_BLOCKS=$(grep -l "FAILED" "$RESULTS_DIR"/*.status 2>/dev/null | wc -l | tr -d ' ')

echo "Total blocks: $TOTAL_BLOCKS"
echo "Passed: $PASSED_BLOCKS"
echo "Failed: $FAILED_BLOCKS"
echo ""

# Show failed blocks if any
if [ $FAILED_BLOCKS -gt 0 ]; then
    echo "Failed blocks:"
    for status_file in "$RESULTS_DIR"/*.status; do
        if grep -q "FAILED" "$status_file"; then
            block=$(basename "$status_file" .status | sed 's/block_//')
            failure_reason=$(cat "$status_file")
            echo "  Block $block: $failure_reason"
        fi
    done
fi

echo ""
echo "📁 Detailed logs saved in: $RESULTS_DIR/"
echo ""

# Create CSV report
CSV_FILE="$RESULTS_DIR/validation_report.csv"
echo "block,status,details" > "$CSV_FILE"
for status_file in "$RESULTS_DIR"/*.status; do
    if [ -f "$status_file" ]; then
        block=$(basename "$status_file" .status | sed 's/block_//')
        status=$(cat "$status_file")
        echo "$block,$status" >> "$CSV_FILE"
    fi
done | sort -n

echo "📊 CSV report saved to: $CSV_FILE"

# Exit with failure if any blocks failed
if [ $FAILED_BLOCKS -gt 0 ]; then
    exit 1
fi