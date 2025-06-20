//! Test utilities for the executor.

use crate::{StatelessL2Builder, TrieDBProvider};
use alloy_consensus::Header;
use alloy_op_evm::OpEvmFactory;
use alloy_primitives::{B256, Bytes, Sealable};
use alloy_provider::{Provider, RootProvider, network::primitives::BlockTransactions};
use alloy_rlp::Decodable;
use alloy_rpc_client::RpcClient;
use alloy_rpc_types_engine::PayloadAttributes;
use alloy_transport_http::{Client, Http};
use kona_genesis::RollupConfig;
use kona_mpt::{NoopTrieHinter, TrieNode, TrieProvider};
use kona_registry::ROLLUP_CONFIGS;
use kona_genesis::{BaseFeeConfig, ChainGenesis, HardForkConfig, SystemConfig};
use alloy_eips::BlockNumHash;
use alloy_primitives::{address, b256, U256};
use op_alloy_rpc_types_engine::OpPayloadAttributes;
use rocksdb::{DB, Options};
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::Arc};
use tokio::{fs, runtime::Handle, sync::Mutex};

/// Executes a [ExecutorTestFixture] stored at the passed `fixture_path` and asserts that the
/// produced block hash matches the expected block hash.
pub async fn run_test_fixture(fixture_path: PathBuf) {
    // First, untar the fixture.
    let fixture_dir = tempfile::tempdir().expect("Failed to create temporary directory");
    tokio::process::Command::new("tar")
        .arg("-xvf")
        .arg(fixture_path.as_path())
        .arg("-C")
        .arg(fixture_dir.path())
        .arg("--strip-components=1")
        .output()
        .await
        .expect("Failed to untar fixture");

    let mut options = Options::default();
    options.set_compression_type(rocksdb::DBCompressionType::Snappy);
    options.create_if_missing(true);
    let kv_store = DB::open(&options, fixture_dir.path().join("kv"))
        .unwrap_or_else(|e| panic!("Failed to open database at {fixture_dir:?}: {e}"));
    let provider = DiskTrieNodeProvider::new(kv_store);
    let fixture: ExecutorTestFixture =
        serde_json::from_slice(&fs::read(fixture_dir.path().join("fixture.json")).await.unwrap())
            .expect("Failed to deserialize fixture");

    let mut executor = StatelessL2Builder::new(
        &fixture.rollup_config,
        OpEvmFactory::default(),
        provider,
        NoopTrieHinter,
        fixture.parent_header.seal_slow(),
    );

    let outcome = executor.build_block(fixture.executing_payload).unwrap();

    assert_eq!(
        outcome.header.hash(),
        fixture.expected_block_hash,
        "Produced header does not match the expected header"
    );
}

/// The test fixture format for the [`StatelessL2Builder`].
#[derive(Debug, Serialize, Deserialize)]
pub struct ExecutorTestFixture {
    /// The rollup configuration for the executing chain.
    pub rollup_config: RollupConfig,
    /// The parent block header.
    pub parent_header: Header,
    /// The executing payload attributes.
    pub executing_payload: OpPayloadAttributes,
    /// The expected block hash
    pub expected_block_hash: B256,
}

/// A test fixture creator for the [`StatelessL2Builder`].
#[derive(Debug)]
pub struct ExecutorTestFixtureCreator {
    /// The RPC provider for the L2 execution layer.
    pub provider: RootProvider,
    /// The block number to create the test fixture for.
    pub block_number: u64,
    /// The key value store for the test fixture.
    pub kv_store: Arc<Mutex<rocksdb::DB>>,
    /// The data directory for the test fixture.
    pub data_dir: PathBuf,
}

impl ExecutorTestFixtureCreator {
    /// Creates a new [`ExecutorTestFixtureCreator`] with the given parameters.
    pub fn new(provider_url: &str, block_number: u64, base_fixture_directory: PathBuf) -> Self {
        let base = base_fixture_directory.join(format!("block-{}", block_number));

        let url = provider_url.parse().expect("Invalid provider URL");
        let http = Http::<Client>::new(url);
        let provider = RootProvider::new(RpcClient::new(http, false));

        let mut options = Options::default();
        options.set_compression_type(rocksdb::DBCompressionType::Snappy);
        options.create_if_missing(true);
        let db = DB::open(&options, base.join("kv").as_path())
            .unwrap_or_else(|e| panic!("Failed to open database at {base:?}: {e}"));

        Self { provider, block_number, kv_store: Arc::new(Mutex::new(db)), data_dir: base }
    }
}

