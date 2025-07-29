//! Contains facet-specific L1 block info types.

use crate::DecodeError;
use alloc::vec::Vec;
use alloy_primitives::{Address, B256, Bytes, U256};

/// Represents the fields within a Facet L1 block info transaction.
///
/// Facet Binary Format (extends Ecotone)
/// +---------+----------------------------------------------+
/// | Bytes   | Field                                        |
/// +---------+----------------------------------------------+
/// | 4       | Function signature                           |
/// | 4       | BaseFeeScalar                                |
/// | 4       | BlobBaseFeeScalar                            |
/// | 8       | SequenceNumber                               |
/// | 8       | Timestamp                                    |
/// | 8       | L1BlockNumber                                |
/// | 32      | BaseFee                                      |
/// | 32      | BlobBaseFee                                  |
/// | 32      | BlockHash                                    |
/// | 32      | BatcherHash                                  |
/// | 16      | FctMintPeriodL1DataGas (deprecated)          |
/// | 16      | FctMintRate                                  |
/// | 16      | FctPeriodStartBlock                          |
/// | 16      | FctTotalMinted                               |
/// | 16      | FctMaxSupply                                 |
/// | 16      | FctPeriodMinted                              |
/// | 32      | FctInitialTargetPerPeriod                    |
/// +---------+----------------------------------------------+
#[derive(Debug, Clone, Hash, Eq, PartialEq, Default, Copy)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct L1BlockInfoFacet {
    /// The current L1 origin block number
    pub number: u64,
    /// The current L1 origin block's timestamp
    pub time: u64,
    /// The current L1 origin block's basefee
    pub base_fee: u64,
    /// The current L1 origin block's hash
    pub block_hash: B256,
    /// The current sequence number
    pub sequence_number: u64,
    /// The address of the batch submitter
    pub batcher_address: Address,
    /// The current blob base fee on L1
    pub blob_base_fee: u128,
    /// The fee scalar for L1 blobspace data
    pub blob_base_fee_scalar: u32,
    /// The fee scalar for L1 data
    pub base_fee_scalar: u32,
    /// Indicates that the scalars are empty.
    /// This is an edge case where the first block in ecotone has no scalars,
    /// so the bedrock tx l1 cost function needs to be used.
    pub empty_scalars: bool,
    /// The l1 fee overhead used along with the `empty_scalars` field for the
    /// bedrock tx l1 cost function.
    ///
    /// This field is deprecated in the Ecotone Hardfork.
    pub l1_fee_overhead: U256,
    /// The facet mint rate (wei/gas)
    pub fct_mint_rate: u128,
    /// Total FCT minted across all periods
    pub fct_total_minted: u128,
    /// Block number when current period started
    pub fct_period_start_block: u128,
    /// FCT minted in current period
    pub fct_period_minted: u128,
    /// Maximum supply of FCT tokens
    pub fct_max_supply: u128,
    /// Initial target FCT to mint per period
    pub fct_initial_target_per_period: u128,
}

impl L1BlockInfoFacet {
    /// The type byte identifier for the L1 scalar format in Facet.
    pub const L1_SCALAR: u8 = 1;

    /// The length of an L1 info transaction in Facet.
    /// 4 (selector) + 32*5 (base fields) + 32*4 (FCT words) = 292 bytes
    pub const L1_INFO_TX_LEN: usize = 292;

    /// The 4 byte selector of "setL1BlockValuesEcotone()"
    pub const L1_INFO_TX_SELECTOR: [u8; 4] = [0x44, 0x0a, 0x5e, 0x20];

