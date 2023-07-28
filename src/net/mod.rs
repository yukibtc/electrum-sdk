// Copyright (c) 2022-2023 Yuki Kishimoto
// Distributed under the MIT software license

//! Bitcoin P2P Network

use std::io::ErrorKind;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::time::Duration;

use tokio::net::TcpStream;
use tokio_socks::{IntoTargetAddr, TargetAddr};

mod socks;

use self::socks::TpcSocks5Stream;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// I/O error
    #[error("io error: {0}")]
    IO(#[from] std::io::Error),
    /// Socks error
    #[error("socks error: {0}")]
    Socks(#[from] tokio_socks::Error),
    /// Timeout
    #[error("timeout")]
    Timeout,
    /// Invalid DNS name
    #[error("invalid DNS name")]
    InvalidDNSName,
}

pub async fn connect<A>(
    addr: A,
    proxy: Option<SocketAddr>,
    timeout: Option<Duration>,
) -> Result<TcpStream, Error>
where
    A: Into<String>,
{
    let mut use_ssl: bool = false;
    let mut addr: String = addr.into();
    if addr.starts_with("tcp://") {
        addr = addr.replace("tcp://", "");
    } else if addr.starts_with("ssl://") {
        use_ssl = true;
        addr = addr.replace("ssl://", "");
    }

    match proxy {
        Some(proxy) => connect_proxy(addr, proxy, timeout).await,
        None => connect_direct(addr, timeout).await,
    }
}

async fn connect_direct<A>(addr: A, timeout: Option<Duration>) -> Result<TcpStream, Error>
where
    A: ToSocketAddrs,
{
    let timeout = timeout.unwrap_or(Duration::from_secs(60));
    let addr: SocketAddr = addr
        .to_socket_addrs()?
        .next()
        .ok_or(Error::IO(std::io::Error::from(ErrorKind::Other)))?;
    let stream = tokio::time::timeout(timeout, TcpStream::connect(addr))
        .await
        .map_err(|_| Error::Timeout)??;
    Ok(stream)
}

async fn connect_proxy<'a, A>(
    addr: A,
    proxy: SocketAddr,
    timeout: Option<Duration>,
) -> Result<TcpStream, Error>
where
    A: IntoTargetAddr<'a>,
{
    let timeout = timeout.unwrap_or(Duration::from_secs(60));
    let stream = tokio::time::timeout(timeout, TpcSocks5Stream::connect(proxy, addr))
        .await
        .map_err(|_| Error::Timeout)??;
    Ok(stream)
}
