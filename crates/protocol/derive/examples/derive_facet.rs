use alloy_consensus::{Signed, TxLegacy, TxEnvelope};
use alloy_primitives::{hex, Bytes, Signature, TxKind, U256, Address, Log, LogData};
use kona_protocol::{FACET_INBOX_ADDRESS, FACET_LOG_INBOX_EVENT_SIG, alias_l1_to_l2};
use kona_derive::derive_facet_deposits;
use alloy_consensus::{Receipt, Eip658Value};

fn main() {
    // Use the known valid payload
    let known_valid_payload = "46e283face7a94111111111111111111111111111111111111111180830f424082123480";
    let facet_data = hex::decode(known_valid_payload).expect("invalid hex");
    let input = Bytes::from(facet_data);
    
    println!("Testing known valid payload: 0x{}", hex::encode(&input));

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

    let (deposits, new_mint_rate, new_cumulative_gas) = derive_facet_deposits(&[envelope], &[receipt], 16436858, 1, 0u128, 0u128).expect("derive failed");

    println!("Derived {} deposit(s) from calldata", deposits.len());
    println!("New FCT mint rate: {}", new_mint_rate);
    println!("New cumulative L1 data gas: {}", new_cumulative_gas);
    for (idx, dep) in deposits.iter().enumerate() {
        println!("Deposit {}: 0x{}", idx, hex::encode(dep));
        
        // Show basic deposit transaction info
        if dep.len() > 1 && dep[0] == 0x7e {
            println!("  - Type: 0x7e (Deposit transaction)");
            println!("  - Length: {} bytes", dep.len());
            println!("  - Successfully encoded for L2 submission");
        }
    }

    // ------------------------------------------------------
    // Test case #2: Facet transaction via log
    // ------------------------------------------------------
    println!("\n--- Testing facet transaction via log ---");
    
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

    println!("Derived {} deposit(s) from log", deposits_log.len());
    for (idx, dep) in deposits_log.iter().enumerate() {
        println!("Deposit {}: 0x{}", idx, hex::encode(dep));
        
        // Show basic deposit transaction info
        if dep.len() > 1 && dep[0] == 0x7e {
            println!("  - Type: 0x7e (Deposit transaction)");
            println!("  - Length: {} bytes", dep.len());
            println!("  - Successfully encoded for L2 submission");
        }
    }
    
    // Verify that the "from" address is the aliased emitting contract
    let aliased_from = alias_l1_to_l2(emitting_contract);
    let expected_aliased = Address::from_slice(&hex::decode("ec9ec4ac38c094746529a14be18d99c18ecafebd").expect("valid hex"));
    
    if aliased_from == expected_aliased {
        println!("✅ Address aliasing is correct!");
    } else {
        println!("❌ Address aliasing mismatch!");
        println!("Expected: 0x{}", hex::encode(expected_aliased));
        println!("Actual: 0x{}", hex::encode(aliased_from));
    }
}