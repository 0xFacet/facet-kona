#!/bin/bash
# Facet validation tool - validates both execution and derivation

set -e

# Colors for output
GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Check for required environment variables
if [ -z "${L1_RPC:-}" ] || [ -z "${L2_RPC:-}" ]; then
    echo -e "${RED}Error: Required environment variables not set${NC}"
    echo "Please set the following environment variables:"
    echo "  export L1_RPC=\"your_l1_rpc_url\""
    echo "  export L2_RPC=\"your_l2_rpc_url\""
    echo ""
    echo "Or use direnv with a .envrc file"
    exit 1
fi

# Default values
DEFAULT_JOBS=16
DEFAULT_SAMPLE_RATE=1

# Help function
show_help() {
    echo "Usage: $0 [OPTIONS] START_BLOCK END_BLOCK"
    echo ""
    echo "Validates Facet blocks by running both execution and derivation tests"
    echo ""
    echo "Arguments:"
    echo "  START_BLOCK        Starting block number"
    echo "  END_BLOCK          Ending block number (inclusive)"
    echo ""
    echo "Options:"
    echo "  -j, --jobs NUM     Number of parallel workers (default: $DEFAULT_JOBS)"
    echo "  --l1-rpc URL       L1 RPC endpoint (env: L1_RPC)"
    echo "  --l2-rpc URL       L2 RPC endpoint (env: L2_RPC)"
    echo "  -o, --output DIR   Output directory for results"
    echo "  --skip-execution   Skip execution validation"
    echo "  --skip-derivation  Skip derivation validation"
    echo "  --sample-rate NUM  Sample rate for derivation (default: $DEFAULT_SAMPLE_RATE)"
    echo "  -r, --resume DIR   Resume from a previous run"
    echo "  --random NUM       Test NUM random blocks from the range"
    echo "  --seed NUM         Random seed for reproducibility (default: 42)"
    echo "  -v, --verbose      Verbose output"
    echo "  -h, --help         Show this help message"
    echo ""
    echo "Examples:"
    echo "  # Validate blocks 6-10 with default settings"
    echo "  $0 6 10"
    echo ""
    echo "  # Validate blocks 100-200 with custom RPC and 32 workers"
    echo "  $0 --l2-rpc http://localhost:8545 -j 32 100 200"
    echo ""
    echo "  # Resume a previous validation run"
    echo "  $0 -r validation_100_200_20250101_120000 100 200"
    echo ""
    echo "  # Test 1000 random blocks from range 0-1000000"
    echo "  $0 --random 1000 0 1000000"
}

# Parse command line arguments
ARGS=()
while [[ $# -gt 0 ]]; do
    case $1 in
        -h|--help)
            show_help
            exit 0
            ;;
        -j|--jobs)
            JOBS="$2"
            shift 2
            ;;
        --l1-rpc)
            L1_RPC="$2"
            shift 2
            ;;
        --l2-rpc)
            L2_RPC="$2"
            shift 2
            ;;
        -o|--output)
            OUTPUT_DIR="$2"
            shift 2
            ;;
        --skip-execution)
            SKIP_EXECUTION="--skip-execution"
            shift
            ;;
        --skip-derivation)
            SKIP_DERIVATION="--skip-derivation"
            shift
            ;;
        --sample-rate)
            SAMPLE_RATE="$2"
            shift 2
            ;;
        -r|--resume)
            RESUME_DIR="$2"
            shift 2
            ;;
        -v|--verbose)
            VERBOSE="--verbose"
            shift
            ;;
        --random)
            RANDOM_SAMPLE="$2"
            shift 2
            ;;
        --seed)
            RANDOM_SEED="$2"
            shift 2
            ;;
        *)
            ARGS+=("$1")
            shift
            ;;
    esac
done

# Check for required arguments
if [ ${#ARGS[@]} -ne 2 ]; then
    echo -e "${RED}Error: START_BLOCK and END_BLOCK are required${NC}"
    echo ""
    show_help
    exit 1
fi

START_BLOCK="${ARGS[0]}"
END_BLOCK="${ARGS[1]}"

# Set defaults
JOBS="${JOBS:-$DEFAULT_JOBS}"
# L1_RPC and L2_RPC are already checked above
SAMPLE_RATE="${SAMPLE_RATE:-$DEFAULT_SAMPLE_RATE}"

# Build the validate-facet binary if needed
BINARY="./target/release/validate-facet"

# Check if source files are newer than binary
SOURCE_DIR="./bin/validate-facet/src"
NEEDS_BUILD=false

if [ ! -f "$BINARY" ]; then
    NEEDS_BUILD=true
    echo -e "${YELLOW}Binary not found, building...${NC}"
elif [ -n "$(find $SOURCE_DIR -name '*.rs' -newer $BINARY 2>/dev/null)" ]; then
    NEEDS_BUILD=true
    echo -e "${YELLOW}Source files changed, rebuilding...${NC}"
fi

if [ "$NEEDS_BUILD" = true ]; then
    echo -e "${YELLOW}Building validate-facet binary...${NC}"
    cargo build -p validate-facet --release
fi

# Construct the command
CMD="$BINARY"
CMD="$CMD --start-block $START_BLOCK"
CMD="$CMD --end-block $END_BLOCK"
CMD="$CMD --jobs $JOBS"
CMD="$CMD --l1-rpc $L1_RPC"
CMD="$CMD --l2-rpc $L2_RPC"
CMD="$CMD --derivation-sample-rate $SAMPLE_RATE"

[ -n "$OUTPUT_DIR" ] && CMD="$CMD --output-dir $OUTPUT_DIR"
[ -n "$SKIP_EXECUTION" ] && CMD="$CMD $SKIP_EXECUTION"
[ -n "$SKIP_DERIVATION" ] && CMD="$CMD $SKIP_DERIVATION"
[ -n "$RESUME_DIR" ] && CMD="$CMD --resume $RESUME_DIR"
[ -n "$VERBOSE" ] && CMD="$CMD $VERBOSE"
[ -n "$RANDOM_SAMPLE" ] && CMD="$CMD --random-sample $RANDOM_SAMPLE"
[ -n "$RANDOM_SEED" ] && CMD="$CMD --random-seed $RANDOM_SEED"

# Run the validation
echo -e "${GREEN}Starting Facet validation${NC}"
echo "Range: $START_BLOCK - $END_BLOCK"
echo "Workers: $JOBS"
echo "L1 RPC: $L1_RPC"
echo "L2 RPC: $L2_RPC"
echo ""

exec $CMD