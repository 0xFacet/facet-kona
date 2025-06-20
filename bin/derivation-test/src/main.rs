//! Minimal derivation test with detailed output similar to execution-fixture
//!
//! Usage: cargo run --release -p derivation-test -- --l2-rpc https://mainnet.facet.org --block-number 721318

use alloy_eips::{eip2718::Encodable2718, BlockNumHash};
use alloy_primitives::hex;
use alloy_provider::{Provider, RootProvider};
use alloy_rpc_types_eth::{BlockNumberOrTag, BlockTransactions};
use clap::Parser;
use eyre::Result;
use kona_derive::{attributes::StatefulAttributesBuilder, traits::AttributesBuilder};
use kona_genesis::RollupConfig;
use kona_protocol::{BatchValidationProvider, BlockInfo, L2BlockInfo};
use kona_providers_alloy::{AlloyChainProvider, AlloyL2ChainProvider};
use op_alloy_network::Optimism;
use std::sync::Arc;
use tracing::{info, warn};

#[derive(Parser)]
#[command(about = "Test derivation with detailed output similar to execution-fixture")]
struct Args {
    #[arg(short = 'b', long)]
    block_number: u64,
    
    #[arg(long, env = "L1_RPC")]
    l1_rpc: String,
    
    #[arg(long, short = 'r', env = "L2_RPC")]
    l2_rpc: String,
}

fn create_facet_rollup_config() -> Result<RollupConfig> {
    let mut config = RollupConfig::default();
    
    // Set Facet-specific values
    config.l2_chain_id = 0xface7;
    config.block_time = 12;
    config.max_sequencer_drift = 600;
    config.seq_window_size = 3600;
    config.channel_timeout = 300;
    config.granite_channel_timeout = 50;
    
    // Set addresses
    config.batch_inbox_address = "0xFACEC003e8e0cF7152467C26D37634925A9ce65B".parse()?;
    config.deposit_contract_address = "0x00000000000000000000000000000000000face7".parse()?;
    
    // Enable all hardforks from genesis
    config.hardforks.regolith_time = Some(0);
    config.hardforks.canyon_time = Some(0);
    config.hardforks.delta_time = Some(0);
    config.hardforks.ecotone_time = Some(0);
    config.hardforks.fjord_time = Some(0);
    config.hardforks.granite_time = Some(0);
    
    Ok(config)
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let args = Args::parse();
    
    warn!("⚠️  Chain ID 1027303 not found in registry, using custom facet config");
    
    // Get actual block first for comparison
    let l2_provider: RootProvider<Optimism> = RootProvider::new_http(args.l2_rpc.parse()?);
    let actual_block = l2_provider
        .get_block_by_number(BlockNumberOrTag::Number(args.block_number))
        .full()
        .await?
        .ok_or_else(|| eyre::eyre!("Block {} not found", args.block_number))?;
    
    let actual_txs = match &actual_block.transactions {
        BlockTransactions::Full(txs) => txs,
        _ => return Err(eyre::eyre!("Expected full transactions")),
    };
    
    // Print Geth transactions like execution-fixture does
    for (i, tx) in actual_txs.iter().enumerate() {
        let tx_bytes = tx.inner.inner.encoded_2718();
        let tx_type = match tx_bytes.get(0) {
            Some(0x7e) => "DEPOSIT (0x7e)",
            Some(0x02) => "EIP-1559 (0x02)",
            Some(0x01) => "EIP-2930 (0x01)",
            _ => "LEGACY",
        };
        
        println!("=== GETH Transaction {} ===", i);
        println!("  Raw length: {} bytes", tx_bytes.len());
        println!("  First 40 bytes: {:?}", &tx_bytes[..tx_bytes.len().min(40)]);
        println!("  Type: {}", tx_type);
        if tx_type.starts_with("DEPOSIT") {
            println!("  Mint value from Geth: \"0x0\"");
        }
    }
    println!("=== Total transactions from Geth: {} ===\n", actual_txs.len());
    
    // Run derivation
    info!(block_number = args.block_number, "Beginning derivation");
    
    // Setup providers and config
    let l1_provider: RootProvider = RootProvider::new_http(args.l1_rpc.parse()?);
    let rollup_config = Arc::new(create_facet_rollup_config()?);
    let l1_chain_provider = AlloyChainProvider::new(l1_provider.clone(), 100);
    let l2_chain_provider = AlloyL2ChainProvider::new(
        l2_provider.clone(),
        rollup_config.clone(),
        100
    );
    
    // Create attributes builder
    let mut builder = StatefulAttributesBuilder::new(
        rollup_config.clone(),
        l2_chain_provider.clone(),
        l1_chain_provider.clone(),
    );
    
    // Get parent block info
    let parent_num = args.block_number.saturating_sub(1);
    let mut l2_provider_mut = l2_chain_provider.clone();
    
    let parent_info = if parent_num == 0 {
        L2BlockInfo {
            block_info: BlockInfo {
                number: 0,
                timestamp: 0,
                hash: Default::default(),
                parent_hash: Default::default(),
            },
            l1_origin: BlockNumHash {
                number: 0,
                hash: Default::default(),
            },
            seq_num: 0,
        }
    } else {
        l2_provider_mut.l2_block_info_by_number(parent_num).await?
    };
    
    // Get the target block to determine L1 epoch
    let target_block_info = l2_provider_mut
        .l2_block_info_by_number(args.block_number)
        .await?;
    
    let l1_epoch = if target_block_info.l1_origin.number != parent_info.l1_origin.number {
        target_block_info.l1_origin
    } else {
        parent_info.l1_origin
    };
    
    // Derive attributes
    let attributes = builder.prepare_payload_attributes(parent_info, l1_epoch).await?;
    
    let kona_txs = attributes.transactions.as_ref()
        .ok_or_else(|| eyre::eyre!("No transactions in derived attributes"))?;
    
    info!("Finished derivation. Derived transactions: {}", kona_txs.len());
    
    println!("\n=== Derivation Results ===");
    println!("Transactions count: {} (expected: {})", kona_txs.len(), actual_txs.len());
    println!();
    
    // Compare like execution-fixture
    let mut all_match = true;
    for (i, (geth_tx, kona_tx_bytes)) in actual_txs.iter().zip(kona_txs.iter()).enumerate() {
        let geth_bytes = geth_tx.inner.inner.encoded_2718();
        
        if &geth_bytes != kona_tx_bytes {
            println!("Transaction {}: ❌ MISMATCH", i);
            println!("  Geth: {} bytes", geth_bytes.len());
            println!("  Kona: {} bytes", kona_tx_bytes.len());
            all_match = false;
        } else {
            println!("Transaction {}: ✅ Match ({} bytes)", i, geth_bytes.len());
        }
    }
    
    if kona_txs.len() != actual_txs.len() {
        println!("\n❌ Transaction count mismatch!");
        all_match = false;
    }
    
    println!("\n=== Derivation Validation ===");
    if all_match {
        println!("✅ All derivation checks passed!");
        info!(block_number = args.block_number, "Successfully validated derivation");
    } else {
        println!("❌ Derivation validation failed");
        return Err(eyre::eyre!("Derivation mismatch"));
    }
    
    Ok(())
}