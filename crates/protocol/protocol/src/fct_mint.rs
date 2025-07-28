//! FCT (Facet Compute Token) mint calculation logic.
//!
//! This module implements the FCT token minting mechanism which includes:
//! - Halving periods based on L2 block numbers
//! - Adjustment periods for rate recalculation
//! - L1 data gas usage tracking
//! - Dynamic mint rate calculations

#[allow(unused_extern_crates)]
extern crate alloc;

use crate::{FctParams, L1BlockInfoFacet};
#[allow(unused_imports)]
use alloy_primitives::U256;
use num_bigint::BigInt;
use num_rational::BigRational;
use num_traits::{Zero, One, ToPrimitive, FromPrimitive};

/// Convert BigRational to u128 (floors to integer with saturation)
fn rational_to_u128_sat(r: &BigRational) -> u128 {
    r.to_integer().to_u128().unwrap_or(u128::MAX)
}

/// Adjustment type for period transitions
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AdjustmentType {
    AdjustUp,
    AdjustDown,
}

/// Halving factor constant
const HALVING_FACTOR: u32 = 2;

/// State machine for managing FCT minting within and across periods
#[derive(Clone, Debug)]
pub(crate) struct MintPeriod {
    /// Current block number
    pub block_num: u64,
    /// Current FCT mint rate (FCT per gas)
    pub fct_mint_rate: BigRational,
    /// Total FCT minted across all periods
    pub total_minted: BigRational,
    /// FCT minted in current period
    pub period_minted: BigRational,
    /// Block number when current period started
    pub period_start_block: u64,
    /// Maximum FCT supply
    pub max_supply: u128,
    /// Target mint per period (before halving adjustments)
    pub target_per_period: u128,
}

impl MintPeriod {
    /// Consumes an ETH burn amount, returns FCT minted for this tx (Rational)
    pub(crate) fn consume_eth(&mut self, eth_burn: u128) -> BigRational {
        let mut remaining_eth = BigRational::from_u128(eth_burn).unwrap();
        let mut minted = BigRational::zero();
        
        while !remaining_eth.is_zero() && !self.supply_exhausted() {
            let mint_possible = &remaining_eth * &self.fct_mint_rate;
            let mint_amount = mint_possible.clone()
                .min(self.remaining_period_quota())
                .min(self.remaining_supply());

            let burn_used = &mint_amount / &self.fct_mint_rate;
            remaining_eth = remaining_eth - burn_used;

            minted = &minted + &mint_amount;
            self.period_minted = &self.period_minted + &mint_amount;
            self.total_minted = &self.total_minted + &mint_amount;
            
            if self.remaining_period_quota().is_zero() {
                self.start_new_period(AdjustmentType::AdjustDown);
            }
        }

        minted
    }
    
    pub(crate) fn remaining_period_quota(&self) -> BigRational {
        let current_target = self.current_target();
        let remaining = &current_target - &self.period_minted;
        if remaining > BigRational::zero() {
            remaining.floor()
        } else {
            BigRational::zero()
        }
    }
    
    pub(crate) fn remaining_supply(&self) -> BigRational {
        let max_supply = self.max_supply();
        let remaining = &max_supply - &self.total_minted;
        if remaining > BigRational::zero() {
            remaining.floor()
        } else {
            BigRational::zero()
        }
    }

    pub(crate) fn assign_mint_amounts(&mut self, facet_txs: &mut [crate::FacetPayload], current_l1_base_fee: u64) {
        if self.blocks_elapsed_in_period() >= FctMintCalculator::adjustment_period_target_length().to_integer().to_u64().unwrap() {
            self.start_new_period(AdjustmentType::AdjustUp);
        }
        
        for tx in facet_txs.iter_mut() {
            let burn = (tx.l1_data_gas_used as u128) * (current_l1_base_fee as u128);
            let mint = self.consume_eth(burn);
            tx.mint = rational_to_u128_sat(&mint);
        }
    }

    pub(crate) fn max_supply(&self) -> BigRational {
        BigRational::from_u128(self.max_supply).unwrap()
    }
    
    pub(crate) fn current_target(&self) -> BigRational {
        let mut target = BigRational::from_u128(self.target_per_period).unwrap();
        let halving_level = self.get_current_halving_level();
        for _ in 0..halving_level {
            target = target / BigRational::from_u32(HALVING_FACTOR).unwrap();
        }
        let min_target = BigRational::one();
        if target > min_target {
            target.floor()
        } else {
            min_target
        }
    }
    