    /// Encodes the [L1BlockInfoFacet] object into Ethereum transaction calldata.
    pub fn encode_calldata(&self) -> Bytes {
        let mut buf = Vec::with_capacity(Self::L1_INFO_TX_LEN);
        buf.extend_from_slice(Self::L1_INFO_TX_SELECTOR.as_ref());
        // First 36 bytes: scalars and numbers
        buf.extend_from_slice(self.base_fee_scalar.to_be_bytes().as_ref()); // 4 bytes
        buf.extend_from_slice(self.blob_base_fee_scalar.to_be_bytes().as_ref()); // 4 bytes
        buf.extend_from_slice(self.sequence_number.to_be_bytes().as_ref()); // 8 bytes
        buf.extend_from_slice(self.time.to_be_bytes().as_ref()); // 8 bytes
        buf.extend_from_slice(self.number.to_be_bytes().as_ref()); // 8 bytes
        buf.extend_from_slice(U256::from(self.base_fee).to_be_bytes::<32>().as_ref());
        buf.extend_from_slice(U256::from(self.blob_base_fee).to_be_bytes::<32>().as_ref());
        buf.extend_from_slice(self.block_hash.as_ref());
        buf.extend_from_slice(self.batcher_address.into_word().as_ref());
        
        
        // At this point we are at offset 164 (including selector)
        // Ruby offsets exclude the selector, so Ruby offset 160 = our offset 164
        
        // Word 1 (offset 164): [fct_mint_period_l1_data_gas | fct_mint_rate]
        // Note: fct_mint_period_l1_data_gas is deprecated and always 0 after fork
        buf.extend_from_slice(0u128.to_be_bytes().as_ref()); // fct_mint_period_l1_data_gas
        buf.extend_from_slice(self.fct_mint_rate.to_be_bytes().as_ref());
        
        // Word 2 (offset 196): [fct_period_start_block | fct_total_minted]
        // Note: fct_period_start_block is promoted to 128-bit
        buf.extend_from_slice(self.fct_period_start_block.to_be_bytes().as_ref());
        buf.extend_from_slice(self.fct_total_minted.to_be_bytes().as_ref());
        
        // Word 3 (offset 228): [fct_max_supply | fct_period_minted]
        buf.extend_from_slice(self.fct_max_supply.to_be_bytes().as_ref());
        buf.extend_from_slice(self.fct_period_minted.to_be_bytes().as_ref());
        
        // Word 4 (offset 260): fct_initial_target_per_period padded to 32 bytes
        buf.extend_from_slice(U256::from(self.fct_initial_target_per_period).to_be_bytes::<32>().as_ref());
        
        // Notice: do not include the `empty_scalars` field in the calldata.
        // Notice: do not include the `l1_fee_overhead` field in the calldata.
        buf.into()
    }

