//! FCT (Facet Compute Token) mint calculation logic.
//!
//! This module implements the FCT token minting mechanism which includes:
//! - Halving periods based on L2 block numbers
//! - Adjustment periods for rate recalculation
//! - L1 data gas usage tracking
//! - Dynamic mint rate calculations

/// FCT mint calculation constants and logic
#[derive(Debug)]
pub struct FctMintCalculator;

impl FctMintCalculator {
    /// Number of blocks in an adjustment period
    pub const ADJUSTMENT_PERIOD: u64 = 10_000;
    
    /// Seconds per Gregorian year (365.2425 days)
    pub const SECONDS_PER_YEAR: u64 = 31_556_952;
    
    /// Halving period in seconds (1 year)
    pub const HALVING_PERIOD_IN_SECONDS: u64 = 1 * Self::SECONDS_PER_YEAR;
    
    /// L2 block time in seconds (assumed 2 seconds)
    pub const L2_BLOCK_TIME: u64 = 12;
    
    /// Raw halving period in blocks
    pub const RAW_HALVING_PERIOD_IN_BLOCKS: u64 = Self::HALVING_PERIOD_IN_SECONDS / Self::L2_BLOCK_TIME;
    
    /// Number of adjustment periods per halving
    pub const ADJUSTMENT_PERIODS_PER_HALVING: u64 = Self::RAW_HALVING_PERIOD_IN_BLOCKS / Self::ADJUSTMENT_PERIOD;
    
    /// Actual halving period in blocks (rounded to adjustment periods)
    pub const HALVING_PERIOD_IN_BLOCKS: u64 = Self::ADJUSTMENT_PERIOD * Self::ADJUSTMENT_PERIODS_PER_HALVING;
    
    /// Target FCT mint per L1 block (40 ETH in wei)
    pub const TARGET_FCT_MINT_PER_L1_BLOCK: u128 = 40_000_000_000_000_000_000; // 40 * 10^18
    
    /// Target mint per adjustment period
    pub const TARGET_MINT_PER_PERIOD: u128 = Self::TARGET_FCT_MINT_PER_L1_BLOCK * Self::ADJUSTMENT_PERIOD as u128;
    
    /// Maximum adjustment factor for rate changes
    pub const MAX_ADJUSTMENT_FACTOR: u128 = 2;
    
    /// Initial mint rate (800,000 gwei)
    pub const INITIAL_RATE: u128 = 800_000_000_000_000; // 800_000 * 10^9
    
    /// Maximum mint rate (10,000,000 gwei)
    pub const MAX_RATE: u128 = 10_000_000_000_000_000; // 10_000_000 * 10^9
    
    /// Minimum mint rate
    pub const MIN_RATE: u128 = 1;
    
    /// Calculate how many halving periods have passed for a given L2 block number
    pub fn halving_periods_passed(current_l2_block: u64) -> u64 {
        current_l2_block / Self::HALVING_PERIOD_IN_BLOCKS
    }
    
    /// Calculate the halving factor (2^halving_periods)
    pub fn halving_factor(l2_block_number: u64) -> u128 {
        let periods = Self::halving_periods_passed(l2_block_number);
        2_u128.pow(periods as u32)
    }
    
    /// Check if this is the first block in an adjustment period
    pub fn is_first_block_in_period(l2_block_number: u64) -> bool {
        l2_block_number % Self::ADJUSTMENT_PERIOD == 0
    }
    
    /// Calculate the halving-adjusted target mint for a period
    pub fn halving_adjusted_target(l2_block_number: u64) -> u128 {
        let factor = Self::halving_factor(l2_block_number);
        if factor == 0 {
            return 0;
        }
        Self::TARGET_MINT_PER_PERIOD / factor
    }
    
    /// Compute the new FCT mint rate based on current conditions
    pub fn compute_new_rate(
        l2_block_number: u64,
        prev_rate: u128,
        cumulative_l1_data_gas: u128,
    ) -> u128 {
        if Self::is_first_block_in_period(l2_block_number) {
            let new_rate = if cumulative_l1_data_gas == 0 {
                Self::MAX_RATE
            } else {
                let halving_adjusted_target = Self::halving_adjusted_target(l2_block_number);
                if halving_adjusted_target == 0 {
                    return 0;
                }
                halving_adjusted_target / cumulative_l1_data_gas
            };
            
            // Apply adjustment factor limits
            let max_allowed_rate = (prev_rate * Self::MAX_ADJUSTMENT_FACTOR).min(Self::MAX_RATE);
            let min_allowed_rate = (prev_rate / Self::MAX_ADJUSTMENT_FACTOR).max(Self::MIN_RATE);
            
            new_rate.clamp(min_allowed_rate, max_allowed_rate)
        } else {
            prev_rate
        }
    }
    
    /// Calculate L1 data gas used for a transaction based on its input data
    pub fn calculate_data_gas_used(input_data: &[u8], contract_initiated: bool) -> u64 {
        if contract_initiated {
            (input_data.len() * 8) as u64
        } else {
            let zero_count = input_data.iter().filter(|&&b| b == 0).count();
            let non_zero_count = input_data.len() - zero_count;
            (zero_count * 4 + non_zero_count * 16) as u64
        }
    }
    