    pub(crate) fn get_current_halving_level(&self) -> u32 {
        let mut level = 0;
        let max_supply = self.max_supply();
        let mut threshold = &max_supply / BigRational::from_u32(HALVING_FACTOR).unwrap();
        
        // Find how many halving thresholds we've crossed
        while self.total_minted >= threshold && threshold < max_supply && self.total_minted < max_supply {
            level += 1;
            let remaining = &max_supply - &threshold;
            threshold = &threshold + (&remaining / BigRational::from_u32(HALVING_FACTOR).unwrap()); // Add half of the remaining supply
        }
        
        level
    }

    pub(crate) fn supply_exhausted(&self) -> bool {
        self.total_minted >= self.max_supply()
    }

    pub(crate) fn blocks_elapsed_in_period(&self) -> u64 {
        self.block_num - self.period_start_block
    }

    pub(crate) fn start_new_period(&mut self, adjustment_type: AdjustmentType) {
        match adjustment_type {
            AdjustmentType::AdjustDown => self.down_adjust_rate(),
            AdjustmentType::AdjustUp => self.up_adjust_rate(),
        }
        
        self.period_start_block = self.block_num;
        self.period_minted = BigRational::zero();
    }

    // --- rate helpers -------------------------------------------------------
    fn down_adjust_rate(&mut self) {
        let raw_ratio = BigRational::from_u64(self.blocks_elapsed_in_period()).unwrap() / FctMintCalculator::adjustment_period_target_length();
        let capped_ratio = raw_ratio.max(FctMintCalculator::max_rate_adjustment_down_factor());
        self.fct_mint_rate = self.compute_and_cap_rate(self.fct_mint_rate.clone(), capped_ratio);
    }
    
    fn up_adjust_rate(&mut self) {
        let capped_ratio = if self.period_minted.is_zero() {
            FctMintCalculator::max_rate_adjustment_up_factor()
        } else {
            let target = self.current_target();
            let raw = target / &self.period_minted;
            let max_up = FctMintCalculator::max_rate_adjustment_up_factor();
            raw.min(max_up)
        };
        
        self.fct_mint_rate = self.compute_and_cap_rate(self.fct_mint_rate.clone(), capped_ratio);
    }

    
    pub(crate) fn compute_and_cap_rate(&self, prev_rate: BigRational, adjustment_factor: BigRational) -> BigRational {
        let raw_new_rate = prev_rate * adjustment_factor;
        raw_new_rate.clamp(FctMintCalculator::min_mint_rate(), FctMintCalculator::max_mint_rate())
    }
}

/// FCT mint calculation constants and logic
#[derive(Debug)]
pub struct FctMintCalculator;

impl FctMintCalculator {
    pub fn adjustment_period_target_length() -> BigRational {
        BigRational::from_u32(500).unwrap()
    }
    
    /// Maximum mint rate ((2^128) - 1)
    pub fn max_mint_rate() -> BigRational {
        // 2^128 - 1 = 340282366920938463463374607431768211455
        let two_pow_128 = BigInt::from(2u32).pow(128);
        BigRational::from_integer(two_pow_128 - 1)
    }
    
    /// Minimum mint rate
    pub fn min_mint_rate() -> BigRational {
        BigRational::one()
    }
    
    /// Maximum rate adjustment up factor
    pub fn max_rate_adjustment_up_factor() -> BigRational {
        BigRational::from_u32(4).unwrap()
    }
    
    /// Maximum rate adjustment down factor (1/4)
    pub fn max_rate_adjustment_down_factor() -> BigRational {
        BigRational::new(
            BigInt::one(),
            BigInt::from_u32(4).unwrap()
        )
    }
    
    /// Target issuance fraction for first halving (1/2)
    pub fn target_issuance_fraction_first_halving() -> BigRational {
        BigRational::new(
            BigInt::one(),
            BigInt::from_u32(2).unwrap()
        )
    }
    
    /// Target number of blocks in halving period
    pub fn target_num_blocks_in_halving() -> BigRational {
        BigRational::from_u32(2_628_000).unwrap()
    }
    
    /// Calculate L1 data gas used for a transaction based on its input data
    /// Uses EIP-7623 pricing: 10 gas per zero byte, 40 gas per non-zero byte
    pub fn calculate_data_gas_used(input_data: &[u8], contract_initiated: bool) -> u64 {
        if contract_initiated {
            // Contract-initiated txs use 8 gas per byte regardless
            (input_data.len() * 8) as u64
        } else {
            // EIP-7623 pricing for EOA transactions
            let zero_count = input_data.iter().filter(|&&b| b == 0).count();
            let non_zero_count = input_data.len() - zero_count;
            (zero_count * 10 + non_zero_count * 40) as u64
        }
    }
    
