use clap::Parser;
use eyre::Result;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;
use tracing::{error, info};

mod derivation;
mod execution;
mod retry;
mod types;

use types::{ErrorType, TestResult, ValidationResult};

#[derive(Parser, Debug, Clone)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Starting block number
    #[arg(short = 's', long)]
    start_block: u64,

    /// Ending block number (inclusive)
    #[arg(short = 'e', long)]
    end_block: u64,

    /// Number of parallel workers
    #[arg(short = 'j', long, default_value = "16")]
    jobs: usize,

    /// L1 RPC endpoint
    #[arg(long, env = "L1_RPC")]
    l1_rpc: String,

    /// L2 RPC endpoint
    #[arg(long, env = "L2_RPC")]
    l2_rpc: String,

    /// Output directory for results
    #[arg(short = 'o', long)]
    output_dir: Option<PathBuf>,

    /// Skip execution validation
    #[arg(long)]
    skip_execution: bool,

    /// Skip derivation validation
    #[arg(long)]
    skip_derivation: bool,

    /// Sample rate for derivation (e.g., 10 means test every 10th block)
    #[arg(long, default_value = "1")]
    derivation_sample_rate: u64,

    /// Resume from a previous run
    #[arg(short = 'r', long)]
    resume: Option<PathBuf>,

    /// Maximum retries per block
    #[arg(long, default_value = "2")]
    max_retries: u32,

    /// Checkpoint interval (blocks)
    #[arg(long, default_value = "1000")]
    checkpoint_interval: u64,

    /// Stop if failure rate exceeds this percentage
    #[arg(long, default_value = "10.0")]
    failure_threshold: f64,

    /// Verbose output
    #[arg(short, long)]
    verbose: bool,

    /// Random sampling mode - test N random blocks from the range
    #[arg(long, conflicts_with = "resume")]
    random_sample: Option<usize>,

    /// Seed for random sampling (for reproducibility)
    #[arg(long, default_value = "42")]
    random_seed: u64,
}


struct ValidationState {
    completed: AtomicUsize,
    failed: AtomicUsize,
    total: usize,
    start_time: Instant,
    results_dir: PathBuf,
    checkpoint_file: PathBuf,
    results_file: PathBuf,
    results_mutex: tokio::sync::Mutex<()>,
    recent_failures: Arc<tokio::sync::Mutex<Vec<(u64, String)>>>,
}

impl ValidationState {
    fn new(total: usize, results_dir: PathBuf) -> Self {
        let checkpoint_file = results_dir.join("checkpoint.json");
        let results_file = results_dir.join("results.jsonl");
        
        Self {
            completed: AtomicUsize::new(0),
            failed: AtomicUsize::new(0),
            total,
            start_time: Instant::now(),
            results_dir,
            checkpoint_file,
            results_file,
            results_mutex: tokio::sync::Mutex::new(()),
            recent_failures: Arc::new(tokio::sync::Mutex::new(Vec::new())),
        }
    }

    async fn record_result(&self, result: ValidationResult) -> Result<()> {
        // Lock mutex to ensure atomic writes
        let _guard = self.results_mutex.lock().await;
        
        // Append to results file
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.results_file)?;
        
        serde_json::to_writer(&mut file, &result)?;
        use std::io::Write;
        writeln!(&mut file)?;
        
        // Update counters
        let is_failed = result.execution.as_ref().map(|r| !r.success).unwrap_or(false) ||
                       result.derivation.as_ref().map(|r| !r.success).unwrap_or(false);
        
        if is_failed {
            self.failed.fetch_add(1, Ordering::Relaxed);
            
            // Track recent failures
            let mut failures = self.recent_failures.lock().await;
            let error_msg = if let Some(exec) = &result.execution {
                if !exec.success {
                    exec.error.clone().unwrap_or_else(|| "Unknown execution error".to_string())
                } else if let Some(deriv) = &result.derivation {
                    deriv.error.clone().unwrap_or_else(|| "Unknown derivation error".to_string())
                } else {
                    "Unknown error".to_string()
                }
            } else {
                "Unknown error".to_string()
            };
            
            failures.push((result.block, error_msg));
            // Keep only last 10 failures
            if failures.len() > 10 {
                failures.remove(0);
            }
        }
        self.completed.fetch_add(1, Ordering::Relaxed);
        
