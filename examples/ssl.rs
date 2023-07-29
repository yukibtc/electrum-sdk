// Copyright (c) 2023 Yuki Kishimoto
// Distributed under the MIT software license

use std::str::FromStr;
use std::time::Duration;

use async_utility::thread;
use bitcoin::{Address, Txid};
use electrum_sdk::Client;

const TIMEOUT: Duration = Duration::from_secs(10);

#[tokio::main]
async fn main() {
    env_logger::init();

    let client = Client::new("ssl://blockstream.info:993", None);

    client.connect(true).await;

    let header = client.block_header(800_000, Some(TIMEOUT)).await.unwrap();
    println!("{}", header.block_hash());

    let fee = client.estimate_fee(6, Some(TIMEOUT)).await.unwrap();
    println!("Fee: {fee}");

    let tx = client
        .get_transaction(
            Txid::from_str("4eacc3a230fb7997f91e437554a048b4bb285e1fcb0e344315da79ec9fe46aaf")
                .unwrap(),
            Some(TIMEOUT),
        )
        .await
        .unwrap();
    println!("{tx:?}");

    client.block_headers_subscribe(Some(TIMEOUT)).await.unwrap();

    let address = Address::from_str("mohjSavDdQYHRYXcS3uS6ttaHP8amyvX78").unwrap();
    let status = client
        .script_subscribe(address.script_pubkey(), Some(TIMEOUT))
        .await
        .unwrap();
    println!("Status: {status:?}");

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
