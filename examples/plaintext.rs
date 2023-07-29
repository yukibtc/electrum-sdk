// Copyright (c) 2023 Yuki Kishimoto
// Distributed under the MIT software license

use std::time::Duration;

use electrum_sdk::Client;

const TIMEOUT: Duration = Duration::from_secs(10);

#[tokio::main]
async fn main() {
    env_logger::init();

    let client = Client::new("tcp://electrum.blockstream.info:50001", None);

    client.connect(true).await;

    let header = client.block_header(800_000, Some(TIMEOUT)).await.unwrap();
    println!("Block hash: {}", header.block_hash());

    let fee = client.estimate_fee(12, Some(TIMEOUT)).await.unwrap();
    println!("Fee: {fee}");
}