        Ok(())
    }

    fn save_checkpoint(&self, processed_blocks: &[u64]) -> Result<()> {
        let checkpoint = Checkpoint {
            processed_blocks: processed_blocks.to_vec(),
            timestamp: chrono::Utc::now(),
        };
        
        let json = serde_json::to_string_pretty(&checkpoint)?;
        fs::write(&self.checkpoint_file, json)?;
        
        Ok(())
    }

    fn get_stats(&self) -> Stats {
        let completed = self.completed.load(Ordering::Relaxed);
        let failed = self.failed.load(Ordering::Relaxed);
        let elapsed = self.start_time.elapsed();
        
        let rate = if elapsed.as_secs() > 0 {
            completed as f64 / elapsed.as_secs_f64() * 60.0
        } else {
            0.0
        };
        
        let success_rate = if completed > 0 {
            ((completed - failed) as f64 / completed as f64) * 100.0
        } else {
            0.0
        };
        
        let eta_seconds = if rate > 0.0 {
            ((self.total - completed) as f64 / rate * 60.0) as u64
        } else {
            0
        };
        
        Stats {
            completed,
            failed,
            total: self.total,
            success_rate,
            blocks_per_minute: rate,
            elapsed_seconds: elapsed.as_secs(),
            eta_seconds,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct Checkpoint {
    processed_blocks: Vec<u64>,
    timestamp: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug)]
struct Stats {
    completed: usize,
    failed: usize,
    total: usize,
    success_rate: f64,
    blocks_per_minute: f64,
    elapsed_seconds: u64,
    eta_seconds: u64,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    
    // Setup logging
    let filter = if args.verbose { "debug" } else { "info" };
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .init();
    
    // Setup output directory
    let output_dir = args.output_dir.clone();
    let results_dir = output_dir.unwrap_or_else(|| {
        let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
        PathBuf::from(format!("validation_{}_{}_{}",
            args.start_block, args.end_block, timestamp))
    });
    fs::create_dir_all(&results_dir)?;
    fs::create_dir_all(results_dir.join("logs"))?;
    
    info!("🚀 Facet Validation Tool");
    info!("Range: {} - {}", args.start_block, args.end_block);
    info!("Workers: {}", args.jobs);
    info!("Output: {}", results_dir.display());
    
    // Build required binaries
    if !args.skip_execution {
        info!("🔨 Building execution-fixture...");
        build_execution_fixture()?;
    }
    
    // Determine blocks to process
    let mut blocks_to_process: Vec<u64> = (args.start_block..=args.end_block).collect();
    
    // Handle random sampling
    if let Some(sample_size) = args.random_sample {
        use rand::SeedableRng;
        use rand::rngs::StdRng;
        
        info!("🎲 Random sampling mode: {} blocks from range", sample_size);
        
        let total_range = blocks_to_process.len();
        if sample_size > total_range {
            return Err(eyre::eyre!("Sample size {} exceeds range size {}", sample_size, total_range));
        }
        
        // Create a seeded RNG for reproducibility
        let mut rng = StdRng::seed_from_u64(args.random_seed);
        
        // Shuffle and take first N blocks
        use rand::seq::SliceRandom;
        blocks_to_process.shuffle(&mut rng);
        blocks_to_process.truncate(sample_size);
        
        info!("  Using seed: {}", args.random_seed);
        info!("  Selected blocks: {} (from {} to {})", 
            blocks_to_process.len(),
            blocks_to_process.first().unwrap_or(&0),
            blocks_to_process.last().unwrap_or(&0)
        );
    }
    
    // Handle resume
    let resume_dir = args.resume.clone();
    if let Some(resume_dir) = resume_dir {
        if resume_dir.exists() {
            info!("📂 Resuming from checkpoint...");
            let checkpoint: Checkpoint = serde_json::from_str(&fs::read_to_string(resume_dir.join("checkpoint.json"))?)?;
            let processed: std::collections::HashSet<_> = checkpoint.processed_blocks.into_iter().collect();
            blocks_to_process.retain(|b| !processed.contains(b));
            info!("  Already processed: {}", processed.len());
            info!("  Remaining: {}", blocks_to_process.len());
        }
    }
    
    let total_blocks = blocks_to_process.len();
    let state = Arc::new(ValidationState::new(total_blocks, results_dir.clone()));
    
    // Progress bars
    let multi_progress = MultiProgress::new();
    let main_progress = multi_progress.add(ProgressBar::new(total_blocks as u64));
    main_progress.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({per_sec}) {msg}")?
            .progress_chars("=>-")
    );
    
    // Spawn stats thread
    let _stats_handle = spawn_stats_monitor(state.clone(), multi_progress.clone());
    
    // Create semaphore for concurrency control
    let semaphore = Arc::new(Semaphore::new(args.jobs));
    
    // Process blocks
    let mut tasks = vec![];
    let processed_blocks = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    
    for block in blocks_to_process {
        let permit = semaphore.clone().acquire_owned().await?;
        let state = state.clone();
        let args = args.clone();
        let main_progress = main_progress.clone();
        let results_dir = results_dir.clone();
        let processed_blocks = processed_blocks.clone();
        
        let task = tokio::spawn(async move {
            let _permit = permit;
            
            let start = Instant::now();
            let mut result = ValidationResult {
                block,
                execution: None,
                derivation: None,
                duration_ms: 0,
                timestamp: chrono::Utc::now(),
            };
            
            // Run execution validation
            if !args.skip_execution {
                match execution::validate_execution(
                    block,
                    &args.l2_rpc,
                    args.max_retries,
                    &results_dir,
                ).await {
                    Ok(test_result) => result.execution = Some(test_result),
                    Err(e) => {
                        error!("Block {} execution error: {}", block, e);
                        result.execution = Some(TestResult {
                            success: false,
                            error: Some(e.to_string()),
                            error_type: Some(ErrorType::Unknown),
                            retries: 0,
                        });
                    }
                }
            }
            
            // Run derivation validation (with sampling)
            if !args.skip_derivation && block % args.derivation_sample_rate == 0 {
                match derivation::validate_derivation(
                    block,
                    &args.l1_rpc,
                    &args.l2_rpc,
                    args.max_retries,
                ).await {
                    Ok(test_result) => result.derivation = Some(test_result),
                    Err(e) => {
                        error!("Block {} derivation error: {}", block, e);
                        result.derivation = Some(TestResult {
                            success: false,
                            error: Some(e.to_string()),
                            error_type: Some(ErrorType::Unknown),
                            retries: 0,
                        });
                    }
                }
            }
            
            result.duration_ms = start.elapsed().as_millis() as u64;
            
            // Record result
            if let Err(e) = state.record_result(result.clone()).await {
                error!("Failed to record result: {}", e);
            }
            
            // Print failures in real-time
            let exec_failed = result.execution.as_ref().map(|r| !r.success).unwrap_or(false);
            let deriv_failed = result.derivation.as_ref().map(|r| !r.success).unwrap_or(false);
            
            if exec_failed || deriv_failed {
                let mut failure_msg = format!("❌ Block {} failed:", block);
                let mut is_infrastructure_issue = false;
                
                if exec_failed {
                    let exec_result = result.execution.as_ref().unwrap();
                    if let Some(err) = &exec_result.error {
                        let error_type_str = exec_result.error_type
                            .map(|t| format!(" [{:?}]", t))
                            .unwrap_or_default();
                        failure_msg.push_str(&format!("\n   Execution: {}{}", err, error_type_str));
                        
                        if let Some(error_type) = exec_result.error_type {
                            if matches!(error_type, ErrorType::Network | ErrorType::RateLimit | ErrorType::NotFound) {
                                is_infrastructure_issue = true;
                            }
                        }
                    }
                }
                
                if deriv_failed {
                    let deriv_result = result.derivation.as_ref().unwrap();
                    if let Some(err) = &deriv_result.error {
                        let error_type_str = deriv_result.error_type
                            .map(|t| format!(" [{:?}]", t))
                            .unwrap_or_default();
                        failure_msg.push_str(&format!("\n   Derivation: {}{}", err, error_type_str));
                        
                        if let Some(error_type) = deriv_result.error_type {
                            if matches!(error_type, ErrorType::Network | ErrorType::RateLimit | ErrorType::NotFound) {
                                is_infrastructure_issue = true;
                            }
                        }
                    }
                }
                
                if is_infrastructure_issue {
                    failure_msg.push_str("\n   ⚠️  This appears to be an infrastructure issue, not a validation failure");
                }
                
                error!("{}", failure_msg);
            }
            
            // Update progress
            main_progress.inc(1);
            
            // Add to processed blocks
            processed_blocks.lock().await.push(block);
            
            // Check if we need to checkpoint
            let completed = state.completed.load(Ordering::Relaxed);
            if completed % args.checkpoint_interval as usize == 0 {
                let blocks = processed_blocks.lock().await.clone();
                if let Err(e) = state.save_checkpoint(&blocks) {
                    error!("Failed to save checkpoint: {}", e);
                }
            }
            
            // Check failure threshold
            let stats = state.get_stats();
            if stats.success_rate < (100.0 - args.failure_threshold) && completed > 10 {
                error!("Failure rate ({:.1}%) exceeds threshold", 100.0 - stats.success_rate);
                std::process::exit(1);
            }
        });
        
        tasks.push(task);
    }
    
    // Wait for all tasks
    for task in tasks {
        let _ = task.await;
    }
    
    main_progress.finish_with_message("Complete!");
    
    // Final stats
    let stats = state.get_stats();
    info!("");
    info!("🏁 Validation Complete");
    info!("====================");
    info!("Total blocks: {}", stats.total);
    info!("Completed: {}", stats.completed);
    info!("Failed: {}", stats.failed);
    info!("Success rate: {:.2}%", stats.success_rate);
    info!("Duration: {}s", stats.elapsed_seconds);
    info!("Average: {:.2} blocks/min", stats.blocks_per_minute);
    
    // Analyze failure types
    analyze_failure_types(&results_dir).await?;
    
    // Generate final report
    let report = FinalReport {
        start_block: args.start_block,
        end_block: args.end_block,
        total_blocks: stats.total,
        completed: stats.completed,
        failed: stats.failed,
        success_rate: stats.success_rate,
        duration_seconds: stats.elapsed_seconds,
        blocks_per_minute: stats.blocks_per_minute,
        timestamp: chrono::Utc::now(),
    };
    
    let report_file = results_dir.join("final_report.json");
    fs::write(report_file, serde_json::to_string_pretty(&report)?)?;
    
    Ok(())
}

