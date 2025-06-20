#[cfg(all(test, feature = "std"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_large_payload_transfer() {
    use alloy_primitives::U256;
    use kona_preimage::{
        BidirectionalChannel, OracleReader, OracleServer, PreimageKey, 
        PreimageOracleClient, PreimageOracleServer, PreimageFetcher,
        errors::{PreimageOracleError, PreimageOracleResult},
    };
    use std::{collections::HashMap, sync::Arc};
    use tokio::sync::Mutex;
    
    struct TestFetcher {
        preimages: Arc<Mutex<HashMap<PreimageKey, Vec<u8>>>>,
    }

    #[async_trait::async_trait]
    impl PreimageFetcher for TestFetcher {
        async fn get_preimage(&self, key: PreimageKey) -> PreimageOracleResult<Vec<u8>> {
            let read_lock = self.preimages.lock().await;
            read_lock.get(&key).cloned().ok_or(PreimageOracleError::KeyNotFound)
        }
    }

    // Create a large payload (similar to rollup config size)
    let large_data = vec![0x42; 1500]; // 1500 bytes
    let key = PreimageKey::new_local(U256::from_be_slice(&[42]).to());
    
    let mut preimages = HashMap::new();
    preimages.insert(key, large_data.clone());
    let preimages = Arc::new(Mutex::new(preimages));

    // Create bidirectional channel
    let channel = BidirectionalChannel::new().unwrap();

    // Spawn the oracle server
    let server_preimages = Arc::clone(&preimages);
    let server_task = tokio::spawn(async move {
        let oracle_server = OracleServer::new(channel.host);
        let test_fetcher = TestFetcher { preimages: server_preimages };

        loop {
            match oracle_server.next_preimage_request(&test_fetcher).await {
                Err(PreimageOracleError::IOError(_)) => break,
                Err(e) => panic!("Unexpected error: {:?}", e),
                Ok(_) => {}
            }
        }
    });

    // Client reads the large payload
    let client_task = tokio::spawn(async move {
        let oracle_reader = OracleReader::new(channel.client);
        
        // Read the large data
        let received_data = oracle_reader.get(key).await.unwrap();
        assert_eq!(received_data.len(), large_data.len());
        assert_eq!(received_data, large_data);
        
        println!("Successfully transferred {} bytes!", received_data.len());
    });

    // Wait for client to complete
    client_task.await.unwrap();
    
    // The server task will exit when the channel is closed
    drop(server_task);
}