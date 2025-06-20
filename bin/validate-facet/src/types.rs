use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationResult {
    pub block: u64,
    pub execution: Option<TestResult>,
    pub derivation: Option<TestResult>,
    pub duration_ms: u64,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestResult {
    pub success: bool,
    pub error: Option<String>,
    pub error_type: Option<ErrorType>,
    pub retries: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorType {
    Network,       // Connection failed, timeout, etc.
    RateLimit,     // Too many requests
    NotFound,      // Block/data not available
    Validation,    // Actual validation failure (mismatch)
    System,        // Out of memory, disk space, etc.
    Unknown,       // Couldn't categorize
}

impl ErrorType {
    pub fn should_retry(&self) -> bool {
        // Always retry at least once for any error type
        true
    }
    
    pub fn max_retries(&self) -> u32 {
        match self {
            ErrorType::Network => 10,      // Network errors get more retries
            ErrorType::RateLimit => 10,    // Rate limits need backing off
            ErrorType::NotFound => 5,     // Data might appear
            ErrorType::System => 1,      // System errors rarely resolve
            ErrorType::Validation => 1,   // Validation errors unlikely to change
            ErrorType::Unknown => 1,     // Unknown errors get one retry
        }
    }
    
    pub fn backoff_multiplier(&self) -> f64 {
        match self {
            ErrorType::RateLimit => 2.0,    // Back off more aggressively
            ErrorType::NotFound => 1.5,     // Block might appear soon
            ErrorType::Network => 1.0,      // Standard backoff
            _ => 1.0,
        }
    }
}