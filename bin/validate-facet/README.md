# validate-facet

A high-performance validation tool for Facet blockchain that validates both execution and derivation of L2 blocks.

## Features

- **Dual Validation**: Tests both execution (state transitions) and derivation (L1→L2 transaction derivation)
- **Parallel Processing**: Configurable number of workers for high-speed validation
- **Resume Support**: Can resume from checkpoints after interruptions
- **Progress Tracking**: Real-time progress bars and statistics
- **Flexible Sampling**: Configurable sampling rate for derivation tests
- **Detailed Logging**: Comprehensive logs for debugging failures

## Usage

### Command Line

```bash
# Basic usage
./target/release/validate-facet -s 6 -e 100

# With custom settings
./target/release/validate-facet \
  --start-block 1000 \
  --end-block 2000 \
  --jobs 32 \
  --l2-rpc http://localhost:8545 \
  --derivation-sample-rate 10
```

### Shell Script Wrapper

```bash
# Using the convenience wrapper
./scripts/validate_facet.sh 6 100

# Skip execution tests (derivation only)
./scripts/validate_facet.sh --skip-execution 6 100

# Resume from previous run
./scripts/validate_facet.sh -r validation_6_100_20250618_120000 6 100
```

## Options

- `-s, --start-block`: Starting block number
- `-e, --end-block`: Ending block number (inclusive)
- `-j, --jobs`: Number of parallel workers (default: 16)
- `--l1-rpc`: L1 RPC endpoint (default: https://ethereum-rpc.publicnode.com)
- `--l2-rpc`: L2 RPC endpoint (default: https://mainnet.facet.org)
- `-o, --output-dir`: Custom output directory
- `--skip-execution`: Skip execution validation
- `--skip-derivation`: Skip derivation validation
- `--derivation-sample-rate`: Test every Nth block for derivation (default: 1)
- `-r, --resume`: Resume from a checkpoint directory
- `--max-retries`: Maximum retries per block (default: 2)
- `--failure-threshold`: Stop if failure rate exceeds percentage (default: 10.0)
- `-v, --verbose`: Enable debug logging

## Output

Results are saved to a timestamped directory containing:
- `results.jsonl`: Detailed results for each block
- `checkpoint.json`: Progress checkpoint for resuming
- `final_report.json`: Summary statistics
- `logs/`: Individual log files for failed blocks

## Architecture

The tool consists of:
- **main.rs**: Orchestration and parallel task management
- **execution.rs**: Execution validation using execution-fixture
- **derivation.rs**: Derivation validation using kona-derive
- **types.rs**: Shared data structures

## Building

```bash
cargo build -p validate-facet --release
```