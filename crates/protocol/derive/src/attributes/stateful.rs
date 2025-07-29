//! The [`AttributesBuilder`] and it's default implementation.

use crate::{
    errors::{BuilderError, PipelineError, PipelineErrorKind},
    traits::{AttributesBuilder, ChainProvider, L2ChainProvider},
    types::PipelineResult,
};
use alloc::{boxed::Box, fmt::Debug, format, string::ToString, sync::Arc, vec, vec::Vec};
use alloy_consensus::Transaction;
use alloy_eips::BlockNumHash;
use alloy_primitives::{Address, Bytes};
use alloy_rpc_types_engine::PayloadAttributes;
use async_trait::async_trait;
use kona_genesis::RollupConfig;
use kona_hardforks::{Hardfork, Hardforks};
use kona_protocol::{
    L1BlockInfoTx, L2BlockInfo
};
use op_alloy_rpc_types_engine::OpPayloadAttributes;
use crate::derive_facet_deposits;
use op_alloy_consensus::{DepositSourceDomain, L1InfoDepositSource, TxDeposit};
use alloy_primitives::{TxKind, U256, address};
use kona_protocol::Predeploys;

// Define constants used for building the deposit transaction
const L1_INFO_DEPOSITOR_ADDRESS: Address = address!("deaddeaddeaddeaddeaddeaddeaddeaddead0001");
const REGOLITH_SYSTEM_TX_GAS: u64 = 1_000_000;

/// A stateful implementation of the [AttributesBuilder].
#[derive(Debug, Default)]
pub struct StatefulAttributesBuilder<L1P, L2P>
where
    L1P: ChainProvider + Debug,
    L2P: L2ChainProvider + Debug,
{
    /// The rollup config.
    rollup_cfg: Arc<RollupConfig>,
    /// The system config fetcher.
    config_fetcher: L2P,
    /// The L1 receipts fetcher.
    receipts_fetcher: L1P,
}

impl<L1P, L2P> StatefulAttributesBuilder<L1P, L2P>
where
    L1P: ChainProvider + Debug,
    L2P: L2ChainProvider + Debug,
{
    /// Create a new [StatefulAttributesBuilder] with the given epoch.
    pub const fn new(rcfg: Arc<RollupConfig>, sys_cfg_fetcher: L2P, receipts: L1P) -> Self {
        Self { rollup_cfg: rcfg, config_fetcher: sys_cfg_fetcher, receipts_fetcher: receipts }
    }
}

