// Copyright (c) 2023 Yuki Kishimoto
// Distributed under the MIT software license

use electrum_sdk::Client;

#[tokio::main]
async fn main() {
    env_logger::init();

    let client = Client::new("tcp://electrum.blockstream.info:50001", None);

    client.connect(true).await;

    let header = client.block_header(800_000).await.unwrap();
    println!("Block hash: {}", header.block_hash());

    let fee = client.estimate_fee(12).await.unwrap();
    println!("Fee: {fee}");
}
