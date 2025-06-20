use alloc::vec::Vec;
use alloy_consensus::{Receipt, TxEnvelope, Eip658Value, Transaction};
use alloy_eips::Encodable2718;
use alloy_primitives::{Address, B256, Bytes, Log};
use kona_protocol::{decode_facet_payload, alias_l1_to_l2, FACET_INBOX_ADDRESS, FACET_LOG_INBOX_EVENT_SIG, FctMintCalculator};
use crate::errors::PipelineEncodingError;

/// Derive Optimism `0x7e` deposit transactions from facet inbox calldata + event logs.
///
/// * `txs`         – list of L1 transactions in canonical order (index already implied)
/// * `receipts`    – receipts matching `txs` by index
/// * `l2_chain_id` – Optimism chain id we expect inside the facet RLP
/// * `l2_block_number` – current L2 block number for mint calculations
/// * `fct_mint_rate` – facet mint rate from parent block
/// * `fct_mint_period_l1_data_gas` – facet mint period L1 data gas from parent block
///
/// Returns (deposit_transactions, new_mint_rate, new_cumulative_l1_data_gas)
pub fn derive_facet_deposits(
    txs: &[TxEnvelope],
    receipts: &[Receipt],
    l2_chain_id: u64,
    l2_block_number: u64,
    fct_mint_rate: u128,
    fct_mint_period_l1_data_gas: u128,
) -> Result<(Vec<Bytes>, u128, u128), PipelineEncodingError> {
    debug_assert_eq!(txs.len(), receipts.len(), "txs/receipts length mismatch");
    
    tracing::info!(
        target: "facet_deposits",
        "derive_facet_deposits: Processing {} transactions for L2 block {}",
        txs.len(),
        l2_block_number
    );

    // Step 1: Collect all facet payloads with their metadata
    let mut facet_payloads = Vec::new();
    let mut facet_inbox_count = 0;
    let mut total_calldata_txs = 0;
    let mut sample_addresses = Vec::new();

    for (tx, receipt) in txs.iter().zip(receipts) {
        if receipt.status != Eip658Value::Eip658(true) {
            continue; // failed L1 txs do not produce deposits
        }

        let tx_hash = *match tx {
            TxEnvelope::Legacy(tx) => tx.hash(),
            TxEnvelope::Eip2930(tx) => tx.hash(),
            TxEnvelope::Eip1559(tx) => tx.hash(),
            TxEnvelope::Eip4844(tx) => tx.hash(),
            _ => &B256::ZERO,
        };

        // ------------------------------------------------------
        // path #1 – calldata to FACET_INBOX_ADDRESS
        // ------------------------------------------------------
        total_calldata_txs += 1;
        let (maybe_to, input): (Option<Address>, &Bytes) = match tx {
            TxEnvelope::Legacy(tx) => (Option::<Address>::from(tx.tx().to), &tx.tx().input),
            TxEnvelope::Eip2930(tx) => (Option::<Address>::from(tx.tx().to), &tx.tx().input),
            TxEnvelope::Eip1559(tx) => (Option::<Address>::from(tx.tx().to), &tx.tx().input),
            TxEnvelope::Eip4844(tx) => (Option::<Address>::from(tx.tx().to()), tx.tx().input()),
            _ => (None, &Bytes::new()),
        };
        
        // Collect sample addresses for debugging
        if sample_addresses.len() < 5 {
            if let Some(to) = maybe_to {
                sample_addresses.push(to);
            }
        }

        if maybe_to == Some(FACET_INBOX_ADDRESS) && !input.is_empty() {
            facet_inbox_count += 1;
            tracing::debug!(
                target: "facet_deposits",
                "Found calldata to FACET_INBOX_ADDRESS in tx {}",
                tx_hash
            );
            // Try to decode the facet payload, skip if invalid
            match decode_facet_payload(input, l2_chain_id, false) {
                Ok(payload) => {
                    let from = tx.recover_signer().unwrap_or_default();
                    tracing::info!(
                        target: "facet_deposits",
                        "Successfully decoded facet payload from calldata in tx {}",
                        tx_hash
                    );
                    facet_payloads.push((payload, from, tx_hash));
                },
                Err(e) => {
                    tracing::debug!(
                        target: "facet_deposits",
                        "Failed to decode facet payload from calldata in tx {}: {:?}",
                        tx_hash,
                        e
                    );
                    // Skip invalid facet transactions (wrong prefix, invalid RLP, etc.)
                    // This handles cases like gzipped data or other malformed inputs
                }
            }
            continue; // one deposit per tx
        }

        // ------------------------------------------------------
        // path #2 – first log with inbox topic0
        // ------------------------------------------------------
        let mut first_log: Option<&Log> = None;
        for l in &receipt.logs {
            if l.data.topics().first().is_some_and(|t| *t == FACET_LOG_INBOX_EVENT_SIG) {
                first_log = Some(l);
                break;
            }
        }
        if let Some(log) = first_log {
            tracing::debug!(
                target: "facet_deposits",
                "Found facet log event in tx {}",
                tx_hash
            );
            // Try to decode the facet payload from log, skip if invalid
            match decode_facet_payload(&log.data.data, l2_chain_id, true) {
                Ok(payload) => {
                    let from = alias_l1_to_l2(log.address);
                    tracing::info!(
                        target: "facet_deposits",
                        "Successfully decoded facet payload from log in tx {}",
                        tx_hash
                    );
                    facet_payloads.push((payload, from, tx_hash));
                },
                Err(e) => {
                    tracing::debug!(
                        target: "facet_deposits",
                        "Failed to decode facet payload from log in tx {}: {:?}",
                        tx_hash,
                        e
                    );
                    // Skip invalid facet log data (wrong prefix, invalid RLP, etc.)
                }
            }
        }
    }

    // Step 2: Calculate new mint rate based on FCT mint calculation
    let new_mint_rate = FctMintCalculator::compute_new_rate(
        l2_block_number,
        fct_mint_rate,
        fct_mint_period_l1_data_gas,
    );

    // Step 3: Assign mint amounts to each facet transaction
    for (payload, _, _) in &mut facet_payloads {
        let mint_amount = FctMintCalculator::calculate_mint_amount(
            payload.l1_data_gas_used,
            new_mint_rate,
        );
        payload.set_mint(mint_amount);
    }

    // Step 4: Calculate new cumulative L1 data gas
    let batch_l1_data_gas: u64 = facet_payloads.iter()
        .map(|(payload, _, _)| payload.l1_data_gas_used)
        .sum();

    let new_cumulative_l1_data_gas = if FctMintCalculator::is_first_block_in_period(l2_block_number) {
        batch_l1_data_gas as u128
    } else {
        fct_mint_period_l1_data_gas + batch_l1_data_gas as u128
    };

    // Step 5: Convert payloads to deposit transactions
    let mut out = Vec::with_capacity(facet_payloads.len());
    for (payload, from, source_hash) in facet_payloads {
        let dep = payload.into_deposit(from, source_hash);
        let mut buf = Vec::with_capacity(dep.eip2718_encoded_length());
        dep.encode_2718(&mut buf);
        out.push(buf.into());
    }
    
    tracing::info!(
        target: "facet_deposits",
        "derive_facet_deposits: Produced {} deposit transactions for L2 block {} (checked {} txs, {} to FACET_INBOX_ADDRESS)",
        out.len(),
        l2_block_number,
        total_calldata_txs,
        facet_inbox_count
    );
    
    if facet_inbox_count == 0 && !sample_addresses.is_empty() {
        tracing::debug!(
            target: "facet_deposits",
            "Sample L1 transaction destinations (looking for {:?}): {:?}",
            FACET_INBOX_ADDRESS,
            sample_addresses
        );
    }

    Ok((out, new_mint_rate, new_cumulative_l1_data_gas))
} 