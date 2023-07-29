// Copyright (c) 2023 Yuki Kishimoto
// Distributed under the MIT software license

use std::str::FromStr;
use std::time::Duration;

use async_utility::thread;
use bitcoin::Txid;
use electrum_sdk::Client;

const TIMEOUT: Duration = Duration::from_secs(10);

#[tokio::main]
async fn main() {
    env_logger::init();

    let client = Client::new("ssl://electrum.blockstream.info:50002", None);

    client.connect(true).await;

    let header = client.block_header(800_000, Some(TIMEOUT)).await.unwrap();
    println!("{}", header.block_hash());

    let fee = client.estimate_fee(6, Some(TIMEOUT)).await.unwrap();
    println!("Fee: {fee}");

    let tx = client
        .get_transaction(
            Txid::from_str("8a1fba5a1d49eb7f7f7ec8524e1472dd6b5369df87fe08b2ed55abd7c35a4f43")
                .unwrap(),
            Some(TIMEOUT),
        )
        .await
        .unwrap();
    println!("{tx:?}");

    client.block_headers_subscribe(Some(TIMEOUT)).await.unwrap();

    /* loop {
        let mut notifications = client.notifications();
        while let Ok(client_notification) = notifications.recv().await {
            match client_notification {
                ClientNotification::Notification(notification) => println!("{notification:?}"),
                other => println!("Other: {other:?}"),
            }
        }
    } */

    loop {
        thread::sleep(Duration::from_secs(60)).await
    }
}
