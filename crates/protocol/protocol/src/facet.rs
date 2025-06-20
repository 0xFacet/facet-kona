use alloy_primitives::{Address, Bytes, TxKind, B256, U256};
use alloy_rlp::{RlpDecodable, RlpEncodable, Decodable};
use op_alloy_consensus::TxDeposit;
use crate::FctMintCalculator;
use alloc::string::{String, ToString};
use alloc::format;

/// Prefix byte identifying a Facet payload.
pub const FACET_TX_TYPE: u8 = 0x46;
/// Prefix byte for an Optimism deposit.
pub const DEPOSIT_TX_TYPE: u8 = 0x7e;

/// 0x1111000000000000000000000000000000001111 per OP Stack address aliasing rule.
const ALIAS_OFFSET: U256 = U256::from_be_bytes([
  0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // 8
  0x00, 0x00, 0x00, 0x00, 0x11, 0x11, 0x00, 0x00, // 16
  0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // 24
  0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x11, 0x11, // 32
]);

#[inline]
pub fn alias_l1_to_l2(addr: Address) -> Address {
  // Convert the L1 address to U256 - pad with zeros on the left
  let mut addr_bytes = [0u8; 32];
  addr_bytes[12..32].copy_from_slice(addr.as_slice());
  let addr_u256 = U256::from_be_bytes(addr_bytes);
  
  // Add the offset (modulo 2^160 to handle overflow)
  let aliased_u256: U256 = (addr_u256 + ALIAS_OFFSET) % (U256::from(1u64) << 160);
  
  // Convert back to Address - take the lower 20 bytes
  let bytes = aliased_u256.to_be_bytes::<32>();
  Address::from_slice(&bytes[12..])
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum DecodeError {
    #[error("payload too short")]
    Short,
    #[error("expected 0x46 prefix, got 0x{0:02x}")]
    WrongPrefix(u8),
    #[error("RLP decode error: {0}")]
    Rlp(String),
    #[error("chain-id {0} does not equal expected {1}")]
    BadChainId(u64, u64),
}

/// Internal RLP structure matching the format: [chain_id, to, value, gas, data, mine_boost]
#[derive(Debug, Clone, RlpDecodable, RlpEncodable)]
struct FacetPayloadRlp {
    chain_id: u64,
    to: Bytes,
    value: U256,
    gas_limit: u64,
    data: Bytes,
    mine_boost: Bytes,  // Additional data that counts toward FCT mint
}

#[derive(Debug, Clone)]
pub struct FacetPayload {
    pub to: Option<Address>,
    pub value: U256,
    pub gas_limit: u64,
    pub data: Bytes,
    pub l1_data_gas_used: u64,
    pub mint: u128,
}

pub fn decode_facet_payload(bytes: &[u8], l2_chain_id: u64, contract_initiated: bool) -> Result<FacetPayload, DecodeError> {
    if bytes.is_empty() {
        return Err(DecodeError::Short);
    }
    if bytes[0] != FACET_TX_TYPE {
        return Err(DecodeError::WrongPrefix(bytes[0]));
    }
    
    let rlp_data = &bytes[1..];
    let rlp_payload = FacetPayloadRlp::decode(&mut &rlp_data[..]).map_err(|e| DecodeError::Rlp(e.to_string()))?;
    
    if rlp_payload.chain_id != l2_chain_id {
        return Err(DecodeError::BadChainId(rlp_payload.chain_id, l2_chain_id));
    }
    
    let to = if rlp_payload.to.is_empty() {
        None // Contract creation
    } else if rlp_payload.to.len() == 20 {
        Some(Address::from_slice(&rlp_payload.to))
    } else {
        // Invalid "to" field - must be either empty or exactly 20 bytes
        return Err(DecodeError::Rlp(format!("invalid 'to' field length: {}", rlp_payload.to.len())));
    };
    
    // Calculate L1 data gas used based on the entire transaction payload
    let l1_data_gas_used = FctMintCalculator::calculate_data_gas_used(bytes, contract_initiated);
    
    Ok(FacetPayload {
        to,
        value: rlp_payload.value,
        gas_limit: rlp_payload.gas_limit,
        data: rlp_payload.data,
        l1_data_gas_used,
        mint: 0u128, // Will be set later by mint calculation
    })
}

impl FacetPayload {
    pub fn into_deposit(self, from: Address, source_hash: B256) -> TxDeposit {
        TxDeposit {
            from,
            to: match self.to {
                Some(addr) => TxKind::Call(addr),
                None => TxKind::Create,
            },
            value: self.value,
            gas_limit: self.gas_limit,
            input: self.data,
            mint: Some(self.mint),
            is_system_transaction: false,
            source_hash,
            ..Default::default()
        }
    }
    
    /// Set the mint amount for this payload
    pub fn set_mint(&mut self, mint: u128) {
        self.mint = mint;
    }
}