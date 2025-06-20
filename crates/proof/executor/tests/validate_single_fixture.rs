//! Simple test for validating a single fixture from environment variable
//! 
//! Usage: FIXTURE_PATH=/path/to/fixture.tar.gz cargo test test_validate_single_fixture

use kona_executor::test_utils::run_test_fixture;
use std::{env, path::PathBuf};

#[tokio::test]
async fn test_validate_single_fixture() {
    // Get fixture path from environment variable
    let fixture_path = match env::var("FIXTURE_PATH") {
        Ok(path) => PathBuf::from(path),
        Err(_) => {
            println!("❌ FIXTURE_PATH environment variable not set");
            println!("Usage: FIXTURE_PATH=/path/to/fixture.tar.gz cargo test test_validate_single_fixture");
            panic!("Missing FIXTURE_PATH");
        }
    };
    
    if !fixture_path.exists() {
        panic!("Fixture file does not exist: {:?}", fixture_path);
    }
    
    println!("📦 Validating fixture: {:?}", fixture_path);
    
    // Run the fixture validation
    run_test_fixture(fixture_path).await;
    
    println!("✅ Fixture validation successful!");
}