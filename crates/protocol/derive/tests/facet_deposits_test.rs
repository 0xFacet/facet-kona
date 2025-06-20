use alloy_consensus::{Signed, TxLegacy, TxEnvelope, Receipt, Eip658Value};
use alloy_primitives::{hex, Bytes, Signature, TxKind, U256, Address, Log, LogData};
use kona_protocol::{FACET_INBOX_ADDRESS, FACET_LOG_INBOX_EVENT_SIG, alias_l1_to_l2};
use kona_derive::derive_facet_deposits;

#[test]
fn test_derive_facet_deposits_from_calldata() {
    // Use the known valid payload
    let known_valid_payload = "46e283face7a94111111111111111111111111111111111111111180830f424082123480";
    let facet_data = hex::decode(known_valid_payload).expect("invalid hex");
    let input = Bytes::from(facet_data);

    // Build a dummy legacy tx to FACET_INBOX_ADDRESS
    let legacy = TxLegacy {
        chain_id: Some(1u64),
        nonce: 0,
        gas_price: 1,
        gas_limit: 21000,
        to: TxKind::Call(FACET_INBOX_ADDRESS),
        value: U256::ZERO,
        input: input.clone(),
    };
    let sig = Signature::test_signature();
    let signed = Signed::new_unchecked(legacy, sig, Default::default());
    let envelope = TxEnvelope::Legacy(signed);

    // Build matching receipt with success and no logs
    let receipt = Receipt { status: Eip658Value::Eip658(true), ..Default::default() };

    let (deposits, _, _) = derive_facet_deposits(&[envelope], &[receipt], 16436858, 1, 0u128, 0u128).expect("derive failed");

    // Verify we got exactly one deposit
    assert_eq!(deposits.len(), 1);
    
    // Verify it's a deposit transaction (type 0x7e)
    assert!(!deposits[0].is_empty());
    assert_eq!(deposits[0][0], 0x7e);
    
    // Verify the length is reasonable
    assert_eq!(deposits[0].len(), 89);
}

#[test]
fn test_derive_facet_deposits_from_log() {
    // Use the known valid payload
    let known_valid_payload = "46e283face7a94111111111111111111111111111111111111111180830f424082123480";
    let facet_data = hex::decode(known_valid_payload).expect("invalid hex");
    let input = Bytes::from(facet_data);

    // Build a dummy transaction that does NOT go to FACET_INBOX_ADDRESS
    let dummy_contract = Address::from_slice(&[0x22; 20]);
    let legacy_log = TxLegacy {
        chain_id: Some(1u64),
        nonce: 0,
        gas_price: 1,
        gas_limit: 21000,
        to: TxKind::Call(dummy_contract), // Different address
        value: U256::ZERO,
        input: Bytes::new(), // No calldata
    };
    let sig_log = Signature::test_signature();
    let signed_log = Signed::new_unchecked(legacy_log, sig_log, Default::default());
    let envelope_log = TxEnvelope::Legacy(signed_log);

    // Build receipt with a log containing the facet payload
    let emitting_contract = Address::from_slice(&hex::decode("db8dc4ac38c094746529a14be18d99c18ecaedac").expect("valid hex"));
    let log = Log {
        address: emitting_contract,
        data: LogData::new(
            vec![FACET_LOG_INBOX_EVENT_SIG], // topic0 is the facet inbox event signature
            input.clone(), // log data is the known valid payload
        ).expect("valid log data"),
    };
    
    let receipt_log = Receipt {
        status: Eip658Value::Eip658(true),
        logs: vec![log],
        ..Default::default()
    };

    let (deposits_log, _, _) = derive_facet_deposits(&[envelope_log], &[receipt_log], 16436858, 1, 0u128, 0u128).expect("derive failed");

    // Verify we got exactly one deposit
    assert_eq!(deposits_log.len(), 1);
    
    // Verify it's a deposit transaction (type 0x7e)
    assert!(!deposits_log[0].is_empty());
    assert_eq!(deposits_log[0][0], 0x7e);
    
    // Verify the length is reasonable
    assert_eq!(deposits_log[0].len(), 89);
}

