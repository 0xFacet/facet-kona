#!/bin/bash

# Validate a block by creating a fixture from Geth and testing it
# Usage: ./scripts/validate_block.sh <block_number>

if [ $# -eq 0 ]; then
    echo "Usage: $0 <block_number>"
    echo "Example: $0 6"
    exit 1
fi

BLOCK=$1
TEMP_DIR=$(mktemp -d)
echo "🎯 Validating block $BLOCK"
echo "📂 Using temp directory: $TEMP_DIR"

# macOS specific setup so Cargo build scripts can locate libclang.dylib (required by bindgen / librocksdb-sys)
if [[ "$OSTYPE" == "darwin"* ]]; then
    export LIBCLANG_PATH="${LIBCLANG_PATH:-$(brew --prefix llvm)/lib}"
    export DYLD_FALLBACK_LIBRARY_PATH="$LIBCLANG_PATH:${DYLD_FALLBACK_LIBRARY_PATH}"
fi

# Build execution-fixture if needed (inherits env above)
echo "🔨 Building execution-fixture..."
cargo build -p execution-fixture

# Create fixture from Geth
echo "📦 Creating fixture from Geth..."
./target/debug/execution-fixture --l2-rpc http://127.0.0.1:8545 --block-number $BLOCK --output-dir $TEMP_DIR

# Check if fixture was created
if [ ! -f "$TEMP_DIR/block-${BLOCK}.tar.gz" ]; then
    echo "❌ Failed to create fixture"
    rm -rf $TEMP_DIR
    exit 1
fi

# Run validation test
echo "✅ Running validation test..."
FIXTURE_PATH="$TEMP_DIR/block-${BLOCK}.tar.gz" cargo test -p kona-executor test_validate_single_fixture -- --nocapture

# Capture test result
TEST_RESULT=$?

# Clean up
echo "🧹 Cleaning up..."
rm -rf $TEMP_DIR

if [ $TEST_RESULT -eq 0 ]; then
    echo "✅ Block $BLOCK validation successful!"
else
    echo "❌ Block $BLOCK validation failed!"
fi

exit $TEST_RESULT