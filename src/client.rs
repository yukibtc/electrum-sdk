// Copyright (c) 2023 Yuki Kishimoto
// Distributed under the MIT software license

//! Electrum Client

use std::collections::HashMap;
use std::fmt;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_utility::{thread, time};
use bitcoin::consensus::deserialize;
use bitcoin::hashes::hex::FromHex;
use bitcoin::BlockHeader;
use crossbeam_channel::{bounded, Receiver as CbReceiver, RecvTimeoutError, Sender as CbSender};
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc::{self, Receiver, Sender};
use tokio::sync::oneshot;
use tokio::sync::Mutex;

use crate::{net, types::*};

type Message = (ClientEvent, Option<oneshot::Sender<bool>>);

#[derive(Debug, Error)]
pub enum Error {
    /// Channel timeout
    #[error("channel timeout")]
    ChannelTimeout,
    /// Message response timeout
    #[error("recv message response timeout")]
    RecvTimeout,
    ///
    #[error(transparent)]
    CbRecvTimeout(#[from] RecvTimeoutError),
    /// Generic timeout
    #[error("timeout")]
    Timeout,
    /// Message not sent
    #[error("message not sent")]
    MessageNotSent,
    /// Impossible to receive oneshot message
    #[error("impossible to recv msg")]
    OneShotRecvError,
}

/// Client connection status
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Status {
    #[default]
    Initialized,
    Connected,
    Connecting,
    Disconnected,
    Stopped,
    Terminated,
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Initialized => write!(f, "Initialized"),
            Self::Connected => write!(f, "Connected"),
            Self::Connecting => write!(f, "Connecting"),
            Self::Disconnected => write!(f, "Disconnected"),
            Self::Stopped => write!(f, "Stopped"),
            Self::Terminated => write!(f, "Terminated"),
        }
    }
}

/// Client event
#[derive(Debug)]
pub enum ClientEvent {
    /// Send request
    SendRequest(Request),
    /// Close
    Close,
    /// Stop
    Stop,
    /// Completely disconnect
    Terminate,
}

#[derive(Debug, Clone)]
pub struct ClientChannels {
    header: (CbSender<BlockHeader>, CbReceiver<BlockHeader>),
    estimate_fee: (CbSender<f64>, CbReceiver<f64>),
}

impl Default for ClientChannels {
    fn default() -> Self {
        Self {
            header: bounded::<BlockHeader>(1),
            estimate_fee: bounded::<f64>(1),
        }
    }
}

/// Electrum Client
#[derive(Debug, Clone)]
pub struct Client {
    addr: String,
    proxy: Option<SocketAddr>,
    status: Arc<Mutex<Status>>,
    last_id: Arc<AtomicUsize>,
    channels: ClientChannels,
    sender: Sender<Message>,
    receiver: Arc<Mutex<Receiver<Message>>>,
    inventory: Arc<Mutex<HashMap<usize, String>>>,
}

impl Client {
    /// New Client
    pub fn new<S>(addr: S, proxy: Option<SocketAddr>) -> Self
    where
        S: Into<String>,
    {
        let (sender, receiver) = mpsc::channel::<Message>(1024);

        Self {
            addr: addr.into(),
            proxy,
            status: Arc::new(Mutex::new(Status::default())),
            last_id: Arc::new(AtomicUsize::new(0)),
            channels: ClientChannels::default(),
            sender,
            receiver: Arc::new(Mutex::new(receiver)),
            inventory: Arc::new(Mutex::new(HashMap::new())),
        }
    }
    pub fn addr(&self) -> String {
        self.addr.clone()
    }

    pub fn proxy(&self) -> Option<SocketAddr> {
        self.proxy
    }

    pub async fn status(&self) -> Status {
        let status = self.status.lock().await;
        status.clone()
    }

    async fn set_status(&self, status: Status) {
        let mut s = self.status.lock().await;
        *s = status;
    }

    pub async fn is_connected(&self) -> bool {
        self.status().await == Status::Connected
    }