    /// Calculate the mint amount for a transaction
    pub fn calculate_mint_amount(l1_data_gas_used: u64, mint_rate: u128) -> u128 {
        (l1_data_gas_used as u128).saturating_mul(mint_rate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn test_halving_periods_calculation() {
        // Test genesis block
        assert_eq!(FctMintCalculator::halving_periods_passed(0), 0);
        
        // Test first halving period
        let halving_block = FctMintCalculator::HALVING_PERIOD_IN_BLOCKS;
        assert_eq!(FctMintCalculator::halving_periods_passed(halving_block), 1);
        
        // Test before first halving
        assert_eq!(FctMintCalculator::halving_periods_passed(halving_block - 1), 0);
    }
    
    #[test]
    fn test_halving_factor() {
        assert_eq!(FctMintCalculator::halving_factor(0), 1);
        assert_eq!(FctMintCalculator::halving_factor(FctMintCalculator::HALVING_PERIOD_IN_BLOCKS), 2);
        assert_eq!(FctMintCalculator::halving_factor(FctMintCalculator::HALVING_PERIOD_IN_BLOCKS * 2), 4);
    }
    
    #[test]
    fn test_is_first_block_in_period() {
        assert!(FctMintCalculator::is_first_block_in_period(0));
        assert!(FctMintCalculator::is_first_block_in_period(FctMintCalculator::ADJUSTMENT_PERIOD));
        assert!(FctMintCalculator::is_first_block_in_period(FctMintCalculator::ADJUSTMENT_PERIOD * 2));
        assert!(!FctMintCalculator::is_first_block_in_period(1));
        assert!(!FctMintCalculator::is_first_block_in_period(FctMintCalculator::ADJUSTMENT_PERIOD - 1));
    }
    
    #[test]
    fn test_data_gas_calculation() {
        // Test zero bytes
        let zero_data = vec![0u8; 10];
        assert_eq!(FctMintCalculator::calculate_data_gas_used(&zero_data, false), 40); // 10 * 4
        
        // Test non-zero bytes  
        let non_zero_data = vec![1u8; 10];
        assert_eq!(FctMintCalculator::calculate_data_gas_used(&non_zero_data, false), 160); // 10 * 16
        
        // Test mixed bytes
        let mixed_data = vec![0, 1, 0, 1];
        assert_eq!(FctMintCalculator::calculate_data_gas_used(&mixed_data, false), 40); // 2*4 + 2*16
        
        // Test contract initiated
        let data = vec![1u8; 10];
        assert_eq!(FctMintCalculator::calculate_data_gas_used(&data, true), 80); // 10 * 8
    }
    
    #[test]
    fn test_compute_new_rate_first_period() {
        let block_number = FctMintCalculator::ADJUSTMENT_PERIOD; // First adjustment
        let prev_rate = FctMintCalculator::INITIAL_RATE;
        
        // Test with zero cumulative gas
        let new_rate = FctMintCalculator::compute_new_rate(block_number, prev_rate, 0);
        assert_eq!(new_rate, FctMintCalculator::MAX_RATE);
        
        // Test with some cumulative gas
        let cumulative_gas = 1_000_000;
        let new_rate = FctMintCalculator::compute_new_rate(block_number, prev_rate, cumulative_gas);
        let expected_target = FctMintCalculator::halving_adjusted_target(block_number);
        let expected_rate = expected_target / cumulative_gas;
        
        // Should be clamped by adjustment factor
        let max_allowed = (prev_rate * FctMintCalculator::MAX_ADJUSTMENT_FACTOR).min(FctMintCalculator::MAX_RATE);
        let min_allowed = (prev_rate / FctMintCalculator::MAX_ADJUSTMENT_FACTOR).max(FctMintCalculator::MIN_RATE);
        
        assert_eq!(new_rate, expected_rate.clamp(min_allowed, max_allowed));
    }
    
    #[test]
    fn test_compute_new_rate_mid_period() {
        let block_number = 5; // Not first block in period
        let prev_rate = FctMintCalculator::INITIAL_RATE;
        let cumulative_gas = 1_000_000;
        
        // Should return previous rate unchanged
        let new_rate = FctMintCalculator::compute_new_rate(block_number, prev_rate, cumulative_gas);
        assert_eq!(new_rate, prev_rate);
    }
    
    #[test]
    fn test_realistic_mint_calculation() {
        // Test with realistic values similar to our example
        let l1_data_gas_used = 576; // From our test payload
        let mint_rate = FctMintCalculator::INITIAL_RATE; // 800,000 gwei
        
        let mint_amount = FctMintCalculator::calculate_mint_amount(l1_data_gas_used, mint_rate);
        let expected = 576u128 * 800_000_000_000_000u128; // 576 * 800,000 gwei
        
        assert_eq!(mint_amount, expected);
        assert_eq!(mint_amount, 460_800_000_000_000_000u128); // 0.4608 ETH
    }
    
    #[test]
    fn test_adjustment_period_boundary() {
        // Test that adjustment happens exactly at period boundaries
        assert!(FctMintCalculator::is_first_block_in_period(0));
        assert!(FctMintCalculator::is_first_block_in_period(FctMintCalculator::ADJUSTMENT_PERIOD));
        assert!(FctMintCalculator::is_first_block_in_period(FctMintCalculator::ADJUSTMENT_PERIOD * 2));
        
        assert!(!FctMintCalculator::is_first_block_in_period(1));
        assert!(!FctMintCalculator::is_first_block_in_period(FctMintCalculator::ADJUSTMENT_PERIOD - 1));
        assert!(!FctMintCalculator::is_first_block_in_period(FctMintCalculator::ADJUSTMENT_PERIOD + 1));
    }
}