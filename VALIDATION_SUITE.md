# Kona Block Validation Suite

This suite provides tools for validating blocks at scale, ensuring Kona + REVM produces identical results to op-geth.

## Quick Start

```bash
# Validate a single block
./scripts/validate.sh 6

# Validate a range of blocks
./scripts/validate.sh 1000 2000

# Validate with custom parallelism
./scripts/validate.sh 1000 10000 -j 32

# Resume a previous validation run
./scripts/validate.sh 1000000 2000000 -r validation_1000000_2000000_20241210_120000
```

## Architecture

The validation suite consists of:

1. **`validate.sh`** - Main entry point that automatically selects the appropriate validator
2. **`validate_block.sh`** - Single block validation
3. **`validate_blocks_bulk.sh`** - Simple parallel validation for small ranges (≤100 blocks)
4. **`validate_blocks_scale.sh`** - Enterprise-grade validation for large ranges with:
   - Checkpointing and resume capability
   - Real-time progress monitoring and ETA
   - Automatic failure threshold detection
   - Retry logic for transient failures
   - Detailed failure analysis

## How It Works

1. **Fixture Creation**: Uses `execution-fixture` to create a complete state fixture from Geth
2. **Validation**: Runs the fixture through Kona's execution engine
3. **Comparison**: Verifies the resulting state root matches Geth's state root

## Performance

- Process-based parallelism for complete isolation
- Release mode builds for maximum performance
- Typical throughput: 100-500 blocks/minute depending on hardware
- For 1M blocks with 32 parallel jobs: ~33 hours

## Implementation Details

- FCT (Facet Compute Token) support with custom rollup config
- Handles deposit transactions with mint values
- Proper gas fee deduction from minted amounts
- Support for custom chains not in the registry

## REVM Dependencies

This project uses a custom fork of REVM with FCT support:

- **Default**: Uses `0xFacet/facet-revm` branch `facet-initial-changes` from GitHub
- **Local Development**: Switch to local REVM with `./scripts/switch-revm.sh local`
- **Switch Back**: Return to GitHub deps with `./scripts/switch-revm.sh github`

The local development path expects REVM to be at `../revm` relative to this project.

## Testing

The validation suite has been tested with:
- Single block validation (block 6)
- Range validation (blocks 5-7, 100-200)
- FCT mint transactions with proper gas deduction