impl ExecutorTestFixtureCreator {
    /// Create a static test fixture with the configuration provided.
    pub async fn create_static_fixture(self) {
        let chain_id = self.provider.get_chain_id().await.expect("Failed to get chain ID");
        let rollup_config = ROLLUP_CONFIGS.get(&chain_id)
            .cloned()
            .unwrap_or_else(|| {
                println!("⚠️  Chain ID {} not found in registry, using custom facet config", chain_id);
                create_custom_facet_config(chain_id)
            });

        let executing_block = self
            .provider
            .get_block_by_number(self.block_number.into())
            .await
            .expect("Failed to get parent block")
            .expect("Block not found");
        let parent_block = self
            .provider
            .get_block_by_number((self.block_number - 1).into())
            .await
            .expect("Failed to get parent block")
            .expect("Block not found");

        let executing_header = executing_block.header;
        let parent_header = parent_block.header.inner.seal_slow();

        let encoded_executing_transactions = match executing_block.transactions {
            BlockTransactions::Hashes(transactions) => {
                let mut encoded_transactions = Vec::with_capacity(transactions.len());
                for (i, tx_hash) in transactions.iter().enumerate() {
                    let tx = self
                        .provider
                        .client()
                        .request::<&[B256; 1], Bytes>("debug_getRawTransaction", &[*tx_hash])
                        .await
                        .expect("Block not found");
                    
                    // Debug logging
                    println!("=== GETH Transaction {} (hash: {}) ===", i, tx_hash);
                    println!("  Raw length: {} bytes", tx.len());
                    println!("  First 40 bytes: {:02x?}", &tx[..40.min(tx.len())]);
                    
                    // Try to identify transaction type
                    if tx.len() > 0 {
                        match tx[0] {
                            0x7e => {
                                println!("  Type: DEPOSIT (0x7e)");
                                // Also fetch the transaction details to see mint value
                                let tx_details: serde_json::Value = self
                                    .provider
                                    .client()
                                    .request("eth_getTransactionByHash", &[*tx_hash])
                                    .await
                                    .expect("Failed to get transaction details");
                                if let Some(mint) = tx_details.get("mint") {
                                    println!("  Mint value from Geth: {}", mint);
                                }
                            },
                            0x02 => println!("  Type: EIP-1559 (0x02)"),
                            0x01 => println!("  Type: EIP-2930 (0x01)"),
                            _ => println!("  Type: Unknown or Legacy (0x{:02x})", tx[0]),
                        }
                    }
                    
                    encoded_transactions.push(tx);
                }
                println!("=== Total transactions from Geth: {} ===\n", encoded_transactions.len());
                encoded_transactions
            }
            _ => panic!("Only BlockTransactions::Hashes are supported."),
        };

        let payload_attrs = OpPayloadAttributes {
            payload_attributes: PayloadAttributes {
                timestamp: executing_header.timestamp,
                parent_beacon_block_root: executing_header.parent_beacon_block_root,
                prev_randao: executing_header.mix_hash,
                withdrawals: Default::default(),
                suggested_fee_recipient: executing_header.beneficiary,
            },
            gas_limit: Some(executing_header.gas_limit),
            transactions: Some(encoded_executing_transactions),
            no_tx_pool: None,
            eip_1559_params: rollup_config.is_holocene_active(executing_header.timestamp).then(
                || {
                    executing_header.extra_data[1..]
                        .try_into()
                        .expect("Invalid header format for Holocene")
                },
            ),
        };

        let fixture_path = self.data_dir.join("fixture.json");
        let fixture = ExecutorTestFixture {
            rollup_config: rollup_config.clone(),
            parent_header: parent_header.inner().clone(),
            executing_payload: payload_attrs.clone(),
            expected_block_hash: executing_header.hash_slow(),
        };

        let mut executor = StatelessL2Builder::new(
            &rollup_config,
            OpEvmFactory::default(),
            self,
            NoopTrieHinter,
            parent_header,
        );
        let outcome = executor.build_block(payload_attrs).expect("Failed to execute block");

        // Debug: Print execution details
        println!("\n=== Execution Results ===");
        println!("Gas used: {} (expected: {})", outcome.execution_result.gas_used, executing_header.gas_used);
        println!("Receipts count: {}", outcome.execution_result.receipts.len());
        
        // Print receipt details to see gas usage per transaction
        for (i, receipt) in outcome.execution_result.receipts.iter().enumerate() {
            println!("\nReceipt {}: {:?}", i, receipt);
        }
        
        // Print state root comparison
        println!("\n=== State Root Comparison ===");
        println!("Kona state root:  {:?}", outcome.header.state_root);
        println!("Geth state root:  {:?}", executing_header.state_root);
        
        assert_eq!(
            outcome.header.inner(),
            &executing_header.inner,
            "Produced header does not match the expected header"
        );
        fs::write(fixture_path.as_path(), serde_json::to_vec(&fixture).unwrap()).await.unwrap();

        // Tar the fixture.
        let data_dir = fixture_path.parent().unwrap();
        tokio::process::Command::new("tar")
            .arg("-czf")
            .arg(data_dir.with_extension("tar.gz").file_name().unwrap())
            .arg(data_dir.file_name().unwrap())
            .current_dir(data_dir.parent().unwrap())
            .output()
            .await
            .expect("Failed to tar fixture");

        // Remove the leftover directory.
        fs::remove_dir_all(data_dir).await.expect("Failed to remove temporary directory");
    }
}

