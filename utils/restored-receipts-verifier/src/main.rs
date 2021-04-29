use std::collections::{HashMap, HashSet};
use std::iter::FromIterator;
use near_chain::{ChainStore, ChainStoreAccess, RuntimeAdapter, ReceiptResult};
use near_primitives::borsh::BorshSerialize;
use near_primitives::receipt::Receipt;
use near_store::create_store;
use neard::{get_default_home, get_store_path, load_config, NightshadeRuntime};
use std::io::Error;
use std::path::Path;
use near_primitives::version::{ProtocolFeature, PROTOCOL_FEATURES_TO_VERSION_MAPPING};
use clap::{App, Arg};
use near_primitives::hash::CryptoHash;

fn main() -> Result<(), Error> {
    let default_home = get_default_home();
    let matches = App::new("restored-receipts-verifier")
        .arg(
            Arg::with_name("home")
                .long("home")
                .default_value(&default_home)
                .help("Directory for config and data (default \"~/.near\")")
                .takes_value(true),
        ).get_matches();

    println!("Start");

    let shard_id: u64 = 0;
    let home_dir = matches.value_of("home").map(|dir| Path::new(dir)).unwrap();
    let near_config = load_config(&home_dir);
    let store = create_store(&get_store_path(&home_dir));
    let mut chain_store = ChainStore::new(store.clone(), near_config.genesis.config.genesis_height);
    let runtime = NightshadeRuntime::new(
        &home_dir,
        store,
        &near_config.genesis,
        near_config.client_config.tracked_accounts.clone(),
        near_config.client_config.tracked_shards.clone(),
    );

    let mut receipts_missing: Vec<Receipt> = vec![];
    // First and last heights for which lost receipts were observed
    let height_first: u64 = 34691244;
    let height_last: u64 = chain_store.get_latest_known().expect("Couldn't get upper bound for block height").height;
    for height in height_first..height_last {
        let block_hash_result = chain_store.get_block_hash_by_height(height);
        if block_hash_result.is_err() {
            println!("{} does not exist, skip", height);
            continue;
        }
        let block_hash = block_hash_result.unwrap();

        let block = chain_store.get_block(&block_hash).unwrap().clone();
        if block.chunks()[shard_id as usize].height_included() == height {
            println!("{} included, skip", height);
            continue;
        }

        if runtime.get_epoch_protocol_version(block.header().epoch_id()).unwrap() >= PROTOCOL_FEATURES_TO_VERSION_MAPPING[&ProtocolFeature::FixApplyChunks] {
            println!("Found block height {} for which apply_chunks was already fixed. Finishing...", height);
            break;
        }

        // Version for master
        // if runtime.get_epoch_protocol_version(block.header().epoch_id()) >= ProtocolFeature::FixApplyChunks.protocol_version() {
        //
        // }

        let chunk_extra =
            chain_store.get_chunk_extra(block.header().prev_hash(), shard_id).unwrap().clone();
        // let apply_result = apply_transactions_for_not_included_chunk(&runtime, shard_id, &chunk_extra, &block);
        let apply_result = runtime
            .apply_transactions(
                shard_id,
                &chunk_extra.state_root,
                block.header().height(),
                block.header().raw_timestamp(),
                block.header().prev_hash(),
                &block.hash(),
                &[],
                &[],
                &chunk_extra.validator_proposals,
                block.header().gas_price(),
                chunk_extra.gas_limit,
                &block.header().challenges_result(),
                *block.header().random_value(),
                false,
            )
            .unwrap();

        let receipts_missing_after_apply: Vec<Receipt> =
            apply_result.receipt_result.values().cloned().into_iter().flatten().collect();
        receipts_missing.extend(receipts_missing_after_apply.into_iter());
        println!("{} applied", height);
    }

    // Temporary, to save to file
    let mut receipt_result_missing: ReceiptResult = HashMap::default();
    receipt_result_missing.insert(shard_id, receipts_missing.clone());
    let receipt_result_missing_str = serde_json::to_string::<ReceiptResult>(&receipt_result_missing)?;
    // let bytes = receipts_missing.try_to_vec().unwrap();
    std::fs::write("./fix_apply_chunks_receipts.json", receipt_result_missing_str);

    // Check that receipts from repo were actually generated and lost
    // let receipt_result_in_repo_json = include_str!("../../../neard/res/fix_apply_chunks_receipts.json");
    let receipt_result_in_repo_json = include_str!("../../../fix_apply_chunks_receipts.json");
    let receipt_result_in_repo = serde_json::from_str::<ReceiptResult>(receipt_result_in_repo_json)
        .expect("File with receipts restored after apply_chunks fix have to be correct");
    let receipts_in_repo = receipt_result_in_repo.get(&shard_id).unwrap();
    let receipt_hashes_in_repo = HashSet::<_>::from_iter(receipts_in_repo.into_iter().map(|receipt| receipt.get_hash()));
    let receipt_hashes_missing = HashSet::<_>::from_iter(receipts_missing.into_iter().map(|receipt| receipt.get_hash()));

    let receipt_hashes_not_verified: Vec<CryptoHash> = receipt_hashes_in_repo.difference(&receipt_hashes_missing).cloned().collect();
    assert_eq!(receipt_hashes_not_verified.len(), 0, "Some of receipt hashes in repo were not verified successfully: {:?}", receipt_hashes_not_verified);

    Ok(())
}
