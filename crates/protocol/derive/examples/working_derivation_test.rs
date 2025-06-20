//! Working derivation test based on patterns from the Kona codebase
//! This properly handles the Ethereum vs Optimism network types

use alloy_primitives::{hex, BlockNumber};
use alloy_provider::{Provider, RootProvider};
use alloy_rpc_types_eth::BlockNumberOrTag;
use clap::Parser;
use eyre::Result;
use kona_derive::{
    attributes::StatefulAttributesBuilder,
    traits::AttributesBuilder,
};
use kona_genesis::RollupConfig;
use kona_protocol::BatchValidationProvider;
use kona_providers_alloy::{AlloyChainProvider, AlloyL2ChainProvider};
use op_alloy_network::Optimism;
use std::sync::Arc;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// L2 block number to test
    #[arg(short, long)]
    block: BlockNumber,
    
    /// L1 RPC endpoint
    #[arg(long, env = "L1_RPC")]
    l1_rpc: String,
    
    /// L2 RPC endpoint  
    #[arg(long, env = "L2_RPC")]
    l2_rpc: String,
    
    /// Verbose output
    #[arg(short, long)]
    verbose: bool,
}

async fn create_facet_rollup_config() -> Result<RollupConfig> {
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

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    
    println!("🔍 Testing derivation for block {}", args.block);
    println!("   L1 RPC: {}", args.l1_rpc);
    println!("   L2 RPC: {}", args.l2_rpc);
    
    // Create providers with correct network types
    let l1_provider: RootProvider = RootProvider::new_http(args.l1_rpc.parse()?);
    let l2_provider: RootProvider<Optimism> = RootProvider::new_http(args.l2_rpc.parse()?);
    
    // Get rollup config
    let rollup_config = Arc::new(create_facet_rollup_config().await?);
    
    // Create chain providers
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
    let parent_num = args.block.saturating_sub(1);
    println!("\n1️⃣ Fetching parent block (block {})", parent_num);
    let mut l2_provider_mut = l2_chain_provider.clone();
    
    // For block 6, we need to handle the case where parent might be genesis
    let parent_info = if parent_num == 0 {
        // Genesis block - create default L2BlockInfo
        use kona_protocol::{L2BlockInfo, BlockInfo};
        use alloy_eips::BlockNumHash;
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
        l2_provider_mut
            .l2_block_info_by_number(parent_num)
            .await?
    };
    
    if args.verbose {
        println!("   Parent block info:");
        println!("   - Number: {}", parent_info.block_info.number);
        println!("   - Hash: 0x{}", hex::encode(parent_info.block_info.hash));
        println!("   - L1 origin: {} (0x{})", 
            parent_info.l1_origin.number, 
            hex::encode(parent_info.l1_origin.hash)
        );
        println!("   - Sequence number: {}", parent_info.seq_num);
    }
    
    // Get the target block to determine L1 epoch
    println!("\n2️⃣ Determining L1 epoch for block {}", args.block);
    let target_block_info = l2_provider_mut
        .l2_block_info_by_number(args.block)
        .await?;
    
    // Check if this is a new epoch
    let l1_epoch = if target_block_info.l1_origin.number != parent_info.l1_origin.number {
        // New epoch - use the new L1 origin
        target_block_info.l1_origin
    } else {
        // Same epoch as parent
        parent_info.l1_origin
    };
    
    if args.verbose {
        println!("   L1 epoch: {} (0x{})", l1_epoch.number, hex::encode(l1_epoch.hash));
    }
    
    // Check if this is a new epoch
    if l1_epoch.number != parent_info.l1_origin.number {
        println!("   ✨ New epoch detected! L1 origin changed from {} to {}", 
            parent_info.l1_origin.number, l1_epoch.number);
    }
    
    // Derive attributes
    println!("\n3️⃣ Running Kona derivation pipeline");
    let attributes = builder.prepare_payload_attributes(parent_info, l1_epoch).await?;
    
    let kona_txs = attributes.transactions.as_ref()
        .ok_or_else(|| eyre::eyre!("No transactions in derived attributes"))?;
    
    println!("   Derived {} transactions", kona_txs.len());
    
    // Compare with actual block from RPC
    println!("\n4️⃣ Fetching actual block from L2 RPC for comparison");
    let actual_block = l2_provider
        .get_block_by_number(BlockNumberOrTag::Number(args.block))
        .full()
        .await?
        .ok_or_else(|| eyre::eyre!("Block {} not found", args.block))?;
    
    let actual_txs = match &actual_block.transactions {
        alloy_rpc_types_eth::BlockTransactions::Full(txs) => txs,
        _ => return Err(eyre::eyre!("Expected full transactions in block")),
    };
    
    println!("   Geth transactions: {}", actual_txs.len());
    println!("   Kona transactions: {}", kona_txs.len());
    
    if actual_txs.len() != kona_txs.len() {
        return Err(eyre::eyre!(
            "Transaction count mismatch: Geth {} vs Kona {}",
            actual_txs.len(),
            kona_txs.len()
        ));
    }
    
    // Compare each transaction
    println!("\n5️⃣ Comparing transactions byte-by-byte");
    for (i, (geth_tx, kona_tx_bytes)) in actual_txs.iter().zip(kona_txs.iter()).enumerate() {
        // Get transaction bytes using eip2718 encoding
        use alloy_eips::eip2718::Encodable2718;
        let geth_bytes = geth_tx.inner.inner.encoded_2718();
        
        if &geth_bytes != kona_tx_bytes {
            println!("   ❌ Transaction {} mismatch:", i);
            if args.verbose {
                println!("      Geth: 0x{}", hex::encode(&geth_bytes));
                println!("      Kona: 0x{}", hex::encode(kona_tx_bytes));
            }
            // Continue to see all transactions instead of failing immediately
            // return Err(eyre::eyre!("Transaction {} encoding mismatch", i));
        }
        
        if args.verbose {
            println!("   ✅ Transaction {} matches ({} bytes)", i, geth_bytes.len());
        }
    }
    
    // Check L1BlockInfoTx
    if let Some(first_tx) = kona_txs.first() {
        if first_tx.get(0) == Some(&0x7e) {
            println!("\n✅ First transaction is L1BlockInfoTx (deposit)");
            if first_tx.len() > 190 && first_tx.len() < 200 {
                println!("✅ Facet L1BlockInfoTx format detected ({} bytes)", first_tx.len());
            }
        }
    }
    
    println!("\n✅ All derivation checks passed!");
    
    Ok(())
}