impl TrieProvider for ExecutorTestFixtureCreator {
    type Error = TestTrieNodeProviderError;

    fn trie_node_by_hash(&self, key: B256) -> Result<TrieNode, Self::Error> {
        // Fetch the preimage from the L2 chain provider.
        let preimage: Bytes = tokio::task::block_in_place(move || {
            Handle::current().block_on(async {
                let preimage: Bytes = self
                    .provider
                    .client()
                    .request("debug_dbGet", &[key])
                    .await
                    .map_err(|_| TestTrieNodeProviderError::PreimageNotFound)?;

                self.kv_store
                    .lock()
                    .await
                    .put(key, preimage.clone())
                    .map_err(|_| TestTrieNodeProviderError::KVStore)?;

                Ok(preimage)
            })
        })?;

        // Decode the preimage into a trie node.
        TrieNode::decode(&mut preimage.as_ref()).map_err(TestTrieNodeProviderError::Rlp)
    }
}

impl TrieDBProvider for ExecutorTestFixtureCreator {
    fn bytecode_by_hash(&self, hash: B256) -> Result<Bytes, Self::Error> {
        // geth hashdb scheme code hash key prefix
        const CODE_PREFIX: u8 = b'c';

        // Fetch the preimage from the L2 chain provider.
        let preimage: Bytes = tokio::task::block_in_place(move || {
            Handle::current().block_on(async {
                // Attempt to fetch the code from the L2 chain provider.
                let code_hash = [&[CODE_PREFIX], hash.as_slice()].concat();
                let code = self
                    .provider
                    .client()
                    .request::<&[Bytes; 1], Bytes>("debug_dbGet", &[code_hash.into()])
                    .await;

                // Check if the first attempt to fetch the code failed. If it did, try fetching the
                // code hash preimage without the geth hashdb scheme prefix.
                let code = match code {
                    Ok(code) => code,
                    Err(_) => self
                        .provider
                        .client()
                        .request::<&[B256; 1], Bytes>("debug_dbGet", &[hash])
                        .await
                        .map_err(|_| TestTrieNodeProviderError::PreimageNotFound)?,
                };

                self.kv_store
                    .lock()
                    .await
                    .put(hash, code.clone())
                    .map_err(|_| TestTrieNodeProviderError::KVStore)?;

                Ok(code)
            })
        })?;

        Ok(preimage)
    }

    fn header_by_hash(&self, hash: B256) -> Result<Header, Self::Error> {
        let encoded_header: Bytes = tokio::task::block_in_place(move || {
            Handle::current().block_on(async {
                let preimage: Bytes = self
                    .provider
                    .client()
                    .request("debug_getRawHeader", &[hash])
                    .await
                    .map_err(|_| TestTrieNodeProviderError::PreimageNotFound)?;

                self.kv_store
                    .lock()
                    .await
                    .put(hash, preimage.clone())
                    .map_err(|_| TestTrieNodeProviderError::KVStore)?;

                Ok(preimage)
            })
        })?;

        // Decode the Header.
        Header::decode(&mut encoded_header.as_ref()).map_err(TestTrieNodeProviderError::Rlp)
    }
}

/// A simple [`TrieDBProvider`] that reads data from a disk-based key-value store.
#[derive(Debug)]
pub struct DiskTrieNodeProvider {
    kv_store: DB,
}

