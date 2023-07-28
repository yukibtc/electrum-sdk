// Copyright (c) 2022-2023 Yuki Kishimoto
// Distributed under the MIT software license

//! Network

use std::io::ErrorKind;
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::Arc;
use std::time::Duration;

use tokio::net::TcpStream;
use tokio_rustls::client::TlsStream;
use tokio_rustls::rustls::{ClientConfig, OwnedTrustAnchor, RootCertStore, ServerName};
use tokio_rustls::TlsConnector;
use tokio_socks::{IntoTargetAddr, TargetAddr};

mod socks;
mod stream;

use self::socks::TpcSocks5Stream;
pub use self::stream::MaybeTlsStream;

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
) -> Result<MaybeTlsStream<TcpStream>, Error>
where
    A: Into<String>,
{
    let mut ssl: bool = false;
    let mut addr: String = addr.into();
    if addr.starts_with("tcp://") {
        addr = addr.replace("tcp://", "");
    } else if addr.starts_with("ssl://") {
        ssl = true;
        addr = addr.replace("ssl://", "");
    }

    match proxy {
        Some(proxy) => connect_proxy(addr, proxy, ssl, timeout).await,
        None => connect_direct(addr, ssl, timeout).await,
    }
}

async fn connect_direct<'a, A>(
    addr: A,
    ssl: bool,
    timeout: Option<Duration>,
) -> Result<MaybeTlsStream<TcpStream>, Error>
where
    A: ToSocketAddrsDomain<'a>,
{
    let timeout = timeout.unwrap_or(Duration::from_secs(60));
    let _addr: SocketAddr = addr
        .to_socket_addrs()?
        .next()
        .ok_or(Error::IO(std::io::Error::from(ErrorKind::Other)))?;
    let stream = tokio::time::timeout(timeout, TcpStream::connect(_addr))
        .await
        .map_err(|_| Error::Timeout)??;

    if ssl {
        Ok(MaybeTlsStream::Rustls(Box::new(
            connect_with_tls(addr.domain().ok_or(Error::InvalidDNSName)?, stream).await?,
        )))
    } else {
        Ok(MaybeTlsStream::Plain(stream))
    }
}

async fn connect_proxy<'a, A>(
    addr: A,
    proxy: SocketAddr,
    ssl: bool,
    timeout: Option<Duration>,
) -> Result<MaybeTlsStream<TcpStream>, Error>
where
    A: ToSocketAddrsDomain<'a> + Clone,
{
    let timeout = timeout.unwrap_or(Duration::from_secs(60));
    let stream = tokio::time::timeout(timeout, TpcSocks5Stream::connect(proxy, addr.clone()))
        .await
        .map_err(|_| Error::Timeout)??;

    if ssl {
        Ok(MaybeTlsStream::Rustls(Box::new(
            connect_with_tls(addr.domain().ok_or(Error::InvalidDNSName)?, stream).await?,
        )))
    } else {
        Ok(MaybeTlsStream::Plain(stream))
    }
}

async fn connect_with_tls<S>(domain: S, stream: TcpStream) -> Result<TlsStream<TcpStream>, Error>
where
    S: Into<String>,
{
    let mut root_cert_store = RootCertStore::empty();
    root_cert_store.add_server_trust_anchors(webpki_roots::TLS_SERVER_ROOTS.0.iter().map(|ta| {
        OwnedTrustAnchor::from_subject_spki_name_constraints(
            ta.subject,
            ta.spki,
            ta.name_constraints,
        )
    }));
    let config = ClientConfig::builder()
        .with_safe_defaults()
        .with_root_certificates(root_cert_store)
        .with_no_client_auth();
    let connector = TlsConnector::from(Arc::new(config));
    let domain: String = domain.into();
    let domain = ServerName::try_from(domain.as_str()).map_err(|_| Error::InvalidDNSName)?;
    Ok(connector.connect(domain, stream).await?)
}

/// A trait for [`ToSocketAddrs`](https://doc.rust-lang.org/std/net/trait.ToSocketAddrs.html) that
/// can also be turned into a domain. Used when an SSL client needs to validate the server's
/// certificate.
pub trait ToSocketAddrsDomain<'a>: ToSocketAddrs + IntoTargetAddr<'a> {
    /// Returns the domain, if present
    fn domain(&self) -> Option<String> {
        None
    }
}

impl ToSocketAddrsDomain<'static> for String {
    fn domain(&self) -> Option<String> {
        self.split(':').next().map(|s| s.to_string())
    }
}

impl<'a> ToSocketAddrsDomain<'a> for &'a str {
    fn domain(&self) -> Option<String> {
        self.split(':').next().map(|s| s.to_string())
    }
}

impl<'a> ToSocketAddrsDomain<'a> for (&'a str, u16) {
    fn domain(&self) -> Option<String> {
        self.0.domain()
    }
}

impl<'a> ToSocketAddrsDomain<'a> for TargetAddr<'a> {
    fn domain(&self) -> Option<String> {
        match self {
            TargetAddr::Ip(_) => None,
            TargetAddr::Domain(domain, _) => Some(domain.to_string()),
        }
    }
}

macro_rules! impl_to_socket_addrs_domain {
    ( $ty:ty ) => {
        impl<'a> ToSocketAddrsDomain<'a> for $ty {}
    };
}

impl_to_socket_addrs_domain!(std::net::SocketAddr);
impl_to_socket_addrs_domain!(std::net::SocketAddrV4);
impl_to_socket_addrs_domain!(std::net::SocketAddrV6);
impl_to_socket_addrs_domain!((std::net::IpAddr, u16));
impl_to_socket_addrs_domain!((std::net::Ipv4Addr, u16));
impl_to_socket_addrs_domain!((std::net::Ipv6Addr, u16));
