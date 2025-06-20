use crate::retry::{calculate_backoff, classify_error, CircuitBreaker};
use crate::types::{ErrorType, TestResult};
use alloy_provider::{Provider, RootProvider};
use eyre::Result;
use kona_derive::attributes::StatefulAttributesBuilder;
use kona_derive::traits::AttributesBuilder;
use kona_genesis::RollupConfig;
use kona_protocol::BatchValidationProvider;
use kona_providers_alloy::{AlloyChainProvider, AlloyL2ChainProvider};
use op_alloy_network::Optimism;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, warn};

pub async fn validate_derivation(
    block: u64,
    l1_rpc: &str,
    l2_rpc: &str,
    max_retries: u32,
) -> Result<TestResult> {
    let mut retries = 0;
    let mut last_error = None;
    let mut last_error_type = None;
    let mut circuit_breaker = CircuitBreaker::new(5, Duration::from_secs(60));
    let mut effective_max_retries = max_retries;
    
    loop {
        // Check circuit breaker
        if circuit_breaker.is_open() {
            warn!("Circuit breaker open for block {} derivation, skipping", block);
            return Ok(TestResult {
                success: false,
                error: Some("Circuit breaker open - too many consecutive network failures".to_string()),
                error_type: Some(ErrorType::Network),
                retries,
            });
        }
        
        match run_derivation_test(block, l1_rpc, l2_rpc).await {
            Ok(_) => {
                circuit_breaker.record_success();
                return Ok(TestResult {
                    success: true,
                    error: None,
                    error_type: None,
                    retries,
                });
            }
            Err(e) => {
                let error_type = classify_error(&e);
                last_error = Some(e.to_string());
                last_error_type = Some(error_type);
                
                // Update effective max retries based on error type
                effective_max_retries = effective_max_retries.min(error_type.max_retries());
                
                // Record failure in circuit breaker for network errors
                if error_type == ErrorType::Network || error_type == ErrorType::RateLimit {
                    circuit_breaker.record_failure();
                }
                
                // Don't retry if it's a validation error
                if !error_type.should_retry() {
                    debug!("Block {} derivation failed with non-retryable error: {:?}", block, error_type);
                    break;
                }
                
                // Check if we've exceeded retries for this error type
                if retries >= effective_max_retries {
                    debug!("Block {} derivation exceeded max retries ({}) for error type {:?}", 
                        block, effective_max_retries, error_type);
                    break;
                }
                
                retries += 1;
                
                let backoff = calculate_backoff(retries - 1, error_type);
                debug!(
                    "Block {} derivation retry {}/{} after {:?} (error type: {:?})",
                    block, retries, effective_max_retries, backoff, error_type
                );
                tokio::time::sleep(backoff).await;
            }
        }
    }
    
    Ok(TestResult {
        success: false,
        error: last_error,
        error_type: last_error_type,
        retries,
    })
}

async fn run_derivation_test(block: u64, l1_rpc: &str, l2_rpc: &str) -> Result<()> {
    debug!("Testing derivation for block {}", block);
    
    // Create providers
    let l1_provider: RootProvider = RootProvider::new_http(l1_rpc.parse()?);
    let l2_provider: RootProvider<Optimism> = RootProvider::new_http(l2_rpc.parse()?);
    
    // Create rollup config for Facet
    let rollup_config = Arc::new(create_facet_rollup_config()?);
    
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
    let parent_num = block.saturating_sub(1);
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
    let target_block_info = l2_provider_mut
        .l2_block_info_by_number(block)
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
    
    // Compare with actual block from RPC
    let actual_block = l2_provider
        .get_block_by_number(alloy_rpc_types_eth::BlockNumberOrTag::Number(block))
        .full()
        .await?
        .ok_or_else(|| eyre::eyre!("Block {} not found", block))?;
    
    let actual_txs = match &actual_block.transactions {
        alloy_rpc_types_eth::BlockTransactions::Full(txs) => txs,
        _ => return Err(eyre::eyre!("Expected full transactions in block")),
    };
    
    // Verify transaction count matches
    if actual_txs.len() != kona_txs.len() {
        return Err(eyre::eyre!(
            "Transaction count mismatch: Geth {} vs Kona {}",
            actual_txs.len(),
            kona_txs.len()
        ));
    }
    
    // Compare each transaction
    for (i, (geth_tx, kona_tx_bytes)) in actual_txs.iter().zip(kona_txs.iter()).enumerate() {
        use alloy_eips::eip2718::Encodable2718;
        let geth_bytes = geth_tx.inner.inner.encoded_2718();
        
        if &geth_bytes != kona_tx_bytes {
            return Err(eyre::eyre!(
                "Transaction {} differs at block {}: Geth {} bytes vs Kona {} bytes",
                i, block, geth_bytes.len(), kona_tx_bytes.len()
            ));
        }
    }
    
    Ok(())
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