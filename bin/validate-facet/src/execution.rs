use crate::retry::{calculate_backoff, classify_error, CircuitBreaker};
use crate::types::{ErrorType, TestResult};
use eyre::Result;
use std::path::Path;
use std::process::Command;
use std::time::Duration;
use tempfile::TempDir;
use tracing::{debug, warn};

pub async fn validate_execution(
    block: u64,
    l2_rpc: &str,
    max_retries: u32,
    results_dir: &Path,
) -> Result<TestResult> {
    let mut retries = 0;
    let mut last_error = None;
    let mut last_error_type = None;
    let mut circuit_breaker = CircuitBreaker::new(5, Duration::from_secs(60));
    let mut effective_max_retries = max_retries;
    
    loop {
        // Check circuit breaker
        if circuit_breaker.is_open() {
            warn!("Circuit breaker open for block {} execution, skipping", block);
            return Ok(TestResult {
                success: false,
                error: Some("Circuit breaker open - too many consecutive network failures".to_string()),
                error_type: Some(ErrorType::Network),
                retries,
            });
        }
        
        match run_execution_test(block, l2_rpc, results_dir).await {
            Ok(_) => {
                circuit_breaker.record_success();
                return Ok(TestResult {
                    success: true,
                    error: None,
                    error_type: None,
                    retries,
                });
            }
            Err(e) => {
                let error_type = classify_error(&e);
                last_error = Some(e.to_string());
                last_error_type = Some(error_type);
                
                // Update effective max retries based on error type
                effective_max_retries = effective_max_retries.min(error_type.max_retries());
                
                // Record failure in circuit breaker for network errors
                if error_type == ErrorType::Network || error_type == ErrorType::RateLimit {
                    circuit_breaker.record_failure();
                }
                
                // Don't retry if it's a validation error
                if !error_type.should_retry() {
                    debug!("Block {} execution failed with non-retryable error: {:?}", block, error_type);
                    break;
                }
                
                // Check if we've exceeded retries for this error type
                if retries >= effective_max_retries {
                    debug!("Block {} execution exceeded max retries ({}) for error type {:?}", 
                        block, effective_max_retries, error_type);
                    break;
                }
                
                retries += 1;
                
                let backoff = calculate_backoff(retries - 1, error_type);
                debug!(
                    "Block {} execution retry {}/{} after {:?} (error type: {:?})",
                    block, retries, effective_max_retries, backoff, error_type
                );
                tokio::time::sleep(backoff).await;
            }
        }
    }
    
    Ok(TestResult {
        success: false,
        error: last_error,
        error_type: last_error_type,
        retries,
    })
}

async fn run_execution_test(block: u64, l2_rpc: &str, results_dir: &Path) -> Result<()> {
    let temp_dir = TempDir::new()?;
    let log_file = results_dir.join("logs").join(format!("exec_{}.log", block));
    
    // Run execution-fixture
    let mut cmd = Command::new("./target/release/execution-fixture");
    cmd.args(&[
        "--l2-rpc", l2_rpc,
        "--block-number", &block.to_string(),
        "--output-dir", temp_dir.path().to_str().unwrap(),
    ]);
    
    let output = cmd.output()?;
    
    // Save logs
    std::fs::write(&log_file, &output.stdout)?;
    if !output.stderr.is_empty() {
        std::fs::write(log_file.with_extension("err"), &output.stderr)?;
    }
    
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        
        // Check for common error patterns
        if stderr.contains("error sending request") || 
           stderr.contains("transport error") ||
           stdout.contains("error sending request") ||
           stdout.contains("transport error") ||
           stderr.contains("500 Internal Server Error") ||
           stderr.contains("HttpError") {
            return Err(eyre::eyre!("execution-fixture failed: network error - {}", stderr));
        }
        
        // Preimage not found errors are infrastructure issues, not validation failures
        if stderr.contains("Preimage not found") {
            return Err(eyre::eyre!("execution-fixture failed: missing preimage data - {}", stderr));
        }
        
        return Err(eyre::eyre!("execution-fixture failed: {}", stderr));
    }
    
    // Check if fixture was created
    let fixture_path = temp_dir.path().join(format!("block-{}.tar.gz", block));
    if !fixture_path.exists() {
        return Err(eyre::eyre!("Fixture not created"));
    }
    
    // Run validation test
    let mut cmd = Command::new("cargo");
    cmd.args(&[
        "test", "-p", "kona-executor",
        "test_validate_single_fixture",
        "--release", "--", "--nocapture"
    ]);
    cmd.env("FIXTURE_PATH", fixture_path);
    
    let output = cmd.output()?;
    
    // Append test logs
    let mut log_content = std::fs::read(&log_file)?;
    log_content.extend_from_slice(b"\n=== Validation Test ===\n");
    log_content.extend_from_slice(&output.stdout);
    std::fs::write(&log_file, log_content)?;
    
    if !output.status.success() {
        return Err(eyre::eyre!("Validation test failed"));
    }
    
    Ok(())
}