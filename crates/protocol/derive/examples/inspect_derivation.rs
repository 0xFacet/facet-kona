//! Derivation test that inspects structured transaction fields before RLP encoding
//! This helps isolate differences between Kona and Geth implementations

use alloy_primitives::{hex, BlockNumber};
use alloy_provider::{Provider, RootProvider};
use alloy_rpc_types_eth::BlockNumberOrTag;
use alloy_eips::eip2718::Decodable2718;
use clap::Parser;
use eyre::Result;
use kona_derive::{
    attributes::StatefulAttributesBuilder,
    traits::AttributesBuilder,
};
use kona_genesis::RollupConfig;
use kona_protocol::{BatchValidationProvider, L1BlockInfoTx, FctMintCalculator};
use kona_providers_alloy::{AlloyChainProvider, AlloyL2ChainProvider};
use op_alloy_network::Optimism;
use op_alloy_consensus::TxDeposit;
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

/// Inspect FCT values from L1BlockInfoTx
fn inspect_l1_block_info_tx(tx_bytes: &[u8]) -> Result<()> {
    // Skip the 0x7e prefix
    let deposit_data = &tx_bytes[1..];
    let deposit_tx = TxDeposit::decode_2718(&mut &deposit_data[..])?;
    
    println!("\n📋 L1BlockInfoTx Deposit Transaction:");
    println!("   From: {}", deposit_tx.from);
    println!("   To: {:?}", deposit_tx.to);
    println!("   Gas limit: {}", deposit_tx.gas_limit);
    println!("   Value: {}", deposit_tx.value);
    println!("   Mint: {:?}", deposit_tx.mint);
    println!("   Input length: {} bytes", deposit_tx.input.len());
    
    // Decode the L1BlockInfoTx from calldata
    let l1_info = L1BlockInfoTx::decode_calldata(&deposit_tx.input)?;
    
    match l1_info {
        L1BlockInfoTx::Facet(facet) => {
            println!("\n📊 L1BlockInfoTx::Facet Fields:");
            println!("   Number: {}", facet.number);
            println!("   Time: {}", facet.time);
            println!("   Base fee: {}", facet.base_fee);
            println!("   Block hash: 0x{}", hex::encode(facet.block_hash));
            println!("   Sequence number: {}", facet.sequence_number);
            println!("   Batcher address: {}", facet.batcher_address);
            println!("   Blob base fee: {}", facet.blob_base_fee);
            println!("   Blob base fee scalar: {}", facet.blob_base_fee_scalar);
            println!("   Base fee scalar: {}", facet.base_fee_scalar);
            println!("   Empty scalars: {}", facet.empty_scalars);
            println!("   L1 fee overhead: {}", facet.l1_fee_overhead);
            println!("   ⭐ FCT mint rate: {}", facet.fct_mint_rate);
            println!("   ⭐ FCT mint period L1 data gas: {}", facet.fct_mint_period_l1_data_gas);
            
            // Show what these values would be at block 6
            let expected_mint_rate = FctMintCalculator::compute_new_rate(6, 0, 0);
            println!("\n   📐 Expected FCT values for block 6:");
            println!("      - Mint rate: {} (vs actual: {})", expected_mint_rate, facet.fct_mint_rate);
            println!("      - Period L1 data gas: Should match cumulative from deposits");
            
            // Decode the raw bytes to see the layout
            println!("\n   🔍 Raw calldata (last 32 bytes):");
            let calldata_len = deposit_tx.input.len();
            if calldata_len >= 32 {
                let last_32 = &deposit_tx.input[calldata_len - 32..];
                println!("      0x{}", hex::encode(last_32));
                
                // Try to parse as two u128 values
                if last_32.len() == 32 {
                    let mint_rate_bytes = &last_32[0..16];
                    let gas_bytes = &last_32[16..32];
                    
                    let mint_rate = u128::from_be_bytes(mint_rate_bytes.try_into()?);
                    let gas = u128::from_be_bytes(gas_bytes.try_into()?);
                    
                    println!("      Parsed as u128s:");
                    println!("        - First 16 bytes (mint rate?): {}", mint_rate);
                    println!("        - Last 16 bytes (gas?): {}", gas);
                }
            }
        }
        _ => {
            println!("\n⚠️  Not a Facet L1BlockInfoTx variant");
        }
    }
    
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    
    println!("🔍 Inspecting derivation for block {}", args.block);
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
    
    let parent_info = if parent_num == 0 {
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
    
    // Get the target block to determine L1 epoch
    println!("\n2️⃣ Determining L1 epoch for block {}", args.block);
    let target_block_info = l2_provider_mut
        .l2_block_info_by_number(args.block)
        .await?;
    
    let l1_epoch = if target_block_info.l1_origin.number != parent_info.l1_origin.number {
        target_block_info.l1_origin
    } else {
        parent_info.l1_origin
    };
    
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
    
    // Inspect the L1BlockInfoTx from Kona
    if let Some(first_tx) = kona_txs.first() {
        println!("\n================== KONA L1BlockInfoTx ==================");
        inspect_l1_block_info_tx(first_tx)?;
    }
    
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
    
    // Inspect the L1BlockInfoTx from Geth
    if let Some(geth_tx) = actual_txs.first() {
        use alloy_eips::eip2718::Encodable2718;
        let geth_bytes = geth_tx.inner.inner.encoded_2718();
        
        println!("\n================== GETH L1BlockInfoTx ==================");
        inspect_l1_block_info_tx(&geth_bytes)?;
    }
    
    // Compare byte-by-byte
    println!("\n5️⃣ Comparing transactions byte-by-byte");
    for (i, (geth_tx, kona_tx_bytes)) in actual_txs.iter().zip(kona_txs.iter()).enumerate() {
        use alloy_eips::eip2718::Encodable2718;
        let geth_bytes = geth_tx.inner.inner.encoded_2718();
        
        println!("\n   Transaction {}: {} bytes (Geth) vs {} bytes (Kona)", 
            i, geth_bytes.len(), kona_tx_bytes.len());
        
        if &geth_bytes != kona_tx_bytes {
            println!("   ❌ Transaction {} differs", i);
            
            // For L1BlockInfoTx, show where the differences are
            if i == 0 {
                println!("\n   📍 Byte-level differences:");
                let min_len = geth_bytes.len().min(kona_tx_bytes.len());
                let mut first_diff = None;
                
                for j in 0..min_len {
                    if geth_bytes[j] != kona_tx_bytes[j] {
                        if first_diff.is_none() {
                            first_diff = Some(j);
                            println!("      First difference at byte {}", j);
                        }
                    }
                }
                
                if let Some(diff_pos) = first_diff {
                    let start = diff_pos.saturating_sub(16);
                    let end = (diff_pos + 16).min(min_len);
                    
                    println!("      Around byte {} (showing bytes {}-{}):", diff_pos, start, end);
                    println!("      Geth: 0x{}", hex::encode(&geth_bytes[start..end]));
                    println!("      Kona: 0x{}", hex::encode(&kona_tx_bytes[start..end]));
                }
                
                // Show the tail differences
                if geth_bytes.len() >= 32 && kona_tx_bytes.len() >= 32 {
                    println!("\n   📏 Last 32 bytes comparison:");
                    let geth_tail = &geth_bytes[geth_bytes.len() - 32..];
                    let kona_tail = &kona_tx_bytes[kona_tx_bytes.len() - 32..];
                    println!("      Geth: 0x{}", hex::encode(geth_tail));
                    println!("      Kona: 0x{}", hex::encode(kona_tail));
                }
            }
        } else {
            println!("   ✅ Transaction {} matches", i);
        }
    }
    
    println!("\n✅ Inspection complete!");
    
    Ok(())
}