// Copyright (c) 2023 Yuki Kishimoto
// Distributed under the MIT software license

use electrum_sdk::Client;

#[tokio::main]
async fn main() {
    env_logger::init();

    let client = Client::new("ssl://electrum.blockstream.info:50002", None);

    client.connect(true).await;

    let header = client.block_header(800_000).await.unwrap();
    println!("{}", header.block_hash());

    let res = client.estimate_fee(6).await;
    println!("{:#?}", res);
}
