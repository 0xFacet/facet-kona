#!/bin/bash
set -e

# Parse command line arguments
START_BLOCK=${1:-2}
END_BLOCK=${2:-10}
WORKERS=${3:-4}
OUTPUT_DIR=${4:-"validation_results_$(date +%Y%m%d_%H%M%S)"}

# Create output directory
mkdir -p "$OUTPUT_DIR"
mkdir -p "$OUTPUT_DIR/logs"

echo "🚀 Facet Bulk Validation Tool"
echo "Range: $START_BLOCK - $END_BLOCK"
echo "Workers: $WORKERS"
echo "Output: $OUTPUT_DIR"
echo ""

# Create a file to track results
RESULTS_FILE="$OUTPUT_DIR/results.txt"
echo "Block,Status,Duration,Timestamp" > "$RESULTS_FILE"

# Function to validate a single block
validate_block() {
    local block=$1
    local log_file="$OUTPUT_DIR/logs/block_$block.log"
    local start_time=$(date +%s)
    
    echo "Validating block $block..."
    
    # Run validation and capture output
    if ./bin/validate-facet/validate-facet-fixed.sh "$block" > "$log_file" 2>&1; then
        local status="SUCCESS"
        echo "✅ Block $block: SUCCESS"
    else
        local status="FAILED"
        echo "❌ Block $block: FAILED"
        # Show last few lines of error
        tail -n 5 "$log_file" | sed 's/^/   /'
    fi
    
    local end_time=$(date +%s)
    local duration=$((end_time - start_time))
    local timestamp=$(date -Iseconds)
    
    # Record result (with file locking to prevent corruption)
    {
        flock -x 200
        echo "$block,$status,$duration,$timestamp" >> "$RESULTS_FILE"
    } 200>"$RESULTS_FILE.lock"
    
    return 0
}

export -f validate_block
export OUTPUT_DIR
export RESULTS_FILE

# Build kona-host first
echo "🔨 Building kona-host..."
cargo build --bin kona-host --release

# Run validations in parallel using GNU parallel or xargs
echo ""
echo "Starting validation..."
echo ""

if command -v parallel &> /dev/null; then
    # Use GNU parallel if available
    seq "$START_BLOCK" "$END_BLOCK" | parallel -j "$WORKERS" validate_block {}
else
    # Fall back to xargs
    seq "$START_BLOCK" "$END_BLOCK" | xargs -P "$WORKERS" -I {} bash -c 'validate_block "$@"' _ {}
fi

# Generate summary report
echo ""
echo "🏁 Validation Complete"
echo "===================="

# Count results
TOTAL=$((END_BLOCK - START_BLOCK + 1))
SUCCESS=$(grep -c ",SUCCESS," "$RESULTS_FILE" || true)
FAILED=$(grep -c ",FAILED," "$RESULTS_FILE" || true)
SUCCESS_RATE=$(awk "BEGIN {printf \"%.2f\", ($SUCCESS/$TOTAL)*100}")

echo "Total blocks: $TOTAL"
echo "Successful: $SUCCESS"
echo "Failed: $FAILED"
echo "Success rate: $SUCCESS_RATE%"
echo ""
echo "Results saved to: $OUTPUT_DIR"

# Create a JSON summary
cat > "$OUTPUT_DIR/summary.json" <<EOF
{
  "start_block": $START_BLOCK,
  "end_block": $END_BLOCK,
  "total_blocks": $TOTAL,
  "successful": $SUCCESS,
  "failed": $FAILED,
  "success_rate": $SUCCESS_RATE,
  "timestamp": "$(date -Iseconds)",
  "results_file": "$RESULTS_FILE",
  "log_directory": "$OUTPUT_DIR/logs"
}
EOF

# List failed blocks if any
if [ "$FAILED" -gt 0 ]; then
    echo ""
    echo "Failed blocks:"
    grep ",FAILED," "$RESULTS_FILE" | cut -d',' -f1 | while read -r block; do
        echo "  - Block $block"
    done
fi