    /// Main driver function to assign mint amounts to transactions
    /// This loads previous period state, handles period rolling, and assigns mints
    pub fn assign_mint(
        facet_txs: &mut [crate::FacetPayload],
        block_number: u64,
        l1_base_fee: u64,
        prev_l1_info: &L1BlockInfoFacet,
    ) -> (u128, u128, u64, u128) {
        // Get FCT params from global state
        let params = FctParams::get().expect("FctParams not initialized");
        
        // Load period state from previous block
        let mut period = MintPeriod {
            block_num: block_number,
            fct_mint_rate: BigRational::from_u128(prev_l1_info.fct_mint_rate).unwrap(),
            total_minted: BigRational::from_u128(prev_l1_info.fct_total_minted).unwrap(),
            period_minted: BigRational::from_u128(prev_l1_info.fct_period_minted).unwrap(),
            period_start_block: prev_l1_info.fct_period_start_block,
            max_supply: params.max_supply,
            target_per_period: params.target_per_period,
        };
        
        // Use assign_mint_amounts which handles period rolling internally
        period.assign_mint_amounts(facet_txs, l1_base_fee);
        
        // Return updated state for L1 block info
        (
            rational_to_u128_sat(&period.fct_mint_rate),
            rational_to_u128_sat(&period.total_minted),
            period.period_start_block,
            rational_to_u128_sat(&period.period_minted),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FacetPayload;
    use alloc::vec;
    use alloy_primitives::{Bytes, Address};
    use num_traits::ToPrimitive;

    #[test]
    fn test_data_gas_calculation_eip7623() {
        // Test zero bytes with EIP-7623 pricing
        let zero_data = vec![0u8; 10];
        assert_eq!(FctMintCalculator::calculate_data_gas_used(&zero_data, false), 100); // 10 * 10
        
        // Test non-zero bytes with EIP-7623 pricing
        let non_zero_data = vec![1u8; 10];
        assert_eq!(FctMintCalculator::calculate_data_gas_used(&non_zero_data, false), 400); // 10 * 40
        
        // Test mixed bytes
        let mixed_data = vec![0, 1, 0, 1];
        assert_eq!(FctMintCalculator::calculate_data_gas_used(&mixed_data, false), 100); // 2*10 + 2*40
        
        // Test contract initiated (always 8 gas/byte)
        let data = vec![1u8; 10];
        assert_eq!(FctMintCalculator::calculate_data_gas_used(&data, true), 80); // 10 * 8
    }

    #[test]
    #[ignore] // This test modifies global state, run with --ignored
    fn test_genesis_period() {
        // Initialize FCT params
        let one_eth = 1_000_000_000_000_000_000u128;
        let max_supply = 21_000_000u128 * one_eth;
        let target_per_period = 20_000u128 * one_eth;
        FctParams::force_init(max_supply, target_per_period);
        
        // Create genesis block state
        let prev_l1_info = L1BlockInfoFacet {
            fct_mint_rate: 800_000_000_000_000u128,
            fct_total_minted: 0,
            fct_period_start_block: 0,
            fct_period_minted: 0,
            ..Default::default()
        };
        
        // Empty transactions, no gas consumed
        let mut facet_txs = vec![];
        
        let (rate, total, start, period) = FctMintCalculator::assign_mint(
            &mut facet_txs,
            1, // block 1
            1_000_000_000, // 1 gwei base fee
            &prev_l1_info,
        );
        
        // Should maintain initial rate
        assert_eq!(rate, 800_000_000_000_000u128);
        assert_eq!(total, 0);
        assert_eq!(start, 0);
        assert_eq!(period, 0);
    }

    #[test]
    fn test_up_rate_adjustment_calculation() {
        // Test the rate adjustment calculation directly
        let one_eth = 1_000_000_000_000_000_000u128;
        let max_supply = 21_000_000u128 * one_eth;
        let target_per_period = 20_000u128 * one_eth;
        
        let mut period = MintPeriod {
            block_num: 500,
            fct_mint_rate: BigRational::from_u64(1_000_000_000_000_000).unwrap(),
            total_minted: BigRational::from_u128(10_000u128 * one_eth).unwrap(), // 10k ETH
            period_minted: BigRational::from_u128(10_000u128 * one_eth).unwrap(), // 10k ETH (half target)
            period_start_block: 0,
            max_supply,
            target_per_period,
        };
        
        // Get current target
        let target = period.current_target();
        let expected_target = BigRational::from_u128(20_000u128 * one_eth).unwrap();
        assert_eq!(target, expected_target);
        
        // Calculate adjustment ratio
        let ratio = &target / &period.period_minted;
        assert_eq!(ratio, BigRational::from_u32(2).unwrap());
        
        // Apply up adjustment
        period.up_adjust_rate();
        
        // Should be 2x the original rate
        let expected_rate = BigRational::from_u64(2_000_000_000_000_000).unwrap();
        assert_eq!(period.fct_mint_rate, expected_rate);
    }
    
    #[test]
    fn test_debug_rate_calculation() {
        // Debug the exact calculation happening
        let one_eth = 1_000_000_000_000_000_000u128;
        let max_supply = 21_000_000u128 * one_eth;
        let target_per_period = 20_000u128 * one_eth;
        
        // Test with exact values from failing test
        let half_target_u128 = target_per_period / 2;
        
        // Create period from L1 info like assign_mint does
        let mut period = MintPeriod {
            block_num: 500,
            fct_mint_rate: BigRational::from_u128(1_000_000_000_000_000u128).unwrap(),
            total_minted: BigRational::from_u128(half_target_u128).unwrap(),
            period_minted: BigRational::from_u128(half_target_u128).unwrap(),
            period_start_block: 0,
            max_supply,
            target_per_period,
        };
        
        // Check current_target
        let target = period.current_target();
        let expected_target = BigRational::from_u128(target_per_period).unwrap();
        
        // Debug print the values
        let target_u128 = rational_to_u128_sat(&target);
        let period_minted_u128 = rational_to_u128_sat(&period.period_minted);
        let expected_target_u128 = rational_to_u128_sat(&expected_target);
        
        // Print the actual values for debugging
        if target_u128 != expected_target_u128 || period_minted_u128 != half_target_u128 {
            // Values don't match, let's see what they are
            panic!("Value mismatch:\n  target: {} (expected {})\n  period_minted: {} (expected {})\n  half_target_u128: {}",
                target_u128, expected_target_u128, period_minted_u128, half_target_u128, half_target_u128);
        }
        
        // Check the adjustment ratio
        let ratio = &target / &period.period_minted;
        let ratio_f64 = ratio.numer().to_f64().unwrap() / ratio.denom().to_f64().unwrap();
        assert!(ratio_f64 > 1.9 && ratio_f64 < 2.1, "Ratio should be ~2, got {}", ratio_f64);
        
        // Now do the full flow
        let mut facet_txs = vec![];
        period.assign_mint_amounts(&mut facet_txs, 1_000_000_000);
        
        // Check final rate
        let final_rate = rational_to_u128_sat(&period.fct_mint_rate);
        assert_eq!(final_rate, 2_000_000_000_000_000u128, "Rate should double");
    }
    
    #[test]
    #[ignore] // This test modifies global state, run with --ignored
    fn test_period_target_consumption() {
        // Initialize FCT params
        let one_eth = 1_000_000_000_000_000_000u128;
        let max_supply = 21_000_000u128 * one_eth;
        let target_per_period = 20_000u128 * one_eth;
        FctParams::force_init(max_supply, target_per_period);
        
        // Create state where period consumed half the target
        let half_target = target_per_period / 2;
        let prev_l1_info = L1BlockInfoFacet {
            fct_mint_rate: 1_000_000_000_000_000u128, // 0.001 FCT per gas
            fct_total_minted: half_target, // Total minted includes this period
            fct_period_start_block: 0,
            fct_period_minted: half_target, // Only minted half the target this period
            ..Default::default()
        };
        
        // Empty tx to trigger period roll
        let mut facet_txs = vec![];
        
        let (rate, total, start, period) = FctMintCalculator::assign_mint(
            &mut facet_txs,
            500, // End of period by time
            1_000_000_000,
            &prev_l1_info,
        );
        
        // Check what params are being used
        let params = FctParams::get().unwrap();
        assert_eq!(params.target_per_period, target_per_period, "Target per period mismatch");
        
        // Check total minted
        assert_eq!(total, half_target, "Total minted should not change with empty tx");
        
        // Rate should double (minted only half target, so need to catch up)
        // Adjustment factor = target/minted = 20k/10k = 2
        assert_eq!(rate, 2_000_000_000_000_000u128); // 2x the original rate
        assert_eq!(start, 500); // New period started
        assert_eq!(period, 0); // Period minted reset
    }

    #[test]
    #[ignore] // This test modifies global state, run with --ignored  
    fn test_supply_cap() {
        // Initialize FCT params
        let one_eth = 1_000_000_000_000_000_000u128;
        let max_supply = 100u128 * one_eth; // Small max supply for testing
        let target_per_period = 50u128 * one_eth;
        FctParams::force_init(max_supply, target_per_period);
        
        // Create state near max supply
        let prev_l1_info = L1BlockInfoFacet {
            fct_mint_rate: 800_000_000_000_000u128, // 0.0008 FCT per gas
            fct_total_minted: 99 * 1_000_000_000_000_000_000, // 99 ETH of 100 ETH max
            fct_period_start_block: 0,
            fct_period_minted: 0,
            ..Default::default()
        };
        
        // Transaction that would mint 2 ETH worth
        let mut facet_txs = vec![
            FacetPayload {
                to: Some(Address::ZERO),
                value: U256::ZERO,
                gas_limit: 100000,
                data: Bytes::from(vec![1u8; 50]),
                l1_data_gas_used: 2000, // 50 non-zero bytes * 40 = 2000 gas
                mint: 0,
            }
        ];
        
        let (_rate, total, _start, _period) = FctMintCalculator::assign_mint(
            &mut facet_txs,
            100,
            1_000_000_000, // 1 gwei base fee
            &prev_l1_info,
        );
        
        // Should cap at max supply (100 ETH)
        assert_eq!(total, 100 * 1_000_000_000_000_000_000);
        // Transaction should only get 1 ETH (capped)
        assert_eq!(facet_txs[0].mint, 1_000_000_000_000_000_000);
    }
    
    // ==================== Comprehensive tests ported from Ruby ====================
    
    /// Helper to create a FacetPayload with specified gas usage
    fn build_tx(burn_tokens: u64) -> FacetPayload {
        FacetPayload {
            to: None,
            value: U256::ZERO,
            gas_limit: 0,
            data: Bytes::new(),
            l1_data_gas_used: burn_tokens,
            mint: 0,
        }
    }
    
    /// Helper to initialize FCT parameters for testing
    fn init_test_params() {
        let max_supply = 622_222_222u128;
        let initial_target = 29_595u128;
        FctParams::force_init(max_supply, initial_target);
    }
    
    // ==================== Post-fork minting logic tests ====================
    
    #[test]
    fn test_mints_within_current_period_without_closing() {
        init_test_params();
        
        let prev_l1_info = L1BlockInfoFacet {
            number: 1000,
            fct_mint_rate: 2,
            fct_total_minted: 140_000_000,
            fct_period_start_block: 995,
            fct_period_minted: 100,
            ..Default::default()
        };
        
        let mut txs = vec![build_tx(100)];
        let base_fee = 10u64;
        
        let (rate, total, start_block, period_minted) = FctMintCalculator::assign_mint(
            &mut txs,
            1010,
            base_fee,
            &prev_l1_info,
        );
        
        // ETH burned = 100 gas * 10 wei/gas = 1000 wei
        // FCT minted = 1000 * 2 = 2000
        assert_eq!(txs[0].mint, 2_000);
        assert_eq!(total, 140_002_000);
        assert_eq!(period_minted, 2_100);
        assert_eq!(rate, 2);
        assert_eq!(start_block, 995); // Period didn't roll
    }
    
    #[test]
    fn test_closes_period_when_mint_cap_hit_and_starts_new() {
        init_test_params();
        
        let target_per_period = 29_595u128;
        let prev_l1_info = L1BlockInfoFacet {
            number: 1000,
            fct_mint_rate: 2,
            fct_total_minted: 140_000_000,
            fct_period_start_block: 995,
            fct_period_minted: target_per_period - 341, // 341 short of cap
            ..Default::default()
        };
        
        let mut txs = vec![build_tx(200)]; // burns 2000 wei ETH
        let base_fee = 10u64;
        
        let (rate, total, start_block, period_minted) = FctMintCalculator::assign_mint(
            &mut txs,
            1010,
            base_fee,
            &prev_l1_info,
        );
        
        // First 341 FCT fills old period, rest goes to new period
        // The exact mint amount depends on rate adjustment
        assert!(txs[0].mint > 341);
        assert_eq!(total, 140_000_000 + txs[0].mint);
        assert_eq!(start_block, 1010); // New period started
        assert!(period_minted < target_per_period); // New period not full
        assert!(rate <= 2); // Rate adjusted down
    }
    
    #[test]
    fn test_adjusts_rate_up_when_period_ends_by_block_count() {
        init_test_params();
        
        let adj_period_len = 500u64;
        let target_per_period = 29_595u128;
        let block_num = 1000 + adj_period_len;
        
        let prev_l1_info = L1BlockInfoFacet {
            number: block_num - 1,
            fct_mint_rate: 2,
            fct_total_minted: 140_000_000,
            fct_period_start_block: 1000,
            fct_period_minted: target_per_period / 2, // Way under target
            ..Default::default()
        };
        
        let mut txs = vec![build_tx(10)];
        let base_fee = 10u64;
        
        let (rate, _total, start_block, period_minted) = FctMintCalculator::assign_mint(
            &mut txs,
            block_num,
            base_fee,
            &prev_l1_info,
        );
        
        // Rate should increase (doubled since actual = target/2)
        assert_eq!(rate, 4); // 2 * 2
        assert_eq!(start_block, block_num); // New period
        assert_eq!(period_minted, txs[0].mint); // Only this tx in new period
    }
    
    #[test]
    fn test_handles_multi_period_spillover() {
        init_test_params();
        
        let prev_l1_info = L1BlockInfoFacet {
            number: 1019,
            fct_mint_rate: 1,
            fct_total_minted: 140_000_000,
            fct_period_start_block: 970,
            fct_period_minted: 0,
            ..Default::default()
        };
        
        let mut txs = vec![build_tx(500_000)]; // Large burn
        let base_fee = 1u64;
        
        let (rate, total, start_block, _period_minted) = FctMintCalculator::assign_mint(
            &mut txs,
            1020,
            base_fee,
            &prev_l1_info,
        );
        
        // Should mint across multiple periods
        let target_per_period = 29_595u128;
        assert!(txs[0].mint > target_per_period);
        assert_eq!(total, 140_000_000 + txs[0].mint);
        assert_eq!(start_block, 1020); // New period at current block
        assert!(rate >= 1); // Rate adjusted based on consumption
    }
    
    #[test]
    fn test_lowers_target_after_crossing_halving_threshold() {
        // Initialize with larger max supply for this test
        if !FctParams::is_initialized() {
            let max_supply = 622_222_222u128;
            let initial_target = 29_595u128;
            FctParams::init(max_supply, initial_target);
        }
        
        // Just before first halving (50% of 622M = 311M)
        let prev_l1_info = L1BlockInfoFacet {
            number: 1029,
            fct_mint_rate: 1,
            fct_total_minted: 310_000_000,
            fct_period_start_block: 1020,
            fct_period_minted: 0,
            ..Default::default()
        };
        
        let mut txs = vec![build_tx(2_000_000)]; // Large burn to cross threshold
        let base_fee = 1u64;
        
        let (_rate, total, _start_block, _period_minted) = FctMintCalculator::assign_mint(
            &mut txs,
            1030,
            base_fee,
            &prev_l1_info,
        );
        
        // Should have crossed halving threshold
        assert!(total > 311_111_111); // Past 50% of max supply
    }
    
    #[test]
    fn test_caps_minting_when_max_supply_exhausted() {
        // Re-initialize params with custom max supply
        let max_supply = 1000u128;
        
        let _prev_l1_info = L1BlockInfoFacet {
            number: 100,
            fct_mint_rate: 5,
            fct_total_minted: max_supply - 50, // Only 50 left
            fct_period_start_block: 99, // Set to current block - 1 to avoid period roll
            fct_period_minted: 0,
            ..Default::default()
        };
        
        // Create period directly to bypass global state issues
        let mut period = MintPeriod {
            block_num: 100,
            fct_mint_rate: BigRational::from_u128(5).unwrap(),
            total_minted: BigRational::from_u128(max_supply - 50).unwrap(),
            period_minted: BigRational::zero(),
            period_start_block: 99,
            max_supply,
            target_per_period: 100u128,
        };
        
        // No need to init global params since MintPeriod has its own
        
        let burn_wei = 100u128; // Would mint 500 normally
        let minted = period.consume_eth(burn_wei);
        
        assert_eq!(rational_to_u128_sat(&minted), 50); // Capped at remaining supply
        assert_eq!(rational_to_u128_sat(&period.total_minted), max_supply); // Exactly at max
    }
    
    #[test]
    fn test_starts_new_period_when_cap_met_exactly() {
        init_test_params();
        
        let target_per_period = 29_595u128;
        let prev_l1_info = L1BlockInfoFacet {
            number: 1059,
            fct_mint_rate: 2,
            fct_total_minted: 140_000_000 + target_per_period,
            fct_period_start_block: 1059,
            fct_period_minted: target_per_period, // Exactly at cap
            ..Default::default()
        };
        
        let mut txs = vec![build_tx(100)];
        let base_fee = 10u64;
        
        let (_rate, _total, start_block, period_minted) = FctMintCalculator::assign_mint(
            &mut txs,
            1060,
            base_fee,
            &prev_l1_info,
        );
        
        // Period should have rolled
        assert!(txs[0].mint > 0);
        assert_eq!(start_block, 1060); // New period
        assert_eq!(period_minted, txs[0].mint); // Only this tx
    }
    
    #[test]
    fn test_proportional_down_adjustment_when_period_ends_mid_block() {
        init_test_params();
        
        let adj_period_len = 500u64;
        let blocks_elapsed = (adj_period_len as f64 * 0.8) as u64; // 80% of period
        let target_per_period = 29_595u128;
        
        let prev_l1_info = L1BlockInfoFacet {
            number: 999 + blocks_elapsed,
            fct_mint_rate: 10,
            fct_total_minted: 140_000_000,
            fct_period_start_block: 1000,
            fct_period_minted: target_per_period - 101, // 101 short
            ..Default::default()
        };
        
        let mut txs = vec![build_tx(101)]; // Exactly fills cap
        let base_fee = 1u64;
        
        let (rate, _total, _start_block, _period_minted) = FctMintCalculator::assign_mint(
            &mut txs,
            1000 + blocks_elapsed,
            base_fee,
            &prev_l1_info,
        );
        
        // Factor should be 0.8 (400/500 = 0.8)
        // new_rate = 10 * 0.8 = 8
        assert_eq!(rate, 8);
    }
    
    #[test]
    fn test_correctly_calculates_fct_based_on_eth_burned() {
        init_test_params();
        
        let mint_rate = 5u128;
        let base_fee = 20u64;
        let gas_used = 1_000u64;
        
        let prev_l1_info = L1BlockInfoFacet {
            number: 1049,
            fct_mint_rate: mint_rate,
            fct_total_minted: 140_000_000,
            fct_period_start_block: 1040,
            fct_period_minted: 1_000,
            ..Default::default()
        };
        
        let mut txs = vec![build_tx(gas_used)];
        
        let (_rate, _total, _start_block, _period_minted) = FctMintCalculator::assign_mint(
            &mut txs,
            1050,
            base_fee,
            &prev_l1_info,
        );
        
        // ETH burned = gas_used * base_fee = 1000 * 20 = 20,000 wei
        // FCT minted = 20,000 * 5 = 100,000 (if within period cap)
        let eth_burned = gas_used * base_fee;
        let expected_uncapped = (eth_burned as u128) * mint_rate;
        
        // Actual mint may be capped by period quota
        assert!(txs[0].mint <= expected_uncapped);
        assert!(txs[0].mint > 0);
    }
    
    #[test]
    fn test_handles_zero_minting_in_period_for_rate_adjustment() {
        init_test_params();
        
        let adj_period_len = 500u64;
        let prev_l1_info = L1BlockInfoFacet {
            number: 999 + adj_period_len,
            fct_mint_rate: 3,
            fct_total_minted: 140_000_000,
            fct_period_start_block: 1000,
            fct_period_minted: 0, // No minting in period
            ..Default::default()
        };
        
        let mut txs = vec![build_tx(10)];
        let base_fee = 1u64;
        
        let (rate, _total, _start_block, _period_minted) = FctMintCalculator::assign_mint(
            &mut txs,
            1000 + adj_period_len,
            base_fee,
            &prev_l1_info,
        );
        
        // Rate should be increased by max factor (4x)
        assert_eq!(rate, 12); // 3 * 4
    }
    
    #[test]
    fn test_opens_fresh_period_when_adjustment_period_elapsed() {
        init_test_params();
        
        let adj_period_len = 500u64;
        let block_num = 1000 + adj_period_len + 3; // > 1 full period
        
        let prev_l1_info = L1BlockInfoFacet {
            number: block_num - 1,
            fct_mint_rate: 2,
            fct_total_minted: 140_050_000,
            fct_period_start_block: 1000,
            fct_period_minted: 50_000,
            ..Default::default()
        };
        
        let mut txs = vec![build_tx(50)];
        let base_fee = 10u64;
        
        let (_rate, _total, start_block, period_minted) = FctMintCalculator::assign_mint(
            &mut txs,
            block_num,
            base_fee,
            &prev_l1_info,
        );
        
        // New period should start at current block
        assert_eq!(start_block, block_num);
        assert_eq!(period_minted, txs[0].mint);
    }
    
    // ==================== Halving threshold tests ====================
    
    #[test]
    fn test_halving_thresholds() {
        init_test_params();
        
        let target_per_period = 29_595u128;
        
        // Test different total minted amounts
        let mut period = MintPeriod {
            block_num: 1100,
            fct_mint_rate: BigRational::one(),
            total_minted: BigRational::from_u128(140_000_000).unwrap(), // Below 50%
            period_minted: BigRational::zero(),
            period_start_block: 1100,
            max_supply: 622_222_222u128,
            target_per_period: 29_595u128,
        };
        
        let _params = FctParams::get().unwrap();
        assert_eq!(period.get_current_halving_level(), 0);
        assert_eq!(period.current_target().to_integer().to_u128().unwrap(), target_per_period);
        
        // First halving (50%)
        period.total_minted = BigRational::from_u128(311_111_112).unwrap();
        assert_eq!(period.get_current_halving_level(), 1);
        assert_eq!(period.current_target().to_integer().to_u128().unwrap(), target_per_period / 2);
        
        // Second halving (75%)
        period.total_minted = BigRational::from_u128(466_666_667).unwrap();
        assert_eq!(period.get_current_halving_level(), 2);
        assert_eq!(period.current_target().to_integer().to_u128().unwrap(), target_per_period / 4);
    }
    
    // ==================== Edge cases and error conditions ====================
    
    #[test]
    fn test_handles_extremely_large_burns() {
        init_test_params();
        
        let prev_l1_info = L1BlockInfoFacet {
            number: 1099,
            fct_mint_rate: 1,
            fct_total_minted: 140_000_000,
            fct_period_start_block: 1090,
            fct_period_minted: 0,
            ..Default::default()
        };
        
        let mut txs = vec![build_tx(1_000_000)]; // Huge burn
        let base_fee = 1u64;
        
        let (_rate, total, start_block, _period_minted) = FctMintCalculator::assign_mint(
            &mut txs,
            1100,
            base_fee,
            &prev_l1_info,
        );
        
        // Should handle gracefully
        assert!(txs[0].mint > 0);
        assert!(total > 140_000_000);
        assert_eq!(start_block, 1100);
    }
    
    #[test]
    fn test_respects_min_rate_limit() {
        init_test_params();
        
        let target_per_period = 29_595u128;
        let prev_l1_info = L1BlockInfoFacet {
            number: 1109,
            fct_mint_rate: 2,
            fct_total_minted: 140_000_000,
            fct_period_start_block: 1100,
            fct_period_minted: target_per_period, // Hit target in 10 blocks
            ..Default::default()
        };
        
        let mut txs = vec![build_tx(100)];
        let base_fee = 1u64;
        
        let (rate, _total, _start_block, _period_minted) = FctMintCalculator::assign_mint(
            &mut txs,
            1110,
            base_fee,
            &prev_l1_info,
        );
        
        // Rate should be reduced but not below minimum of 1
        assert!(rate >= 1);
    }
    
    #[test]
    fn test_handles_zero_base_fee() {
        init_test_params();
        
        let prev_l1_info = L1BlockInfoFacet {
            number: 1099,
            fct_mint_rate: 5,
            fct_total_minted: 140_000_000,
            fct_period_start_block: 1090,
            fct_period_minted: 1000,
            ..Default::default()
        };
        
        let mut txs = vec![build_tx(1000)];
        let base_fee = 0u64; // Zero base fee
        
        let (_rate, _total, _start_block, _period_minted) = FctMintCalculator::assign_mint(
            &mut txs,
            1100,
            base_fee,
            &prev_l1_info,
        );
        
        // Should mint 0 when base fee is 0 (no ETH burned)
        assert_eq!(txs[0].mint, 0);
    }
    
    #[test]
    fn test_handles_exact_halving_boundaries() {
        // Initialize with specific max supply
        let max_supply = 622_222_222u128;
        let initial_target = 29_595u128;
        FctParams::init(max_supply, initial_target);
        
        // Set up to land exactly on first halving threshold
        let prev_l1_info = L1BlockInfoFacet {
            number: 1099,
            fct_mint_rate: 1,
            fct_total_minted: 311_111_110, // 2 FCT short
            fct_period_start_block: 1090,
            fct_period_minted: 0,
            ..Default::default()
        };
        
        let mut txs = vec![build_tx(2)]; // Exactly crosses threshold
        let base_fee = 1u64;
        
        let (_rate, total, _start_block, _period_minted) = FctMintCalculator::assign_mint(
            &mut txs,
            1100,
            base_fee,
            &prev_l1_info,
        );
        
        // Should trigger first halving exactly
        assert_eq!(total, 311_111_112);
    }
    
    // ==================== MintPeriod tests ====================
    
    #[test]
    fn test_compute_and_cap_rate() {
        init_test_params();
        
        let period = MintPeriod {
            block_num: 1000,
            fct_mint_rate: BigRational::from_u128(100).unwrap(),
            total_minted: BigRational::zero(),
            period_minted: BigRational::zero(),
            period_start_block: 1000,
            max_supply: 622_222_222u128,
            target_per_period: 29_595u128,
        };
        
        // Test capping above MAX_MINT_RATE
        let huge_rate = FctMintCalculator::max_mint_rate() 
            + BigRational::from_u128(1000).unwrap();
        let factor = BigRational::from_u32(2).unwrap();
        let capped = period.compute_and_cap_rate(huge_rate, factor);
        assert_eq!(capped, FctMintCalculator::max_mint_rate());
        
        // Test capping below MIN_MINT_RATE  
        let tiny_rate = BigRational::from_u128(10).unwrap();
        let small_factor = BigRational::new(
            BigInt::one(),
            BigInt::from_u32(100_000).unwrap()
        );
        let capped = period.compute_and_cap_rate(tiny_rate, small_factor);
        assert_eq!(capped, FctMintCalculator::min_mint_rate());
        
        // Test normal case - multiply by 3/2 = 1.5
        let normal_rate = BigRational::from_u128(1000).unwrap();
        let factor = BigRational::new(
            BigInt::from_u32(3).unwrap(),
            BigInt::from_u32(2).unwrap()
        );
        let result = period.compute_and_cap_rate(normal_rate, factor);
        assert_eq!(result, BigRational::from_u128(1500).unwrap());
    }
}