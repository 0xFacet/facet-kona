//! FCT parameters singleton module
//! 
//! This module provides a singleton accessor for FCT (Facet Compute Token) parameters
//! that are configured at node startup and remain constant throughout execution.

use core::sync::atomic::{AtomicBool, Ordering};

/// FCT parameters that control the minting behavior
#[derive(Debug, Clone, Copy)]
pub struct FctParams {
    /// Maximum FCT supply in wei
    pub max_supply: u128,
    /// Target mint per period in wei (before halving adjustments)
    pub target_per_period: u128,
}

/// Global singleton for FCT parameters - simplified for no_std
static mut FCT_PARAMS: Option<FctParams> = None;
static FCT_INITIALIZED: AtomicBool = AtomicBool::new(false);

impl FctParams {
    /// Initialize the FCT parameters singleton
    /// 
    /// This should be called once at node startup with values from the rollup config.
    /// Returns false if already initialized.
    pub fn init(max_supply: u128, target_per_period: u128) -> bool {
        if FCT_INITIALIZED.load(Ordering::Acquire) {
            return false;
        }
        
        // SAFETY: This is safe because we only write once, protected by atomic check
        unsafe {
            FCT_PARAMS = Some(FctParams {
                max_supply,
                target_per_period,
            });
        }
        
        FCT_INITIALIZED.store(true, Ordering::Release);
        true
    }

    /// Get the FCT parameters singleton
    /// 
    /// Returns None if the parameters haven't been initialized yet.
    pub fn get() -> Option<&'static FctParams> {
        if FCT_INITIALIZED.load(Ordering::Acquire) {
            // SAFETY: If initialized flag is true, FCT_PARAMS is Some
            // Using addr_of! to avoid creating a reference to the static mut
            unsafe { 
                let ptr = core::ptr::addr_of!(FCT_PARAMS);
                (*ptr).as_ref()
            }
        } else {
            None
        }
    }

    /// Check if the FCT parameters have been initialized
    pub fn is_initialized() -> bool {
        FCT_INITIALIZED.load(Ordering::Acquire)
    }
    
    /// Force re-initialization of FCT parameters (TEST ONLY)
    /// 
    /// This is unsafe and should only be used in tests that need to
    /// change the global parameters.
    #[cfg(test)]
    pub fn force_init(max_supply: u128, target_per_period: u128) {
        // SAFETY: This is only used in tests and we accept the race conditions
        unsafe {
            FCT_PARAMS = Some(FctParams {
                max_supply,
                target_per_period,
            });
        }
        FCT_INITIALIZED.store(true, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore] // This test modifies global state, run with --ignored
    fn test_fct_params_singleton() {
        // This test should run in isolation to avoid conflicts with other tests
        if FctParams::is_initialized() {
            // Skip if already initialized by another test
            return;
        }

        // Initially not initialized
        assert!(!FctParams::is_initialized());
        assert!(FctParams::get().is_none());

        // Initialize with test values
        let max_supply = 21_000_000u128 * 10u128.pow(18);
        let target_per_period = 1000u128 * 10u128.pow(18);
        assert!(FctParams::init(max_supply, target_per_period));

        // Now it should be initialized
        assert!(FctParams::is_initialized());
        
        let params = FctParams::get().unwrap();
        assert_eq!(params.max_supply, max_supply);
        assert_eq!(params.target_per_period, target_per_period);
        // Trying to init again should return false
        assert!(!FctParams::init(max_supply, target_per_period));
    }
}