#[async_trait]
impl<L1P, L2P> AttributesBuilder for StatefulAttributesBuilder<L1P, L2P>
where
    L1P: ChainProvider + Debug + Send,
    L2P: L2ChainProvider + Debug + Send,
{
    async fn prepare_payload_attributes(
        &mut self,
        l2_parent: L2BlockInfo,
        epoch: BlockNumHash,
    ) -> PipelineResult<OpPayloadAttributes> {
        tracing::info!(
            target: "attributes_builder",
            "prepare_payload_attributes called for L2 block {} (parent: {}), L1 origin: {} -> epoch: {}",
            l2_parent.block_info.number + 1,
            l2_parent.block_info.number,
            l2_parent.l1_origin.number,
            epoch.number
        );
        let l1_header: alloy_consensus::Header;
        let deposit_transactions: Vec<Bytes>;

        let mut sys_config = self
            .config_fetcher
            .system_config_by_number(l2_parent.block_info.number, self.rollup_cfg.clone())
            .await
            .map_err(Into::into)?;

        // Initialize FCT values - will be updated if processing facet deposits
        let new_fct_mint_rate: u128;
        let new_fct_total_minted: u128;
        let new_fct_period_start_block: u128;
        let new_fct_period_minted: u128;
        // Pull static FCT parameters from rollup config (fall back to zero if absent)
        let default_fct_max_supply: u128 = self
            .rollup_cfg
            .fct_max_supply
            .and_then(|v| v.try_into().ok())
            .unwrap_or(0u128);
        let default_fct_initial_target_per_period: u128 = self
            .rollup_cfg
            .fct_initial_target_per_period
            .and_then(|v| v.try_into().ok())
            .unwrap_or(0u128);
        
        // Read parent L1 info from parent block (needed for both new and continuing epochs)
        let parent_l1_info = if l2_parent.block_info.number > 0 {
            // Fetch parent block to get facet parameters
            let parent_block = self
                .config_fetcher
                .block_by_number(l2_parent.block_info.number)
                .await
                .map_err(|e| PipelineError::AttributesBuilder(BuilderError::Custom(e.to_string())).crit())?;
            
            let first_tx = parent_block.body.transactions.first()
                .ok_or_else(|| PipelineError::AttributesBuilder(BuilderError::Custom("Parent block has no transactions".to_string())).crit())?;
            
            let deposit_tx = first_tx.as_deposit()
                .ok_or_else(|| PipelineError::AttributesBuilder(BuilderError::Custom("First transaction is not a deposit".to_string())).crit())?;
            
            let l1_info = L1BlockInfoTx::decode_calldata(deposit_tx.input().as_ref())
                .map_err(|e| PipelineError::AttributesBuilder(BuilderError::Custom(format!("Failed to decode L1 info: {}", e))).crit())?;
            
            match l1_info {
                L1BlockInfoTx::Facet(facet_info) => Some(facet_info),
                _ => return Err(PipelineError::AttributesBuilder(BuilderError::Custom("Parent block is not using Facet L1 info variant".to_string())).crit()),
            }
        } else {
            // For genesis block, return None - we'll create default values
            None
        };

        // If the L1 origin changed in this block, then we are in the first block of the epoch.
        // In this case we need to fetch all transaction receipts from the L1 origin block so
        // we can scan for user deposits.
        let sequence_number = if l2_parent.l1_origin.number != epoch.number {
            tracing::info!(
                target: "attributes_builder",
                "L1 origin changed from {} to {} for L2 block {}",
                l2_parent.l1_origin.number,
                epoch.number,
                l2_parent.block_info.number + 1
            );
            let header =
                self.receipts_fetcher.header_by_hash(epoch.hash).await.map_err(Into::into)?;
            if l2_parent.l1_origin.hash != header.parent_hash {
                return Err(PipelineErrorKind::Reset(
                    BuilderError::BlockMismatchEpochReset(
                        epoch,
                        l2_parent.l1_origin,
                        header.parent_hash,
                    )
                    .into(),
                ));
            }
            let receipts =
                self.receipts_fetcher.receipts_by_hash(epoch.hash).await.map_err(Into::into)?;
            let (_, txs) = self
                .receipts_fetcher
                .block_info_and_transactions_by_hash(epoch.hash)
                .await
                .map_err(Into::into)?;

            tracing::info!(
                target: "attributes_builder",
                "Processing L1 block {} with {} transactions and {} receipts",
                epoch.number,
                txs.len(),
                receipts.len()
            );
            
            l1_header = header;
            
            // Get L1 base fee from header
            let l1_base_fee = l1_header.base_fee_per_gas.unwrap_or(0);
            
            // Get parent L1 info for FCT state - use the one we loaded earlier or create default
            let parent_l1_info_data = parent_l1_info.unwrap_or(kona_protocol::L1BlockInfoFacet {
                number: 0,
                time: 0,
                base_fee: 0,
                block_hash: Default::default(),
                sequence_number: 0,
                batcher_address: Default::default(),
                blob_base_fee: 0,
                blob_base_fee_scalar: 0,
                base_fee_scalar: 0,
                empty_scalars: false,
                l1_fee_overhead: Default::default(),
                fct_mint_rate: self.rollup_cfg.fct_initial_rate
                    .and_then(|rate| rate.try_into().ok())
                    .unwrap_or(0u128),
                fct_total_minted: 0,
                fct_period_start_block: 0,
                fct_period_minted: 0,
                fct_max_supply: default_fct_max_supply,
                fct_initial_target_per_period: default_fct_initial_target_per_period,
            });
            
            let (deposits, rate, total_minted, period_start_block, period_minted) = derive_facet_deposits(
                &txs,
                &receipts,
                self.rollup_cfg.l2_chain_id,
                l2_parent.block_info.number + 1, // Next L2 block number
                l1_base_fee,
                &parent_l1_info_data,
            )
            .map_err(|e| PipelineError::BadEncoding(e).crit())?;
            
            tracing::info!(
                target: "attributes_builder",
                "derive_facet_deposits returned {} deposits for L2 block {}",
                deposits.len(),
                l2_parent.block_info.number + 1
            );
            
            // Update FCT values
            new_fct_mint_rate = rate;
            new_fct_total_minted = total_minted;
            new_fct_period_start_block = period_start_block;
            new_fct_period_minted = period_minted;
            sys_config
                .update_with_receipts(
                    &receipts,
                    self.rollup_cfg.l1_system_config_address,
                    self.rollup_cfg.is_ecotone_active(l1_header.timestamp),
                )
                .map_err(|e| PipelineError::SystemConfigUpdate(e).crit())?;
            deposit_transactions = deposits;
            0
        } else {
            tracing::debug!(
                target: "attributes_builder",
                "L1 origin unchanged at {} for L2 block {}, sequence number {}",
                epoch.number,
                l2_parent.block_info.number + 1,
                l2_parent.seq_num + 1
            );
            
            #[allow(clippy::collapsible_else_if)]
            if l2_parent.l1_origin.hash != epoch.hash {
                return Err(PipelineErrorKind::Reset(
                    BuilderError::BlockMismatch(epoch, l2_parent.l1_origin).into(),
                ));
            }

            let header =
                self.receipts_fetcher.header_by_hash(epoch.hash).await.map_err(Into::into)?;
            l1_header = header;
            deposit_transactions = vec![];
            // Preserve parent FCT values when not processing deposits
            if let Some(parent_info) = parent_l1_info {
                new_fct_mint_rate = parent_info.fct_mint_rate;
                new_fct_total_minted = parent_info.fct_total_minted;
                new_fct_period_start_block = parent_info.fct_period_start_block;
                new_fct_period_minted = parent_info.fct_period_minted;
            } else {
                // Genesis case
                new_fct_mint_rate = self.rollup_cfg.fct_initial_rate
                    .and_then(|rate| rate.try_into().ok())
                    .unwrap_or(0u128);
                new_fct_total_minted = 0;
                new_fct_period_start_block = 0;
                new_fct_period_minted = 0;
            }
            l2_parent.seq_num + 1
        };

        // Sanity check the L1 origin was correctly selected to maintain the time invariant
        // between L1 and L2.
        let next_l2_time = l2_parent.block_info.timestamp + self.rollup_cfg.block_time;
        if next_l2_time < l1_header.timestamp {
            return Err(PipelineErrorKind::Reset(
                BuilderError::BrokenTimeInvariant(
                    l2_parent.l1_origin,
                    next_l2_time,
                    BlockNumHash { hash: l1_header.hash_slow(), number: l1_header.number },
                    l1_header.timestamp,
                )
                .into(),
            ));
        }

        let mut upgrade_transactions: Vec<Bytes> = vec![];
        if self.rollup_cfg.is_ecotone_active(next_l2_time) &&
            !self.rollup_cfg.is_ecotone_active(l2_parent.block_info.timestamp)
        {
            upgrade_transactions = Hardforks::ECOTONE.txs().collect();
        }
        if self.rollup_cfg.is_fjord_active(next_l2_time) &&
            !self.rollup_cfg.is_fjord_active(l2_parent.block_info.timestamp)
        {
            upgrade_transactions.append(&mut Hardforks::FJORD.txs().collect());
        }
        if self.rollup_cfg.is_isthmus_active(next_l2_time) &&
            !self.rollup_cfg.is_isthmus_active(l2_parent.block_info.timestamp)
        {
            upgrade_transactions.append(&mut Hardforks::ISTHMUS.txs().collect());
        }
        if self.rollup_cfg.is_interop_active(next_l2_time) &&
            !self.rollup_cfg.is_interop_active(l2_parent.block_info.timestamp)
        {
            upgrade_transactions.append(&mut Hardforks::INTEROP.txs().collect());
        }

        // Build and encode the L1 info transaction for the current payload.
        // First create the L1 info object.
        let mut l1_info_tx = L1BlockInfoTx::try_new(
            &self.rollup_cfg,
            &sys_config,
            sequence_number,
            &l1_header,
            next_l2_time,
        )
        .map_err(|e| {
            PipelineError::AttributesBuilder(BuilderError::Custom(e.to_string())).crit()
        })?;
        // Ensure static FCT parameters are populated for Facet variant
        if let L1BlockInfoTx::Facet(ref mut facet_info) = l1_info_tx {
            if facet_info.fct_max_supply == 0 {
                facet_info.fct_max_supply = default_fct_max_supply;
            }
            if facet_info.fct_initial_target_per_period == 0 {
                facet_info.fct_initial_target_per_period = default_fct_initial_target_per_period;
            }
        }

        // Update the Facet-specific FCT values before encoding.
        l1_info_tx.set_fct_values(
            new_fct_mint_rate,
            new_fct_total_minted,
            new_fct_period_start_block,
            new_fct_period_minted,
        );

        // Build the deposit transaction envelope from the updated info.
        let source = DepositSourceDomain::L1Info(L1InfoDepositSource {
            l1_block_hash: l1_info_tx.block_hash(),
            seq_number: sequence_number,
        });

        let deposit_tx = TxDeposit {
            source_hash: source.source_hash(),
            from: L1_INFO_DEPOSITOR_ADDRESS,
            to: TxKind::Call(Predeploys::L1_BLOCK_INFO),
            mint: None,
            value: U256::ZERO,
            gas_limit: REGOLITH_SYSTEM_TX_GAS,
            is_system_transaction: false,
            input: l1_info_tx.encode_calldata(),
        };

        let encoded_l1_info_tx = kona_protocol::encode_deposit_with_bluebird_type(&deposit_tx);

        let mut txs =
            Vec::with_capacity(1 + deposit_transactions.len() + upgrade_transactions.len());
        txs.push(encoded_l1_info_tx.into());
        txs.extend(deposit_transactions.clone());
        txs.extend(upgrade_transactions);
        
        tracing::info!(
            target: "attributes_builder",
            "Building payload for L2 block {} with {} total transactions (1 L1 info + {} deposits + {} upgrades)",
            l2_parent.block_info.number + 1,
            txs.len(),
            deposit_transactions.len(),
            txs.len() - 1 - deposit_transactions.len()
        );

        let mut withdrawals = None;
        if self.rollup_cfg.is_canyon_active(next_l2_time) {
            withdrawals = Some(Vec::default());
        }

        let mut parent_beacon_root = None;
        if self.rollup_cfg.is_ecotone_active(next_l2_time) {
            // if the parent beacon root is not available, default to zero hash
            parent_beacon_root = Some(l1_header.parent_beacon_block_root.unwrap_or_default());
        }

        tracing::debug!(
            target: "attributes_builder",
            "Using L1 header for payload attributes: number={}, hash={:?}, mix_hash={:?}",
            l1_header.number,
            l1_header.hash_slow(),
            l1_header.mix_hash
        );
        
        Ok(OpPayloadAttributes {
            payload_attributes: PayloadAttributes {
                timestamp: next_l2_time,
                prev_randao: l1_header.mix_hash,
                suggested_fee_recipient: Address::ZERO, // Facet uses zero address as beneficiary
                parent_beacon_block_root: parent_beacon_root,
                withdrawals,
            },
            transactions: Some(txs),
            no_tx_pool: Some(true),
            gas_limit: Some(u64::from_be_bytes(
                alloy_primitives::U64::from(sys_config.gas_limit).to_be_bytes(),
            )),
            eip_1559_params: sys_config.eip_1559_params(
                &self.rollup_cfg,
                l2_parent.block_info.timestamp,
                next_l2_time,
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        errors::ResetError,
        test_utils::{TestChainProvider, TestSystemConfigL2Fetcher},
    };
    use alloc::vec;
    use alloy_consensus::Header;
    use alloy_primitives::{B256, Log, LogData, U64, U256, address};
    use kona_genesis::{HardForkConfig, SystemConfig};
    use kona_protocol::{BlockInfo, DepositError};

    fn generate_valid_log() -> Log {
        let deposit_contract = address!("1111111111111111111111111111111111111111");
        let mut data = vec![0u8; 192];
        let offset: [u8; 8] = U64::from(32).to_be_bytes();
        data[24..32].copy_from_slice(&offset);
        let len: [u8; 8] = U64::from(128).to_be_bytes();
        data[56..64].copy_from_slice(&len);
        // Copy the u128 mint value
        let mint: [u8; 16] = 10_u128.to_be_bytes();
        data[80..96].copy_from_slice(&mint);
        // Copy the tx value
        let value: [u8; 32] = U256::from(100).to_be_bytes();
        data[96..128].copy_from_slice(&value);
        // Copy the gas limit
        let gas: [u8; 8] = 1000_u64.to_be_bytes();
        data[128..136].copy_from_slice(&gas);
        // Copy the isCreation flag
        data[136] = 1;
        let from = address!("2222222222222222222222222222222222222222");
        let mut from_bytes = vec![0u8; 32];
        from_bytes[12..32].copy_from_slice(from.as_slice());
        let to = address!("3333333333333333333333333333333333333333");
        let mut to_bytes = vec![0u8; 32];
        to_bytes[12..32].copy_from_slice(to.as_slice());
        Log {
            address: deposit_contract,
            data: LogData::new_unchecked(
                vec![
                    DEPOSIT_EVENT_ABI_HASH,
                    B256::from_slice(&from_bytes),
                    B256::from_slice(&to_bytes),
                    B256::default(),
                ],
                Bytes::from(data),
            ),
        }
    }

    fn generate_valid_receipt() -> Receipt {
        let mut bad_dest_log = generate_valid_log();
        bad_dest_log.data.topics_mut()[1] = B256::default();
        let mut invalid_topic_log = generate_valid_log();
        invalid_topic_log.data.topics_mut()[0] = B256::default();
        Receipt {
            status: Eip658Value::Eip658(true),
            logs: vec![generate_valid_log(), bad_dest_log, invalid_topic_log],
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn test_prepare_payload_block_mismatch_epoch_reset() {
        let cfg = Arc::new(RollupConfig::default());
        let l2_number = 1;
        let mut fetcher = TestSystemConfigL2Fetcher::default();
        fetcher.insert(l2_number, SystemConfig::default());
        let mut provider = TestChainProvider::default();
        let header = Header::default();
        let hash = header.hash_slow();
        provider.insert_header(hash, header);
        let mut builder = StatefulAttributesBuilder::new(cfg, fetcher, provider);
        let epoch = BlockNumHash { hash, number: l2_number };
        let l2_parent = L2BlockInfo {
            block_info: BlockInfo { hash: B256::ZERO, number: l2_number, ..Default::default() },
            l1_origin: BlockNumHash { hash: B256::left_padding_from(&[0xFF]), number: 2 },
            seq_num: 0,
        };
        // This should error because the l2 parent's l1_origin.hash should equal the epoch header
        // hash. Here we use the default header whose hash will not equal the custom `l2_hash`.
        let expected =
            BuilderError::BlockMismatchEpochReset(epoch, l2_parent.l1_origin, B256::default());
        let err = builder.prepare_payload_attributes(l2_parent, epoch).await.unwrap_err();
        assert_eq!(err, PipelineErrorKind::Reset(expected.into()));
    }

    #[tokio::test]
    async fn test_prepare_payload_block_mismatch() {
        let cfg = Arc::new(RollupConfig::default());
        let l2_number = 1;
        let mut fetcher = TestSystemConfigL2Fetcher::default();
        fetcher.insert(l2_number, SystemConfig::default());
        let mut provider = TestChainProvider::default();
        let header = Header::default();
        let hash = header.hash_slow();
        provider.insert_header(hash, header);
        let mut builder = StatefulAttributesBuilder::new(cfg, fetcher, provider);
        let epoch = BlockNumHash { hash, number: l2_number };
        let l2_parent = L2BlockInfo {
            block_info: BlockInfo { hash: B256::ZERO, number: l2_number, ..Default::default() },
            l1_origin: BlockNumHash { hash: B256::ZERO, number: l2_number },
            seq_num: 0,
        };
        // This should error because the l2 parent's l1_origin.hash should equal the epoch hash
        // Here the default header is used whose hash will not equal the custom `l2_hash` above.
        let expected = BuilderError::BlockMismatch(epoch, l2_parent.l1_origin);
        let err = builder.prepare_payload_attributes(l2_parent, epoch).await.unwrap_err();
        assert_eq!(err, PipelineErrorKind::Reset(ResetError::AttributesBuilder(expected)));
    }

    #[tokio::test]
    async fn test_prepare_payload_broken_time_invariant() {
        let block_time = 10;
        let timestamp = 100;
        let cfg = Arc::new(RollupConfig { block_time, ..Default::default() });
        let l2_number = 1;
        let mut fetcher = TestSystemConfigL2Fetcher::default();
        fetcher.insert(l2_number, SystemConfig::default());
        let mut provider = TestChainProvider::default();
        let header = Header { timestamp, ..Default::default() };
        let hash = header.hash_slow();
        provider.insert_header(hash, header);
        let mut builder = StatefulAttributesBuilder::new(cfg, fetcher, provider);
        let epoch = BlockNumHash { hash, number: l2_number };
        let l2_parent = L2BlockInfo {
            block_info: BlockInfo { hash: B256::ZERO, number: l2_number, ..Default::default() },
            l1_origin: BlockNumHash { hash, number: l2_number },
            seq_num: 0,
        };
        let next_l2_time = l2_parent.block_info.timestamp + block_time;
        let block_id = BlockNumHash { hash, number: 0 };
        let expected = BuilderError::BrokenTimeInvariant(
            l2_parent.l1_origin,
            next_l2_time,
            block_id,
            timestamp,
        );
        let err = builder.prepare_payload_attributes(l2_parent, epoch).await.unwrap_err();
        assert_eq!(err, PipelineErrorKind::Reset(ResetError::AttributesBuilder(expected)));
    }

    #[tokio::test]
    async fn test_prepare_payload_without_forks() {
        let block_time = 10;
        let timestamp = 100;
        let cfg = Arc::new(RollupConfig { block_time, ..Default::default() });
        let l2_number = 1;
        let mut fetcher = TestSystemConfigL2Fetcher::default();
        fetcher.insert(l2_number, SystemConfig::default());
        let mut provider = TestChainProvider::default();
        let header = Header { timestamp, ..Default::default() };
        let prev_randao = header.mix_hash;
        let hash = header.hash_slow();
        provider.insert_header(hash, header);
        let mut builder = StatefulAttributesBuilder::new(cfg, fetcher, provider);
        let epoch = BlockNumHash { hash, number: l2_number };
        let l2_parent = L2BlockInfo {
            block_info: BlockInfo {
                hash: B256::ZERO,
                number: l2_number,
                timestamp,
                parent_hash: hash,
            },
            l1_origin: BlockNumHash { hash, number: l2_number },
            seq_num: 0,
        };
        let next_l2_time = l2_parent.block_info.timestamp + block_time;
        let payload = builder.prepare_payload_attributes(l2_parent, epoch).await.unwrap();
        let expected = OpPayloadAttributes {
            payload_attributes: PayloadAttributes {
                timestamp: next_l2_time,
                prev_randao,
                suggested_fee_recipient: Predeploys::SEQUENCER_FEE_VAULT,
                parent_beacon_block_root: None,
                withdrawals: None,
            },
            transactions: payload.transactions.clone(),
            no_tx_pool: Some(true),
            gas_limit: Some(u64::from_be_bytes(
                alloy_primitives::U64::from(SystemConfig::default().gas_limit).to_be_bytes(),
            )),
            eip_1559_params: None,
        };
        assert_eq!(payload, expected);
        assert_eq!(payload.transactions.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn test_prepare_payload_with_canyon() {
        let block_time = 10;
        let timestamp = 100;
        let cfg = Arc::new(RollupConfig {
            block_time,
            hardforks: HardForkConfig { canyon_time: Some(0), ..Default::default() },
            ..Default::default()
        });
        let l2_number = 1;
        let mut fetcher = TestSystemConfigL2Fetcher::default();
        fetcher.insert(l2_number, SystemConfig::default());
        let mut provider = TestChainProvider::default();
        let header = Header { timestamp, ..Default::default() };
        let prev_randao = header.mix_hash;
        let hash = header.hash_slow();
        provider.insert_header(hash, header);
        let mut builder = StatefulAttributesBuilder::new(cfg, fetcher, provider);
        let epoch = BlockNumHash { hash, number: l2_number };
        let l2_parent = L2BlockInfo {
            block_info: BlockInfo {
                hash: B256::ZERO,
                number: l2_number,
                timestamp,
                parent_hash: hash,
            },
            l1_origin: BlockNumHash { hash, number: l2_number },
            seq_num: 0,
        };
        let next_l2_time = l2_parent.block_info.timestamp + block_time;
        let payload = builder.prepare_payload_attributes(l2_parent, epoch).await.unwrap();
        let expected = OpPayloadAttributes {
            payload_attributes: PayloadAttributes {
                timestamp: next_l2_time,
                prev_randao,
                suggested_fee_recipient: Predeploys::SEQUENCER_FEE_VAULT,
                parent_beacon_block_root: None,
                withdrawals: Some(Vec::default()),
            },
            transactions: payload.transactions.clone(),
            no_tx_pool: Some(true),
            gas_limit: Some(u64::from_be_bytes(
                alloy_primitives::U64::from(SystemConfig::default().gas_limit).to_be_bytes(),
            )),
            eip_1559_params: None,
        };
        assert_eq!(payload, expected);
        assert_eq!(payload.transactions.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn test_prepare_payload_with_ecotone() {
        let block_time = 2;
        let timestamp = 100;
        let cfg = Arc::new(RollupConfig {
            block_time,
            hardforks: HardForkConfig { ecotone_time: Some(102), ..Default::default() },
            ..Default::default()
        });
        let l2_number = 1;
        let mut fetcher = TestSystemConfigL2Fetcher::default();
        fetcher.insert(l2_number, SystemConfig::default());
        let mut provider = TestChainProvider::default();
        let header = Header { timestamp, ..Default::default() };
        let parent_beacon_block_root = Some(header.parent_beacon_block_root.unwrap_or_default());
        let prev_randao = header.mix_hash;
        let hash = header.hash_slow();
        provider.insert_header(hash, header);
        let mut builder = StatefulAttributesBuilder::new(cfg, fetcher, provider);
        let epoch = BlockNumHash { hash, number: l2_number };
        let l2_parent = L2BlockInfo {
            block_info: BlockInfo {
                hash: B256::ZERO,
                number: l2_number,
                timestamp,
                parent_hash: hash,
            },
            l1_origin: BlockNumHash { hash, number: l2_number },
            seq_num: 0,
        };
        let next_l2_time = l2_parent.block_info.timestamp + block_time;
        let payload = builder.prepare_payload_attributes(l2_parent, epoch).await.unwrap();
        let expected = OpPayloadAttributes {
            payload_attributes: PayloadAttributes {
                timestamp: next_l2_time,
                prev_randao,
                suggested_fee_recipient: Predeploys::SEQUENCER_FEE_VAULT,
                parent_beacon_block_root,
                withdrawals: Some(vec![]),
            },
            transactions: payload.transactions.clone(),
            no_tx_pool: Some(true),
            gas_limit: Some(u64::from_be_bytes(
                alloy_primitives::U64::from(SystemConfig::default().gas_limit).to_be_bytes(),
            )),
            eip_1559_params: None,
        };
        assert_eq!(payload, expected);
        assert_eq!(payload.transactions.unwrap().len(), 7);
    }

    #[tokio::test]
    async fn test_prepare_payload_with_fjord() {
        let block_time = 2;
        let timestamp = 100;
        let cfg = Arc::new(RollupConfig {
            block_time,
            hardforks: HardForkConfig { fjord_time: Some(102), ..Default::default() },
            ..Default::default()
        });
        let l2_number = 1;
        let mut fetcher = TestSystemConfigL2Fetcher::default();
        fetcher.insert(l2_number, SystemConfig::default());
        let mut provider = TestChainProvider::default();
        let header = Header { timestamp, ..Default::default() };
        let prev_randao = header.mix_hash;
        let hash = header.hash_slow();
        provider.insert_header(hash, header);
        let mut builder = StatefulAttributesBuilder::new(cfg, fetcher, provider);
        let epoch = BlockNumHash { hash, number: l2_number };
        let l2_parent = L2BlockInfo {
            block_info: BlockInfo {
                hash: B256::ZERO,
                number: l2_number,
                timestamp,
                parent_hash: hash,
            },
            l1_origin: BlockNumHash { hash, number: l2_number },
            seq_num: 0,
        };
        let next_l2_time = l2_parent.block_info.timestamp + block_time;
        let payload = builder.prepare_payload_attributes(l2_parent, epoch).await.unwrap();
        let expected = OpPayloadAttributes {
            payload_attributes: PayloadAttributes {
                timestamp: next_l2_time,
                prev_randao,
                suggested_fee_recipient: Predeploys::SEQUENCER_FEE_VAULT,
                parent_beacon_block_root: Some(B256::ZERO),
                withdrawals: Some(vec![]),
            },
            transactions: payload.transactions.clone(),
            no_tx_pool: Some(true),
            gas_limit: Some(u64::from_be_bytes(
                alloy_primitives::U64::from(SystemConfig::default().gas_limit).to_be_bytes(),
            )),
            eip_1559_params: None,
        };
        assert_eq!(payload.transactions.as_ref().unwrap().len(), 10);
        assert_eq!(payload, expected);
    }
}
