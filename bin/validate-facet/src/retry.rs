use crate::types::ErrorType;
use rand::Rng;
use std::time::Duration;

/// Classify an error based on its message
pub fn classify_error(error: &eyre::Error) -> ErrorType {
    let error_str = error.to_string().to_lowercase();
    
    // Network-related errors
    if error_str.contains("connection") ||
       error_str.contains("timeout") ||
       error_str.contains("broken pipe") ||
       error_str.contains("reset by peer") ||
       error_str.contains("network") ||
       error_str.contains("dns") ||
       error_str.contains("failed to connect") ||
       error_str.contains("transport error") ||
       error_str.contains("error sending request") ||
       error_str.contains("http error") {
        return ErrorType::Network;
    }
    
    // Rate limiting
    if error_str.contains("rate limit") ||
       error_str.contains("too many requests") ||
       error_str.contains("429") {
        return ErrorType::RateLimit;
    }
    
    // Block/data not found
    if error_str.contains("not found") ||
       error_str.contains("does not exist") ||
       error_str.contains("unknown block") ||
       error_str.contains("missing") && error_str.contains("block") ||
       error_str.contains("failed to fetch block") {
        return ErrorType::NotFound;
    }
    
    // Execution fixture errors - classify based on the specific error
    if error_str.contains("execution-fixture failed") {
        if error_str.contains("network error") || 
           error_str.contains("500 internal server error") ||
           error_str.contains("httperror") {
            return ErrorType::Network;
        }
        if error_str.contains("missing preimage") {
            return ErrorType::NotFound;  // Missing data, might be retryable
        }
        // Other execution failures are likely validation errors
        return ErrorType::Validation;
    }
    
    // System errors
    if error_str.contains("out of memory") ||
       error_str.contains("disk full") ||
       error_str.contains("permission denied") ||
       error_str.contains("no space left") {
        return ErrorType::System;
    }
    
    // Validation errors (actual mismatches)
    if error_str.contains("mismatch") ||
       error_str.contains("differs") ||
       error_str.contains("validation failed") ||
       error_str.contains("transaction count mismatch") ||
       error_str.contains("hash mismatch") {
        return ErrorType::Validation;
    }
    
    ErrorType::Unknown
}

/// Calculate backoff duration with jitter
pub fn calculate_backoff(retry_count: u32, error_type: ErrorType) -> Duration {
    let base_delay_ms = 1000u64;
    let max_delay_ms = 60_000u64; // Cap at 60 seconds
    
    // Exponential backoff: 2^retry * base_delay
    let exponential_delay = base_delay_ms.saturating_mul(2u64.saturating_pow(retry_count));
    
    // Apply error-specific multiplier
    let multiplier = error_type.backoff_multiplier();
    let delay_with_multiplier = (exponential_delay as f64 * multiplier) as u64;
    
    // Cap the delay
    let capped_delay = delay_with_multiplier.min(max_delay_ms);
    
    // Add jitter (±25%)
    let mut rng = rand::thread_rng();
    let jitter_factor = 0.75 + (rng.gen::<f64>() * 0.5); // 0.75 to 1.25
    let final_delay = (capped_delay as f64 * jitter_factor) as u64;
    
    Duration::from_millis(final_delay)
}

/// Circuit breaker state
pub struct CircuitBreaker {
    consecutive_failures: u32,
    last_failure_time: Option<std::time::Instant>,
    threshold: u32,
    reset_duration: Duration,
}

impl CircuitBreaker {
    pub fn new(threshold: u32, reset_duration: Duration) -> Self {
        Self {
            consecutive_failures: 0,
            last_failure_time: None,
            threshold,
            reset_duration,
        }
    }
    
    pub fn record_success(&mut self) {
        self.consecutive_failures = 0;
        self.last_failure_time = None;
    }
    
    pub fn record_failure(&mut self) {
        self.consecutive_failures += 1;
        self.last_failure_time = Some(std::time::Instant::now());
    }
    
    pub fn is_open(&self) -> bool {
        if self.consecutive_failures >= self.threshold {
            if let Some(last_failure) = self.last_failure_time {
                // Check if we should reset
                if last_failure.elapsed() > self.reset_duration {
                    return false;
                }
            }
            true
        } else {
            false
        }
    }
    
    pub fn reset(&mut self) {
        self.consecutive_failures = 0;
        self.last_failure_time = None;
    }
}