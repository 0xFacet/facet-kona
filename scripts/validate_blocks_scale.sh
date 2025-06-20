#!/bin/bash

# Ultra-scale block validation for 1M+ blocks
# Features: Checkpointing, batching, statistics, ETA, failure analysis
# Usage: ./scripts/validate_blocks_scale.sh <start_block> <end_block> [options]

set -euo pipefail

# Default configuration
PARALLEL_JOBS=16
CHECKPOINT_INTERVAL=1000
STATS_INTERVAL=60  # seconds
MAX_RETRIES=2
FAILURE_THRESHOLD=10  # percent
L2_RPC="http://127.0.0.1:8545"
RESULTS_DIR=""
RESUME=false

usage() {
    cat <<EOF
Ultra-scale block validation for Kona

Usage: $0 <start_block> <end_block> [options]

Options:
  -j, --jobs N              Parallel jobs (default: 16)
  -c, --checkpoint N        Checkpoint every N blocks (default: 1000)
  -s, --stats N             Show stats every N seconds (default: 60)
  -f, --failure-threshold N Stop if failure rate exceeds N% (default: 10)
  -r, --resume DIR          Resume from checkpoint in DIR
  -o, --output DIR          Output directory
  -l, --l2-rpc URL          L2 RPC endpoint (default: http://127.0.0.1:8545)
  -h, --help                Show this help

Examples:
  # Validate 1M blocks with 32 parallel jobs
  $0 1000000 2000000 -j 32

  # Resume a previous run
  $0 1000000 2000000 -r validation_1000000_2000000_20241210_120000

  # Stop if failure rate exceeds 5%
  $0 1000000 2000000 -f 5
EOF
    exit 1
}

# Parse arguments
if [ $# -lt 2 ]; then
    usage
fi

START_BLOCK=$1
END_BLOCK=$2
shift 2

while [[ $# -gt 0 ]]; do
    case $1 in
        -j|--jobs) PARALLEL_JOBS="$2"; shift 2 ;;
        -c|--checkpoint) CHECKPOINT_INTERVAL="$2"; shift 2 ;;
        -s|--stats) STATS_INTERVAL="$2"; shift 2 ;;
        -f|--failure-threshold) FAILURE_THRESHOLD="$2"; shift 2 ;;
        -r|--resume) RESUME=true; RESULTS_DIR="$2"; shift 2 ;;
        -o|--output) RESULTS_DIR="$2"; shift 2 ;;
        -l|--l2-rpc) L2_RPC="$2"; shift 2 ;;
        -h|--help) usage ;;
        *) echo "Unknown option: $1"; usage ;;
    esac
done

# Setup directories
if [ -z "$RESULTS_DIR" ]; then
    RESULTS_DIR="validation_${START_BLOCK}_${END_BLOCK}_$(date +%Y%m%d_%H%M%S)"
fi

mkdir -p "$RESULTS_DIR"/{logs,status,checkpoints,stats}

# State files
STATE_DIR="$RESULTS_DIR/state"
mkdir -p "$STATE_DIR"
QUEUE_FILE="$STATE_DIR/queue.txt"
COMPLETED_FILE="$STATE_DIR/completed.txt"
FAILED_FILE="$STATE_DIR/failed.txt"
STATS_FILE="$STATE_DIR/stats.json"
LOCK_DIR="$STATE_DIR/locks"
mkdir -p "$LOCK_DIR"

# Initialize or resume
if [ "$RESUME" = true ] && [ -f "$COMPLETED_FILE" ]; then
    echo "📂 Resuming from checkpoint..."
    COMPLETED_COUNT=$(wc -l < "$COMPLETED_FILE" | tr -d ' ')
    FAILED_COUNT=$(wc -l < "$FAILED_FILE" 2>/dev/null | tr -d ' ' || echo 0)
    echo "✅ Already completed: $COMPLETED_COUNT"
    echo "❌ Already failed: $FAILED_COUNT"
    
    # Rebuild queue excluding completed blocks
    comm -23 <(seq $START_BLOCK $END_BLOCK | sort) <(sort "$COMPLETED_FILE") > "$QUEUE_FILE"
else
    # Fresh start
    seq $START_BLOCK $END_BLOCK > "$QUEUE_FILE"
    > "$COMPLETED_FILE"
    > "$FAILED_FILE"
    echo '{"start_time":"'$(date -u +%Y-%m-%dT%H:%M:%SZ)'","blocks_total":'$((END_BLOCK - START_BLOCK + 1))'}' > "$STATS_FILE"
fi

TOTAL_BLOCKS=$((END_BLOCK - START_BLOCK + 1))
REMAINING_BLOCKS=$(wc -l < "$QUEUE_FILE" | tr -d ' ')

cat <<EOF

🚀 Ultra-Scale Block Validation
================================
Range: $START_BLOCK - $END_BLOCK
Total blocks: $TOTAL_BLOCKS
Remaining: $REMAINING_BLOCKS
Parallel jobs: $PARALLEL_JOBS
Checkpoint interval: $CHECKPOINT_INTERVAL blocks
Results: $RESULTS_DIR