impl DiskTrieNodeProvider {
    /// Creates a new [`DiskTrieNodeProvider`] with the given [`rocksdb`] K/V store.
    pub const fn new(kv_store: DB) -> Self {
        Self { kv_store }
    }
}

impl TrieProvider for DiskTrieNodeProvider {
    type Error = TestTrieNodeProviderError;

    fn trie_node_by_hash(&self, key: B256) -> Result<TrieNode, Self::Error> {
        TrieNode::decode(
            &mut self
                .kv_store
                .get(key)
                .map_err(|_| TestTrieNodeProviderError::PreimageNotFound)?
                .ok_or(TestTrieNodeProviderError::PreimageNotFound)?
                .as_slice(),
        )
        .map_err(TestTrieNodeProviderError::Rlp)
    }
}

impl TrieDBProvider for DiskTrieNodeProvider {
    fn bytecode_by_hash(&self, code_hash: B256) -> Result<Bytes, Self::Error> {
        self.kv_store
            .get(code_hash)
            .map_err(|_| TestTrieNodeProviderError::PreimageNotFound)?
            .map(Bytes::from)
            .ok_or(TestTrieNodeProviderError::PreimageNotFound)
    }

    fn header_by_hash(&self, hash: B256) -> Result<Header, Self::Error> {
        Header::decode(
            &mut self
                .kv_store
                .get(hash)
                .map_err(|_| TestTrieNodeProviderError::PreimageNotFound)?
                .ok_or(TestTrieNodeProviderError::PreimageNotFound)?
                .as_slice(),
        )
        .map_err(TestTrieNodeProviderError::Rlp)
    }
}

/// An error type for the [`DiskTrieNodeProvider`] and [`ExecutorTestFixtureCreator`].
#[derive(Debug, thiserror::Error)]
pub enum TestTrieNodeProviderError {
    /// The preimage was not found in the key-value store.
    #[error("Preimage not found")]
    PreimageNotFound,
    /// Failed to decode the RLP-encoded data.
    #[error("Failed to decode RLP: {0}")]
    Rlp(alloy_rlp::Error),
    /// Failed to write back to the key-value store.
    #[error("Failed to write back to key value store")]
    KVStore,
}

/// Creates a custom rollup config for the facet chain when not found in registry
fn create_custom_facet_config(chain_id: u64) -> RollupConfig {
    RollupConfig {
        genesis: ChainGenesis {
            l1: BlockNumHash {
                hash: b256!("0x481724ee99b1f4cb71d826e2ec5a37265f460e9b112315665c977f4050b0af54"),
                number: 10,
            },
            l2: BlockNumHash {
                hash: b256!("0x88aedfbf7dea6bfa2c4ff315784ad1a7f145d8f650969359c003bbed68c87631"),
                number: 0,
            },
            l2_time: 1725557164,
            system_config: Some(SystemConfig {
                batcher_address: address!("c81f87a644b41e49b3221f41251f15c6cb00ce03"),
                overhead: U256::ZERO,
                scalar: U256::from(1_000_000u64),
                gas_limit: 30_000_000,
                base_fee_scalar: Some(1368),
                blob_base_fee_scalar: Some(810949),
                ..Default::default()
            }),
        },
        l1_chain_id: 1, // Ethereum mainnet
        l2_chain_id: chain_id,
        block_time: 12,
        max_sequencer_drift: 600,
        seq_window_size: 3600,
        channel_timeout: 300,
        hardforks: HardForkConfig {
            regolith_time: Some(0),
            canyon_time: Some(0),
            delta_time: Some(0),
            ecotone_time: Some(0),
            fjord_time: Some(0),
            isthmus_time: None,
            ..Default::default()
        },
        batch_inbox_address: address!("ff00000000000000000000000000000000042069"),
        deposit_contract_address: address!("08073dc48dde578137b8af042bcbc1c2491f1eb2"),
        l1_system_config_address: address!("94ee52a9d8edd72a85dea7fae3ba6d75e4bf1710"),
        protocol_versions_address: address!("0000000000000000000000000000000000000000"),
        superchain_config_address: Some(address!("0000000000000000000000000000000000000000")),
        da_challenge_address: Some(address!("0000000000000000000000000000000000000000")),
        blobs_enabled_l1_timestamp: None,
        granite_channel_timeout: 50,
        interop_message_expiry_window: 3600,
        alt_da_config: None,
        chain_op_config: BaseFeeConfig {
            eip1559_elasticity: 2,
            eip1559_denominator: 8,
            eip1559_denominator_canyon: 8,
        },
    }
}