    /// Decodes the [L1BlockInfoFacet] object from ethereum transaction calldata.
    pub fn decode_calldata(r: &[u8]) -> Result<Self, DecodeError> {
        if r.len() != Self::L1_INFO_TX_LEN {
            return Err(DecodeError::InvalidEcotoneLength(Self::L1_INFO_TX_LEN, r.len()));
        }

        // Helper function to decode u128 from big-endian bytes
        fn decode_u128_be(data: &[u8], start: usize) -> u128 {
            let mut bytes = [0u8; 16];
            bytes.copy_from_slice(&data[start..start + 16]);
            u128::from_be_bytes(bytes)
        }

        // SAFETY: 4 bytes are copied directly into the array
        let mut base_fee_scalar = [0u8; 4];
        base_fee_scalar.copy_from_slice(&r[4..8]);
        let base_fee_scalar = u32::from_be_bytes(base_fee_scalar);

        // SAFETY: 4 bytes are copied directly into the array
        let mut blob_base_fee_scalar = [0u8; 4];
        blob_base_fee_scalar.copy_from_slice(&r[8..12]);
        let blob_base_fee_scalar = u32::from_be_bytes(blob_base_fee_scalar);

        // SAFETY: 8 bytes are copied directly into the array
        let mut sequence_number = [0u8; 8];
        sequence_number.copy_from_slice(&r[12..20]);
        let sequence_number = u64::from_be_bytes(sequence_number);

        // SAFETY: 8 bytes are copied directly into the array
        let mut time = [0u8; 8];
        time.copy_from_slice(&r[20..28]);
        let time = u64::from_be_bytes(time);

        // SAFETY: 8 bytes are copied directly into the array
        let mut number = [0u8; 8];
        number.copy_from_slice(&r[28..36]);
        let number = u64::from_be_bytes(number);

        // SAFETY: 8 bytes are copied directly into the array
        let mut base_fee = [0u8; 8];
        base_fee.copy_from_slice(&r[60..68]);
        let base_fee = u64::from_be_bytes(base_fee);

        // SAFETY: 16 bytes are copied directly into the array
        let mut blob_base_fee = [0u8; 16];
        blob_base_fee.copy_from_slice(&r[84..100]);
        let blob_base_fee = u128::from_be_bytes(blob_base_fee);

        let block_hash = B256::from_slice(r[100..132].as_ref());
        // Batcher address is padded to 32 bytes, with the address in the last 20 bytes
        let batcher_address = Address::from_slice(r[144..164].as_ref());

        // Ruby offsets are from start of data (excluding selector)
        // Our offsets include the selector, so add 4 to each Ruby offset
        
        // Word 1 starts at offset 164 (Ruby 160 + 4)
        // Skip fct_mint_period_l1_data_gas at 164-180 (deprecated, always 0)
        let fct_mint_rate = decode_u128_be(r, 180); // 180-196
        
        // Word 2 starts at offset 196 (Ruby 192 + 4)
        let fct_period_start_block = decode_u128_be(r, 196); // Full 128-bit value
        let fct_total_minted = decode_u128_be(r, 212);
        
        // Word 3 starts at offset 228 (Ruby 224 + 4)
        let fct_max_supply = decode_u128_be(r, 228);
        let fct_period_minted = decode_u128_be(r, 244);
        
        // Word 4 starts at offset 260 (Ruby 256 + 4)
        // fct_initial_target_per_period is in the lower 128 bits
        let fct_initial_target_per_period = decode_u128_be(r, 276);

        Ok(Self {
            number,
            time,
            base_fee,
            block_hash,
            sequence_number,
            batcher_address,
            blob_base_fee,
            blob_base_fee_scalar,
            base_fee_scalar,
            // Notice: the `empty_scalars` field is not included in the calldata.
            // This is used by the evm to indicate that the bedrock tx l1 cost function
            // needs to be used.
            empty_scalars: false,
            // Notice: the `l1_fee_overhead` field is not included in the calldata.
            l1_fee_overhead: U256::ZERO,
            fct_mint_rate,
            fct_total_minted,
            fct_period_start_block,
            fct_period_minted,
            fct_max_supply,
            fct_initial_target_per_period,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn test_decode_calldata_facet_invalid_length() {
        let r = vec![0u8; 1];
        assert_eq!(
            L1BlockInfoFacet::decode_calldata(&r),
            Err(DecodeError::InvalidEcotoneLength(L1BlockInfoFacet::L1_INFO_TX_LEN, r.len(),))
        );
    }

    #[test]
    fn test_l1_block_info_facet_roundtrip_calldata_encoding() {
        let info = L1BlockInfoFacet {
            number: 1,
            time: 2,
            base_fee: 3,
            block_hash: B256::from([4u8; 32]),
            sequence_number: 5,
            batcher_address: Address::from([6u8; 20]),
            blob_base_fee: 7,
            blob_base_fee_scalar: 8,
            base_fee_scalar: 9,
            empty_scalars: false,
            l1_fee_overhead: U256::ZERO,
            fct_mint_rate: 1000,
            fct_total_minted: 2000,
            fct_period_start_block: 100,
            fct_period_minted: 500,
            fct_max_supply: 622_222_222,
            fct_initial_target_per_period: 29_595,
        };

        let calldata = info.encode_calldata();
        assert_eq!(calldata.len(), L1BlockInfoFacet::L1_INFO_TX_LEN);
        
        
        let decoded_info = L1BlockInfoFacet::decode_calldata(&calldata).unwrap();
        assert_eq!(info, decoded_info);
    }
    
    #[test]
    fn test_encoding_offsets() {
        // Test to verify the exact offsets of encoded fields
        let mut expected = vec![0u8; 292];
        
        // Selector
        expected[0..4].copy_from_slice(&L1BlockInfoFacet::L1_INFO_TX_SELECTOR);
        
        // Put a unique value for fct_mint_rate at the correct offset
        let test_rate = 0x1234567890abcdef_u128;
        expected[180..196].copy_from_slice(&test_rate.to_be_bytes()); // Updated offset
        
        // Now decode and check
        let decoded = L1BlockInfoFacet::decode_calldata(&expected).unwrap();
        assert_eq!(decoded.fct_mint_rate, test_rate);
    }
}