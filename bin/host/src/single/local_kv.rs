//! Contains a concrete implementation of the [KeyValueStore] trait that stores data on disk,
//! using the [SingleChainHost] config.

use super::SingleChainHost;
use crate::KeyValueStore;
use alloy_primitives::B256;
use anyhow::Result;
use kona_preimage::PreimageKey;
use kona_proof::boot::{
    L1_HEAD_KEY, L2_CHAIN_ID_KEY, L2_CLAIM_BLOCK_NUMBER_KEY, L2_CLAIM_KEY, L2_OUTPUT_ROOT_KEY,
    L2_ROLLUP_CONFIG_KEY,
};
use tracing::{error, trace};

/// A simple, synchronous key-value store that returns data from a [SingleChainHost] config.
#[derive(Debug)]
pub struct SingleChainLocalInputs {
    cfg: SingleChainHost,
}

impl SingleChainLocalInputs {
    /// Create a new [SingleChainLocalInputs] with the given [SingleChainHost] config.
    pub const fn new(cfg: SingleChainHost) -> Self {
        Self { cfg }
    }
}

impl KeyValueStore for SingleChainLocalInputs {
    fn get(&self, key: B256) -> Option<Vec<u8>> {
        let preimage_key = PreimageKey::try_from(*key).ok()?;
        match preimage_key.key_value() {
            L1_HEAD_KEY => Some(self.cfg.l1_head.to_vec()),
            L2_OUTPUT_ROOT_KEY => Some(self.cfg.agreed_l2_output_root.to_vec()),
            L2_CLAIM_KEY => Some(self.cfg.claimed_l2_output_root.to_vec()),
            L2_CLAIM_BLOCK_NUMBER_KEY => {
                Some(self.cfg.claimed_l2_block_number.to_be_bytes().to_vec())
            }
            L2_CHAIN_ID_KEY => {
                trace!(target: "local_kv", "L2_CHAIN_ID_KEY requested");
                // If l2_chain_id is set directly, use it
                if let Some(chain_id) = self.cfg.l2_chain_id {
                    trace!(target: "local_kv", "Using direct l2_chain_id: {}", chain_id);
                    Some(chain_id.to_be_bytes().to_vec())
                } else if self.cfg.rollup_config_path.is_some() {
                    // If using rollup config path, extract chain ID from the rollup config
                    trace!(target: "local_kv", "Reading rollup config to get chain ID");
                    match self.cfg.read_rollup_config() {
                        Ok(rollup_config) => {
                            trace!(target: "local_kv", "Successfully read rollup config, chain ID: {}", rollup_config.l2_chain_id);
                            Some(rollup_config.l2_chain_id.to_be_bytes().to_vec())
                        }
                        Err(e) => {
                            error!(target: "local_kv", "Failed to read rollup config: {:?}", e);
                            None
                        }
                    }
                } else {
                    // Default to 0 if neither is set
                    trace!(target: "local_kv", "No chain ID source, defaulting to 0");
                    Some(0u64.to_be_bytes().to_vec())
                }
            }
            L2_ROLLUP_CONFIG_KEY => {
                trace!(target: "local_kv", "Reading rollup config for L2_ROLLUP_CONFIG_KEY");
                let rollup_config = match self.cfg.read_rollup_config() {
                    Ok(config) => config,
                    Err(e) => {
                        error!(target: "local_kv", "Failed to read rollup config: {}", e);
                        return None;
                    }
                };
                
                match serde_json::to_vec(&rollup_config) {
                    Ok(serialized) => {
                        trace!(target: "local_kv", "Successfully serialized rollup config, size: {} bytes", serialized.len());
                        Some(serialized)
                    }
                    Err(e) => {
                        error!(target: "local_kv", "Failed to serialize rollup config: {}", e);
                        None
                    }
                }
            }
            _ => None,
        }
    }

    fn set(&mut self, _: B256, _: Vec<u8>) -> Result<()> {
        unreachable!("LocalKeyValueStore is read-only")
    }
}