    /// Connect to electrum server and keep alive connection
    pub async fn connect(&self, wait_for_connection: bool) {
        if let Status::Initialized | Status::Stopped | Status::Terminated = self.status().await {
            if wait_for_connection {
                self.try_connect().await
            } else {
                // Update status
                self.set_status(Status::Disconnected).await;
            }

            let client = self.clone();
            thread::spawn(async move {
                loop {
                    log::debug!(
                        "{} channel capacity: {}",
                        client.addr(),
                        client.sender.capacity()
                    );

                    // Schedule relay for termination
                    // Needed to terminate the auto reconnect loop, also if the relay is not connected yet.
                    /* if client.is_scheduled_for_termination() {
                        client.set_status(Status::Terminated).await;
                        client.schedule_for_termination(false);
                        log::debug!("Auto connect loop terminated for {} [schedule]", client.url);
                        break;
                    } */

                    // Check status
                    match client.status().await {
                        Status::Disconnected => client.try_connect().await,
                        Status::Terminated => {
                            log::debug!("Auto connect loop terminated for {}", client.addr);
                            break;
                        }
                        _ => (),
                    };

                    thread::sleep(Duration::from_secs(20)).await;
                }
            });
        }
    }

    async fn try_connect(&self) {
        let addr: String = self.addr();

        // Set Status to `Connecting`
        self.set_status(Status::Connecting).await;
        log::debug!("Connecting to {}", addr);

        // Connect
        match net::connect(self.addr(), self.proxy, None).await {
            Ok(stream) => {
                let (read, mut write) = tokio::io::split(stream);

                self.set_status(Status::Connected).await;
                log::info!("Connected to {}", addr);

                let client = self.clone();
                thread::spawn(async move {
                    log::debug!("Relay Event Thread Started");
                    let mut rx = client.receiver.lock().await;
                    while let Some((event, oneshot_sender)) = rx.recv().await {
                        match event {
                            ClientEvent::SendRequest(req) => {
                                let json = req.as_json();
                                log::debug!("Sending message {json}");
                                match write.write_all(&req.as_bytes().unwrap()).await {
                                    Ok(_) => {
                                        if let Some(sender) = oneshot_sender {
                                            if let Err(e) = sender.send(true) {
                                                log::error!(
                                                    "Impossible to send oneshot msg: {}",
                                                    e
                                                );
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        log::error!(
                                            "Impossible to send msg to {}: {}",
                                            client.addr(),
                                            e.to_string()
                                        );
                                        if let Some(sender) = oneshot_sender {
                                            if let Err(e) = sender.send(false) {
                                                log::error!(
                                                    "Impossible to send oneshot msg: {}",
                                                    e
                                                );
                                            }
                                        }
                                        break;
                                    }
                                }
                            }
                            ClientEvent::Close => {
                                let _ = write.shutdown().await;
                                client.set_status(Status::Disconnected).await;
                                log::info!("Disconnected from {}", addr);
                                break;
                            }
                            ClientEvent::Stop => {
                                /* if client.is_scheduled_for_stop() {
                                    let _ = ws_tx.close().await;
                                    client.set_status(Status::Stopped).await;
                                    client.schedule_for_stop(false);
                                    log::info!("Stopped {}", addr);
                                    break;
                                } */
                            }
                            ClientEvent::Terminate => {
                                /* if client.is_scheduled_for_termination() {
                                    // Unsubscribe from client
                                    if let Err(e) = client.unsubscribe(None).await {
                                        log::error!(
                                            "Impossible to unsubscribe from {}: {}",
                                            client.addr(),
                                            e.to_string()
                                        )
                                    }
                                    // Close stream
                                    let _ = ws_tx.close().await;
                                    client.set_status(Status::Terminated).await;
                                    client.schedule_for_termination(false);
                                    log::info!("Completely disconnected from {}", addr);
                                    break;
                                } */
                            }
                        }
                    }
                    log::debug!("Exited from Client Event Thread");
                });

                let client = self.clone();
                thread::spawn(async move {
                    log::debug!("Relay Message Thread Started");

                    let mut reader = BufReader::new(read);
                    let mut data = String::new();
                    loop {
                        data.clear();

                        reader.read_line(&mut data).await.unwrap();

                        match Response::from_json(&data) {
                            Ok(res) => {
                                log::trace!("Received response: {data}");

                                // Get method from inventory by ID and deserialize
                                let mut inventory = client.inventory.lock().await;

                                if let Some(e) = res.error {
                                    log::error!("Received error: {e}");
                                    inventory.remove(&res.id);
                                } else if let Some(result) = res.result {
                                    match inventory.get(&res.id) {
                                        Some(method) => match method.as_str() {
                                            "blockchain.block.header" => {
                                                let data =
                                                    Vec::<u8>::from_hex(result.as_str().unwrap())
                                                        .unwrap();
                                                let header: BlockHeader =
                                                    deserialize(&data).unwrap();
                                                client.channels.header.0.send(header).unwrap();
                                            }
                                            "blockchain.estimatefee" => {
                                                let fee: f64 =
                                                    serde_json::from_value(result).unwrap();
                                                client.channels.estimate_fee.0.send(fee).unwrap();
                                            }
                                            _ => log::warn!("NOT IMPLEMENTED"),
                                        },
                                        None => log::error!("ID not found in inventory"),
                                    }
                                }
                            }
                            Err(err) => {
                                log::error!("{err}");
                                break;
                            }
                        };
                    }

                    log::debug!("Exited from Message Thread of {}", client.addr);

                    if let Err(err) = client.disconnect().await {
                        log::error!("Impossible to disconnect {}: {}", client.addr, err);
                    }
                });
            }
            Err(err) => {
                self.set_status(Status::Disconnected).await;
                log::error!("Impossible to connect to {}: {}", addr, err);
            }
        };
    }

    fn send_client_event(
        &self,
        relay_msg: ClientEvent,
        sender: Option<oneshot::Sender<bool>>,
    ) -> Result<(), Error> {
        self.sender
            .try_send((relay_msg, sender))
            .map_err(|_| Error::MessageNotSent)
    }

    /// Disconnect from relay and set status to 'Disconnected'
    async fn disconnect(&self) -> Result<(), Error> {
        let status = self.status().await;
        if status.ne(&Status::Disconnected)
            && status.ne(&Status::Stopped)
            && status.ne(&Status::Terminated)
        {
            self.send_client_event(ClientEvent::Close, None)?;
        }
        Ok(())
    }

    /* /// Disconnect from relay and set status to 'Stopped'
    pub async fn stop(&self) -> Result<(), Error> {
        self.schedule_for_stop(true);
        let status = self.status().await;
        if status.ne(&Status::Disconnected)
            && status.ne(&Status::Stopped)
            && status.ne(&Status::Terminated)
        {
            self.send_client_event(ClientEvent::Stop, None)?;
        }
        Ok(())
    }

    /// Disconnect from relay and set status to 'Terminated'
    pub async fn terminate(&self) -> Result<(), Error> {
        self.schedule_for_termination(true);
        let status = self.status().await;
        if status.ne(&Status::Disconnected)
            && status.ne(&Status::Stopped)
            && status.ne(&Status::Terminated)
        {
            self.send_client_event(ClientEvent::Terminate, None)?;
        }
        Ok(())
    } */

    pub async fn send_msg(&self, msg: Request, wait: Option<Duration>) -> Result<(), Error> {
        match wait {
            Some(timeout) => {
                let (tx, rx) = oneshot::channel::<bool>();
                self.send_client_event(ClientEvent::SendRequest(msg), Some(tx))?;
                match time::timeout(Some(timeout), rx).await {
                    Some(result) => match result {
                        Ok(val) => {
                            if val {
                                Ok(())
                            } else {
                                Err(Error::MessageNotSent)
                            }
                        }
                        Err(_) => Err(Error::OneShotRecvError),
                    },
                    _ => Err(Error::RecvTimeout),
                }
            }
            None => self.send_client_event(ClientEvent::SendRequest(msg), None),
        }
    }

    /// Get block header
    pub async fn block_header(&self, height: usize) -> Result<BlockHeader, Error> {
        let next_id = self.last_id.fetch_add(1, Ordering::SeqCst);
        let req = Request::new(
            next_id,
            "blockchain.block.header",
            vec![Param::Usize(height)],
        );

        {
            let mut inventory = self.inventory.lock().await;
            inventory.insert(req.id, req.method.clone());
        }

        self.send_msg(req, Some(Duration::from_secs(30))).await?;

        Ok(self
            .channels
            .header
            .1
            .recv_timeout(Duration::from_secs(30))?)
    }

    /// Estimate fee
    pub async fn estimate_fee(&self, number: usize) -> Result<f64, Error> {
        let next_id = self.last_id.fetch_add(1, Ordering::SeqCst);
        let req = Request::new(
            next_id,
            "blockchain.estimatefee",
            vec![Param::Usize(number)],
        );

        {
            let mut inventory = self.inventory.lock().await;
            inventory.insert(req.id, req.method.clone());
        }

        self.send_msg(req, Some(Duration::from_secs(30))).await?;

        Ok(self
            .channels
            .estimate_fee
            .1
            .recv_timeout(Duration::from_secs(30))?)
    }
}