EOF

# Build tools
echo "🔨 Building execution-fixture (release mode)..."
# Set LIBCLANG_PATH for macOS to avoid RocksDB build issues
if [[ "$OSTYPE" == "darwin"* ]]; then
    LIBCLANG_PATH=${LIBCLANG_PATH:-/Library/Developer/CommandLineTools/usr/lib} cargo build -p execution-fixture --release
else
    cargo build -p execution-fixture --release
fi
EXECUTION_FIXTURE="./target/release/execution-fixture"

echo "🔨 Building test suite (release mode)..."
if [[ "$OSTYPE" == "darwin"* ]]; then
    LIBCLANG_PATH=${LIBCLANG_PATH:-/Library/Developer/CommandLineTools/usr/lib} cargo build -p kona-executor --release --tests
else
    cargo build -p kona-executor --release --tests
fi

# Statistics tracking
start_stats_monitor() {
    while true; do
        sleep "$STATS_INTERVAL"
        
        local completed=$(wc -l < "$COMPLETED_FILE" 2>/dev/null | tr -d ' ' || echo 0)
        local failed=$(wc -l < "$FAILED_FILE" 2>/dev/null | tr -d ' ' || echo 0)
        local processed=$((completed + failed))
        
        if [ $processed -gt 0 ]; then
            local elapsed=$(($(date +%s) - START_TIME))
            local rate=$(echo "scale=2; $processed / ($elapsed / 60.0)" | bc)
            local eta_minutes=$(echo "($TOTAL_BLOCKS - $processed) / $rate" | bc)
            local success_rate=$(echo "scale=2; ($completed * 100.0) / $processed" | bc)
            
            # Clear previous line and print update
            printf "\r\033[K"
            echo "📊 Progress Update ($(date +%H:%M:%S))"
            echo "  Processed: $processed / $TOTAL_BLOCKS ($(echo "scale=1; ($processed * 100.0) / $TOTAL_BLOCKS" | bc)%)"
            echo "  Success rate: $success_rate%"
            echo "  Speed: $rate blocks/min"
            if [[ "$OSTYPE" == "darwin"* ]]; then
                echo "  ETA: $(date -v +${eta_minutes}M +%H:%M:%S)"
            else
                echo "  ETA: $(date -d "+$eta_minutes minutes" +%H:%M:%S)"
            fi
            
            # Check failure threshold
            if (( $(echo "$failed * 100 / $processed > $FAILURE_THRESHOLD" | bc) )); then
                echo ""
                echo "⚠️  STOPPING: Failure rate ($(echo "scale=1; ($failed * 100.0) / $processed" | bc)%) exceeds threshold ($FAILURE_THRESHOLD%)"
                kill -TERM $$
            fi
            
            # Save stats
            cat > "$RESULTS_DIR/stats/stats_$(date +%s).json" <<STATEOF
{
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "processed": $processed,
  "completed": $completed,
  "failed": $failed,
  "remaining": $((TOTAL_BLOCKS - processed)),
  "rate_per_min": $rate,
  "success_rate": $success_rate,
  "eta_minutes": $eta_minutes
}
STATEOF
        fi
    done
}

# Checkpoint saving
save_checkpoint() {
    local checkpoint_name="checkpoint_$(date +%Y%m%d_%H%M%S)"
    local checkpoint_dir="$RESULTS_DIR/checkpoints/$checkpoint_name"
    
    mkdir -p "$checkpoint_dir"
    cp "$COMPLETED_FILE" "$checkpoint_dir/"
    cp "$FAILED_FILE" "$checkpoint_dir/" 2>/dev/null || true
    
    local completed=$(wc -l < "$COMPLETED_FILE" | tr -d ' ')
    echo "💾 Checkpoint saved: $completed blocks completed"
}