#[test]
fn test_address_aliasing() {
    // Test the specific address aliasing case
    let emitting_contract = Address::from_slice(&hex::decode("db8dc4ac38c094746529a14be18d99c18ecaedac").expect("valid hex"));
    let expected_aliased = Address::from_slice(&hex::decode("ec9ec4ac38c094746529a14be18d99c18ecafebd").expect("valid hex"));
    
    let aliased_from = alias_l1_to_l2(emitting_contract);
    
    assert_eq!(aliased_from, expected_aliased, 
        "Address aliasing failed: expected 0x{}, got 0x{}", 
        hex::encode(expected_aliased), 
        hex::encode(aliased_from)
    );
}

#[test]
fn test_facet_deposits_different_from_addresses() {
    // Test that calldata and log cases produce different "from" addresses
    let known_valid_payload = "46e283face7a94111111111111111111111111111111111111111180830f424082123480";
    let facet_data = hex::decode(known_valid_payload).expect("invalid hex");
    let input = Bytes::from(facet_data);

    // Calldata case
    let legacy = TxLegacy {
        chain_id: Some(1u64),
        nonce: 0,
        gas_price: 1,
        gas_limit: 21000,
        to: TxKind::Call(FACET_INBOX_ADDRESS),
        value: U256::ZERO,
        input: input.clone(),
    };
    let sig = Signature::test_signature();
    let signed = Signed::new_unchecked(legacy, sig, Default::default());
    let envelope = TxEnvelope::Legacy(signed);
    let receipt = Receipt { status: Eip658Value::Eip658(true), ..Default::default() };
    let (deposits_calldata, _, _) = derive_facet_deposits(&[envelope], &[receipt], 16436858, 1, 0u128, 0u128).expect("derive failed");

    // Log case
    let dummy_contract = Address::from_slice(&[0x22; 20]);
    let legacy_log = TxLegacy {
        chain_id: Some(1u64),
        nonce: 0,
        gas_price: 1,
        gas_limit: 21000,
        to: TxKind::Call(dummy_contract),
        value: U256::ZERO,
        input: Bytes::new(),
    };
    let sig_log = Signature::test_signature();
    let signed_log = Signed::new_unchecked(legacy_log, sig_log, Default::default());
    let envelope_log = TxEnvelope::Legacy(signed_log);

    let emitting_contract = Address::from_slice(&hex::decode("db8dc4ac38c094746529a14be18d99c18ecaedac").expect("valid hex"));
    let log = Log {
        address: emitting_contract,
        data: LogData::new(
            vec![FACET_LOG_INBOX_EVENT_SIG],
            input.clone(),
        ).expect("valid log data"),
    };
    let receipt_log = Receipt {
        status: Eip658Value::Eip658(true),
        logs: vec![log],
        ..Default::default()
    };
    let (deposits_log, _, _) = derive_facet_deposits(&[envelope_log], &[receipt_log], 16436858, 1, 0u128, 0u128).expect("derive failed");

    // Both should produce deposits
    assert_eq!(deposits_calldata.len(), 1);
    assert_eq!(deposits_log.len(), 1);
    
    // But they should be different (different "from" addresses)
    assert_ne!(deposits_calldata[0], deposits_log[0], 
        "Calldata and log deposits should be different due to different 'from' addresses");
}

#[test]
fn test_failed_transaction_no_deposits() {
    // Test that failed transactions don't produce deposits
    let known_valid_payload = "46e283face7a94111111111111111111111111111111111111111180830f424082123480";
    let facet_data = hex::decode(known_valid_payload).expect("invalid hex");
    let input = Bytes::from(facet_data);

    let legacy = TxLegacy {
        chain_id: Some(1u64),
        nonce: 0,
        gas_price: 1,
        gas_limit: 21000,
        to: TxKind::Call(FACET_INBOX_ADDRESS),
        value: U256::ZERO,
        input: input.clone(),
    };
    let sig = Signature::test_signature();
    let signed = Signed::new_unchecked(legacy, sig, Default::default());
    let envelope = TxEnvelope::Legacy(signed);

    // Build receipt with FAILED status
    let receipt = Receipt { 
        status: Eip658Value::Eip658(false), // Failed transaction
        ..Default::default() 
    };

    let (deposits, _, _) = derive_facet_deposits(&[envelope], &[receipt], 16436858, 1, 0u128, 0u128).expect("derive failed");

    // Should produce no deposits for failed transactions
    assert_eq!(deposits.len(), 0);
}

