// Copyright (c) 2023 Yuki Kishimoto
// Distributed under the MIT software license

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::time::Duration;

use electrum_sdk::Client;

const TIMEOUT: Duration = Duration::from_secs(10);

#[tokio::main]
async fn main() {
    env_logger::init();

    let proxy = Some(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 9050)));
    let client = Client::new(
        "tcp://explorerzydxu5ecjrkwceayqybizmpjjznk5izmitf2modhcusuqlid.onion:110",
        proxy,
    );

    client.connect(true).await;

    let header = client.block_header(800_000, Some(TIMEOUT)).await.unwrap();
    println!("{}", header.block_hash());
}