# Worker function
validate_block_scaled() {
    local block=$1
    local temp_dir=$(mktemp -d)
    local log_file="$RESULTS_DIR/logs/block_${block}.log"
    local status_file="$RESULTS_DIR/status/block_${block}.status"
    local lock_file="$LOCK_DIR/block_${block}.lock"
    
    # Atomic lock to prevent duplicate processing
    if ! mkdir "$lock_file" 2>/dev/null; then
        return 0
    fi
    
    # Skip if already processed
    if grep -q "^${block}$" "$COMPLETED_FILE" "$FAILED_FILE" 2>/dev/null; then
        rmdir "$lock_file"
        return 0
    fi
    
    local retry_count=0
    local success=false
    
    while [ $retry_count -lt $MAX_RETRIES ] && [ "$success" = false ]; do
        if $EXECUTION_FIXTURE --l2-rpc "$L2_RPC" --block-number "$block" --output-dir "$temp_dir" > "$log_file" 2>&1; then
            if [ -f "$temp_dir/block-${block}.tar.gz" ]; then
                if FIXTURE_PATH="$temp_dir/block-${block}.tar.gz" \
                   timeout 300 cargo test -p kona-executor test_validate_single_fixture --release -- --nocapture >> "$log_file" 2>&1; then
                    echo "PASSED" > "$status_file"
                    echo "$block" >> "$COMPLETED_FILE"
                    success=true
                fi
            fi
        fi
        
        if [ "$success" = false ]; then
            retry_count=$((retry_count + 1))
            if [ $retry_count -lt $MAX_RETRIES ]; then
                sleep 1
                echo "Retry $retry_count/$MAX_RETRIES" >> "$log_file"
            fi
        fi
    done
    
    if [ "$success" = false ]; then
        echo "FAILED after $MAX_RETRIES attempts" > "$status_file"
        echo "$block" >> "$FAILED_FILE"
        
        # Print error immediately
        echo ""
        echo "❌ ERROR: Block $block failed validation"
        echo "   Log: $log_file"
        tail -n 5 "$log_file" | sed 's/^/   > /'
        echo ""
    fi
    
    # Cleanup
    rm -rf "$temp_dir"
    rmdir "$lock_file"
    
    # Checkpoint if needed
    local processed_count=$(wc -l < "$COMPLETED_FILE" | tr -d ' ')
    if [ $((processed_count % CHECKPOINT_INTERVAL)) -eq 0 ]; then
        save_checkpoint
    fi
}

export -f validate_block_scaled save_checkpoint
export RESULTS_DIR L2_RPC EXECUTION_FIXTURE MAX_RETRIES
export COMPLETED_FILE FAILED_FILE LOCK_DIR CHECKPOINT_INTERVAL

# Start monitoring
START_TIME=$(date +%s)
start_stats_monitor &
MONITOR_PID=$!

# Trap to cleanup monitor on exit
trap "kill $MONITOR_PID 2>/dev/null || true" EXIT

# Run validation
echo "🏃 Starting validation..."
echo ""

if command -v parallel &> /dev/null; then
    # Use GNU parallel without progress bar to avoid output conflicts
    parallel -j $PARALLEL_JOBS \
        --joblog "$RESULTS_DIR/parallel.log" \
        --resume-failed \
        validate_block_scaled :::: "$QUEUE_FILE"
else
    # Fallback to xargs
    cat "$QUEUE_FILE" | \
        xargs -P $PARALLEL_JOBS -I {} bash -c 'validate_block_scaled "$@"' _ {}
fi

# Kill monitor
kill $MONITOR_PID 2>/dev/null || true

# Final checkpoint
save_checkpoint

# Final report
echo ""
echo ""
echo "🏁 Validation Complete"
echo "===================="

COMPLETED_COUNT=$(wc -l < "$COMPLETED_FILE" | tr -d ' ')
FAILED_COUNT=$(wc -l < "$FAILED_FILE" 2>/dev/null | tr -d ' ' || echo 0)
DURATION=$(($(date +%s) - START_TIME))

cat <<EOF
Total blocks: $TOTAL_BLOCKS
Completed: $COMPLETED_COUNT
Failed: $FAILED_COUNT
Success rate: $(echo "scale=2; ($COMPLETED_COUNT * 100.0) / ($COMPLETED_COUNT + $FAILED_COUNT)" | bc)%
Duration: $((DURATION / 3600))h $((DURATION % 3600 / 60))m $((DURATION % 60))s
Average speed: $(echo "scale=2; ($COMPLETED_COUNT + $FAILED_COUNT) / ($DURATION / 60.0)" | bc) blocks/min

Results saved to: $RESULTS_DIR
EOF

# Generate final report
REPORT_FILE="$RESULTS_DIR/final_report.json"
cat > "$REPORT_FILE" <<EOF
{
  "start_block": $START_BLOCK,
  "end_block": $END_BLOCK,
  "total_blocks": $TOTAL_BLOCKS,
  "completed": $COMPLETED_COUNT,
  "failed": $FAILED_COUNT,
  "success_rate": $(echo "scale=2; ($COMPLETED_COUNT * 100.0) / ($COMPLETED_COUNT + $FAILED_COUNT)" | bc),
  "duration_seconds": $DURATION,
  "average_blocks_per_minute": $(echo "scale=2; ($COMPLETED_COUNT + $FAILED_COUNT) / ($DURATION / 60.0)" | bc),
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
}
EOF

# Analyze failures if any
if [ $FAILED_COUNT -gt 0 ]; then
    echo ""
    echo "📊 Failure Analysis:"
    echo ""
    
    # Group failures by error type
    echo "Error types:"
    grep -h "FAILED" "$RESULTS_DIR"/status/*.status | sort | uniq -c | sort -rn | head -10
    
    echo ""
    echo "Failed blocks saved to: $FAILED_FILE"
    echo ""
    echo "To retry failed blocks:"
    echo "  cat $FAILED_FILE | xargs -I {} ./scripts/validate_block.sh {}"
fi