#[test]
fn test_facet_payload_values() {
    // Test that the known valid payload decodes to the expected values
    use kona_protocol::decode_facet_payload;
    
    let known_valid_payload = "46e283face7a94111111111111111111111111111111111111111180830f424082123480";
    let facet_data = hex::decode(known_valid_payload).expect("invalid hex");

    let payload = decode_facet_payload(&facet_data, 16436858, false).expect("decode failed");
    
    // Check the expected values
    assert_eq!(payload.data, hex::decode("1234").expect("valid hex"), 
        "Data should be 0x1234");
    
    assert_eq!(payload.to, Some(Address::from_slice(&hex::decode("1111111111111111111111111111111111111111").expect("valid hex"))), 
        "To should be 0x1111111111111111111111111111111111111111");
    
    assert_eq!(payload.gas_limit, 1_000_000, 
        "Gas limit should be 1,000,000");
    
    assert_eq!(payload.value, U256::ZERO, 
        "Value should be 0");
}

#[test]
fn test_facet_mint_calculation() {
    use kona_protocol::FctMintCalculator;
    
    // Use the known valid payload
    let known_valid_payload = "46e283face7a94111111111111111111111111111111111111111180830f424082123480";
    let facet_data = hex::decode(known_valid_payload).expect("invalid hex");
    let input = Bytes::from(facet_data);

    // Build a dummy legacy tx to FACET_INBOX_ADDRESS
    let legacy = TxLegacy {
        chain_id: Some(1u64),
        nonce: 0,
        gas_price: 1,
        gas_limit: 21000,
        to: TxKind::Call(FACET_INBOX_ADDRESS),
        value: U256::ZERO,
        input: input.clone(),
    };
    let sig = Signature::test_signature();
    let signed = Signed::new_unchecked(legacy, sig, Default::default());
    let envelope = TxEnvelope::Legacy(signed);

    // Build matching receipt with success and no logs
    let receipt = Receipt { status: Eip658Value::Eip658(true), ..Default::default() };

    // Use INITIAL_RATE for mint rate and 0 for cumulative data gas (as specified)
    let mint_rate = FctMintCalculator::INITIAL_RATE; // 800_000_000_000_000
    let cumulative_data_gas = 0u128;

    let (deposits, _, _) = derive_facet_deposits(&[envelope], &[receipt], 16436858, 1, mint_rate, cumulative_data_gas).expect("derive failed");

    // Verify we got exactly one deposit
    assert_eq!(deposits.len(), 1);
    
    // Verify it's a deposit transaction (type 0x7e)
    assert!(!deposits[0].is_empty());
    assert_eq!(deposits[0][0], 0x7e);
    
    // Decode the deposit transaction to extract the mint amount
    use alloy_eips::eip2718::Decodable2718;
    use op_alloy_consensus::TxDeposit;
    
    let deposit_data = &deposits[0][1..]; // Skip the 0x7e prefix
    let deposit_tx = TxDeposit::decode_2718(&mut &deposit_data[..]).expect("failed to decode deposit tx");
    
    // Verify the mint amount matches the expected value
    let expected_mint = 460800000000000000u128;
    assert_eq!(deposit_tx.mint, Some(expected_mint), 
        "Expected mint amount {} but got {:?}", expected_mint, deposit_tx.mint);
    
    // Verify the calculation: data_gas_used * mint_rate = expected_mint
    let facet_data_for_verification = hex::decode(known_valid_payload).expect("invalid hex");
    let data_gas_used = FctMintCalculator::calculate_data_gas_used(&facet_data_for_verification, false);
    let calculated_mint = FctMintCalculator::calculate_mint_amount(data_gas_used, mint_rate);
    assert_eq!(calculated_mint, expected_mint, 
        "Mint calculation verification failed: {} * {} = {} (expected {})", 
        data_gas_used, mint_rate, calculated_mint, expected_mint);
}