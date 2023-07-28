// Copyright (c) 2023 Yuki Kishimoto
// Distributed under the MIT software license

use electrum_sdk::Client;

#[tokio::main]
async fn main() {
    let client = Client::new("ssl://electrum.blockstream.info:50002", None);
    let res = client.estimate_fee(6).await;
    println!("{:#?}", res);
}