async fn analyze_failure_types(results_dir: &PathBuf) -> Result<()> {
    use std::collections::HashMap;
    
    let results_file = results_dir.join("results.jsonl");
    let content = tokio::fs::read_to_string(&results_file).await?;
    
    let mut error_type_counts: HashMap<String, usize> = HashMap::new();
    let mut validation_failures: Vec<(u64, &str, String)> = Vec::new();
    let mut infrastructure_failures: Vec<(u64, &str, ErrorType)> = Vec::new();
    
    for line in content.lines() {
        if let Ok(result) = serde_json::from_str::<ValidationResult>(line) {
            // Check execution failures
            if let Some(exec) = &result.execution {
                if !exec.success {
                    let error_type = exec.error_type.unwrap_or(ErrorType::Unknown);
                    let key = format!("Execution/{:?}", error_type);
                    *error_type_counts.entry(key).or_insert(0) += 1;
                    
                    match error_type {
                        ErrorType::Validation => {
                            let error_msg = exec.error.clone().unwrap_or_else(|| "unknown".to_string());
                            validation_failures.push((result.block, "execution", error_msg));
                        }
                        ErrorType::Network | ErrorType::RateLimit | ErrorType::NotFound => {
                            infrastructure_failures.push((result.block, "execution", error_type));
                        }
                        _ => {}
                    }
                }
            }
            
            // Check derivation failures
            if let Some(deriv) = &result.derivation {
                if !deriv.success {
                    let error_type = deriv.error_type.unwrap_or(ErrorType::Unknown);
                    let key = format!("Derivation/{:?}", error_type);
                    *error_type_counts.entry(key).or_insert(0) += 1;
                    
                    match error_type {
                        ErrorType::Validation => {
                            let error_msg = deriv.error.clone().unwrap_or_else(|| "unknown".to_string());
                            validation_failures.push((result.block, "derivation", error_msg));
                        }
                        ErrorType::Network | ErrorType::RateLimit | ErrorType::NotFound => {
                            infrastructure_failures.push((result.block, "derivation", error_type));
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    
    // Print failure analysis
    info!("");
    info!("📊 Failure Analysis");
    info!("==================");
    
    if !error_type_counts.is_empty() {
        info!("Error Type Breakdown:");
        let mut sorted_errors: Vec<_> = error_type_counts.into_iter().collect();
        sorted_errors.sort_by(|a, b| b.1.cmp(&a.1));
        
        for (error_type, count) in sorted_errors {
            info!("  {}: {}", error_type, count);
        }
    }
    
    if !validation_failures.is_empty() {
        info!("");
        info!("🚨 Real Validation Failures ({}):", validation_failures.len());
        for (block, test_type, _error) in validation_failures.iter().take(10) {
            info!("  Block {} ({})", block, test_type);
        }
        if validation_failures.len() > 10 {
            info!("  ... and {} more", validation_failures.len() - 10);
        }
    }
    
    if !infrastructure_failures.is_empty() {
        info!("");
        info!("⚠️  Infrastructure Issues ({}):", infrastructure_failures.len());
        info!("  These are likely transient failures due to RPC issues, not validation problems");
    }
    
    Ok(())
}

fn format_duration(seconds: u64) -> String {
    if seconds < 60 {
        format!("{}s", seconds)
    } else if seconds < 3600 {
        let minutes = seconds / 60;
        let secs = seconds % 60;
        format!("{}m {}s", minutes, secs)
    } else {
        let hours = seconds / 3600;
        let minutes = (seconds % 3600) / 60;
        format!("{}h {}m", hours, minutes)
    }
}

fn build_execution_fixture() -> Result<()> {
    let mut cmd = Command::new("cargo");
    cmd.args(&["build", "-p", "execution-fixture", "--release"]);
    
    // Set LIBCLANG_PATH for macOS
    #[cfg(target_os = "macos")]
    cmd.env("LIBCLANG_PATH", "/Library/Developer/CommandLineTools/usr/lib");
    
    let status = cmd.status()?;
    if !status.success() {
        return Err(eyre::eyre!("Failed to build execution-fixture"));
    }
    
    Ok(())
}

fn spawn_stats_monitor(state: Arc<ValidationState>, multi_progress: MultiProgress) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let stats_bar = multi_progress.add(ProgressBar::new_spinner());
        stats_bar.set_style(
            ProgressStyle::default_spinner()
                .template("{spinner:.green} {msg}")
                .unwrap()
        );
        
        loop {
            tokio::time::sleep(Duration::from_secs(10)).await;
            
            let stats = state.get_stats();
            let eta_formatted = format_duration(stats.eta_seconds);
            let msg = format!(
                "Success: {:.1}% | Speed: {:.1} blocks/min | ETA: {}",
                stats.success_rate,
                stats.blocks_per_minute,
                eta_formatted
            );
            stats_bar.set_message(msg);
        }
    })
}

#[derive(Debug, Serialize, Deserialize)]
struct FinalReport {
    start_block: u64,
    end_block: u64,
    total_blocks: usize,
    completed: usize,
    failed: usize,
    success_rate: f64,
    duration_seconds: u64,
    blocks_per_minute: f64,
    timestamp: chrono::DateTime<chrono::Utc>,
}