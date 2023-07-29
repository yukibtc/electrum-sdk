// Copyright (c) 2023 Yuki Kishimoto
// Distributed under the MIT software license

//! Electrum Client

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_utility::{thread, time};
use bitcoin::{BlockHeader, Script, Transaction, Txid};
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc::{self, Receiver, Sender};
use tokio::sync::Mutex;
use tokio::sync::{broadcast, oneshot};

use crate::net;
use crate::types::{
    GetBalanceRes, GetHeadersRes, GetHistoryRes, GetMerkleRes, JsonRpcMsg, ListUnspentRes,
    Notification, Request, Response, ScriptStatus, ServerFeaturesRes,
};

type Message = (ClientEvent, Option<oneshot::Sender<bool>>);

#[derive(Debug, Error)]
pub enum Error {
    /// Channel timeout
    #[error("channel timeout")]
    ChannelTimeout,
    /// Message response timeout
    #[error("recv message response timeout")]
    RecvTimeout,
    /// Generic timeout
    #[error("timeout")]
    Timeout,
    /// Message not sent
    #[error("message not sent")]
    MessageNotSent,
    /// Impossible to receive oneshot message
    #[error("impossible to recv msg")]
    OneShotRecvError,
    /// Invalid response
    #[error("invalid response")]
    InvalidResponse,
    /// Already subscribed to script
    #[error("Already subscribed to the notifications of script {0}")]
    AlreadySubscribed(Script),
    /// Not subscribed to the notifications of an address
    #[error("Not subscribed to the notifications of script {0}")]
    NotSubscribed(Script),
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
    SendMsg(JsonRpcMsg),
    /// Send batch request
    SendBatch(Vec<JsonRpcMsg>),
    /// Close
    Close,
    /// Stop
    Stop,
    /// Completely disconnect
    Terminate,
}

/// Client notification
#[derive(Debug, Clone)]
pub enum ClientNotification {
    Response {
        id: usize,
        response: Response,
    },
    Notification(Notification),
    /// Stop
    Stop,
    /// Shutdown
    Shutdown,
}

#[derive(Debug, Clone, Default)]
struct ActiveSubscriptions {
    headers: Arc<AtomicBool>,
    scripts: Arc<Mutex<HashSet<Script>>>,
}

impl ActiveSubscriptions {
    fn headers(&self) -> bool {
        self.headers.load(Ordering::SeqCst)
    }

    fn set_headers(&self, enabled: bool) {
        let _ = self
            .headers
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |_| Some(enabled));
    }

    async fn scripts(&self) -> Vec<Script> {
        let scripts = self.scripts.lock().await;
        scripts.iter().cloned().collect()
    }
}

#[derive(Debug, Clone, Default)]
struct Schedule {
    scheduled_for_stop: Arc<AtomicBool>,
    scheduled_for_termination: Arc<AtomicBool>,
}

impl Schedule {
    fn is_scheduled_for_stop(&self) -> bool {
        self.scheduled_for_stop.load(Ordering::SeqCst)
    }

    fn schedule_for_stop(&self, value: bool) {
        let _ = self
            .scheduled_for_stop
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |_| Some(value));
    }

    fn is_scheduled_for_termination(&self) -> bool {
        self.scheduled_for_termination.load(Ordering::SeqCst)
    }

    fn schedule_for_termination(&self, value: bool) {
        let _ =
            self.scheduled_for_termination
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |_| Some(value));
    }
}

/// Electrum Client
#[derive(Debug, Clone)]
pub struct Client {
    addr: String,
    proxy: Option<SocketAddr>,
    status: Arc<Mutex<Status>>,
    last_id: Arc<AtomicUsize>,
    inventory: Arc<Mutex<HashMap<usize, Request>>>,
    subscriptions: ActiveSubscriptions,
    schedule: Schedule,
    sender: Sender<Message>,
    receiver: Arc<Mutex<Receiver<Message>>>,
    notifications: broadcast::Sender<ClientNotification>,
}

impl Client {
    /// New Client
    pub fn new<S>(addr: S, proxy: Option<SocketAddr>) -> Self
    where
        S: Into<String>,
    {
        let (sender, receiver) = mpsc::channel::<Message>(1024);
        let (notifications, _) = broadcast::channel::<ClientNotification>(1024);

        let this = Self {
            addr: addr.into(),
            proxy,
            status: Arc::new(Mutex::new(Status::default())),
            last_id: Arc::new(AtomicUsize::new(0)),
            inventory: Arc::new(Mutex::new(HashMap::new())),
            subscriptions: ActiveSubscriptions::default(),
            schedule: Schedule::default(),
            sender,
            receiver: Arc::new(Mutex::new(receiver)),
            notifications,
        };

        // Needed to keep opened the notifications channel
        let client = this.clone();
        thread::spawn(async move {
            let mut notifications = client.notifications();
            while notifications.recv().await.is_ok() {}
        });

        this
    }
    pub fn addr(&self) -> String {
        self.addr.clone()
    }

    pub fn proxy(&self) -> Option<SocketAddr> {
        self.proxy
    }

    pub fn notifications(&self) -> broadcast::Receiver<ClientNotification> {
        self.notifications.subscribe()
    }

    pub async fn status(&self) -> Status {
        let status = self.status.lock().await;
        *status
    }

    async fn set_status(&self, status: Status) {
        let mut s = self.status.lock().await;
        *s = status;
    }

    pub async fn is_connected(&self) -> bool {
        self.status().await == Status::Connected
    }

    pub async fn start(&self) {
        self.connect(false).await;
    }

    /// Connect to electrum server and keep alive connection
    pub async fn connect(&self, wait_for_connection: bool) {
        self.schedule.schedule_for_stop(false);
        self.schedule.schedule_for_termination(false);

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

                    // Schedule client for termination
                    // Needed to terminate the auto reconnect loop, also if the client is not connected yet.
                    if client.schedule.is_scheduled_for_termination() {
                        client.set_status(Status::Terminated).await;
                        client.schedule.schedule_for_termination(false);
                        log::debug!(
                            "Auto connect loop terminated for {} [schedule]",
                            client.addr
                        );
                        break;
                    }

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
                let pinger = thread::abortable(async move {
                    log::debug!("Client Pinger Thread Started");

                    loop {
                        thread::sleep(Duration::from_secs(45)).await;

                        if let Err(e) = client.ping(Some(Duration::from_secs(10))).await {
                            log::error!("Impossible to ping: {e}");
                        }
                    }
                });

                let client = self.clone();
                thread::spawn(async move {
                    log::debug!("Client Sender Thread Started");
                    let mut rx = client.receiver.lock().await;
                    while let Some((event, oneshot_sender)) = rx.recv().await {
                        match event {
                            ClientEvent::SendMsg(msg) => {
                                if msg.is_request() {
                                    let json = msg.as_json();
                                    log::debug!("Sending message {json}");
                                    match msg.as_bytes() {
                                        Ok(bytes) => match write.write_all(&bytes).await {
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
                                        },
                                        Err(e) => {
                                            log::error!("Impossible to convert msg to bytes: {e}")
                                        }
                                    }
                                }
                            }
                            ClientEvent::SendBatch(batch) => {
                                log::debug!(
                                    "Sending batch: {:?}",
                                    batch.iter().map(|m| m.as_json()).collect::<Vec<String>>()
                                );
                                let mut bytes: Vec<u8> = Vec::new();
                                for msg in batch.into_iter().filter(|m| m.is_request()) {
                                    match msg.as_bytes() {
                                        Ok(mut b) => bytes.append(&mut b),
                                        Err(e) => {
                                            log::error!("Impossible to convert msg to bytes: {e}")
                                        }
                                    }
                                }

                                match write.write_all(&bytes).await {
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
                                if client.schedule.is_scheduled_for_stop() {
                                    let _ = write.shutdown().await;
                                    client.set_status(Status::Stopped).await;
                                    client.schedule.schedule_for_stop(false);
                                    log::info!("Stopped {}", addr);
                                    break;
                                }
                            }
                            ClientEvent::Terminate => {
                                if client.schedule.is_scheduled_for_termination() {
                                    let _ = write.shutdown().await;
                                    client.set_status(Status::Terminated).await;
                                    client.schedule.schedule_for_termination(false);
                                    log::info!("Completely disconnected from {}", addr);
                                    break;
                                }
                            }
                        }
                    }

                    pinger.abort();

                    log::debug!("Exited from Client Event Thread");
                });

                let client = self.clone();
                thread::spawn(async move {
                    log::debug!("Client Receiver Thread Started");

                    let mut reader = BufReader::new(read);
                    let mut data = String::new();
                    loop {
                        data.clear();

                        match reader.read_line(&mut data).await {
                            Ok(size) => {
                                if size > 0 {
                                    match JsonRpcMsg::from_json(&data) {
                                        Ok(msg) => {
                                            log::trace!("Received response: {data}");

                                            match msg {
                                                JsonRpcMsg::Response { id, .. } => {
                                                    let mut inventory =
                                                        client.inventory.lock().await;
                                                    match inventory.get(&id) {
                                                        Some(req) => {
                                                            match msg.to_response(req) {
                                                                Ok(response) => {
                                                                    log::debug!("Sending response client notification: {response:?}");
                                                                    if response != Response::Null {
                                                                        if let Err(e) = client
                                                                .notifications
                                                                .send(ClientNotification::Response {
                                                                    id,
                                                                    response,
                                                                }) {
                                                                    log::error!("Impossible to send client notification: {e}")
                                                                }
                                                                    }
                                                                },
                                                                Err(e) => log::error!("Impossible to handle JSONRPC response: {e}")
                                                            }
                                                        },
                                                        None => {
                                                            log::error!("ID not found in inventory")
                                                        }
                                                    }

                                                    inventory.remove(&id);
                                                }
                                                JsonRpcMsg::Notification { .. } => {
                                                    match msg.to_notification() {
                                                        Ok(notification) => {
                                                            log::debug!("Sending client notification: {notification:?}");
                                                            if let Err(e) = client
                                                        .notifications
                                                        .send(ClientNotification::Notification(notification)) {
                                                            log::error!("Impossible to send client notification: {e}")
                                                        }
                                                        },
                                                        Err(e) => log::error!("Impossible to handle JSONRPC notification: {e}")
                                                    }
                                                }
                                                _ => log::warn!("Unexpected msg: {data}"),
                                            }
                                        }
                                        Err(e) => {
                                            log::error!("{e}: {data}");
                                        }
                                    };
                                } else {
                                    log::error!("Stream has reached EOF");
                                    break;
                                }
                            }
                            Err(e) => {
                                log::error!("Impossible to read line: {e}");
                                break;
                            }
                        }
                    }

                    log::debug!("Exited from Message Thread of {}", client.addr);

                    if let Err(err) = client.disconnect().await {
                        log::error!("Impossible to disconnect {}: {}", client.addr, err);
                    }
                });

                if self.subscriptions.headers() {
                    if let Err(e) = self._block_headers_subscribe(None).await {
                        log::error!("Impossible to subscribe to headers: {e}");
                    }
                }

                let scripts = self.subscriptions.scripts().await;
                if let Err(e) = self._batch_script_subscribe(scripts, None).await {
                    log::error!("Impossible to subscribe to scripts: {e}");
                }
            }
            Err(err) => {
                self.set_status(Status::Disconnected).await;
                log::error!("Impossible to connect to {}: {}", addr, err);
            }
        };
    }

    fn send_client_event(
        &self,
        client_msg: ClientEvent,
        sender: Option<oneshot::Sender<bool>>,
    ) -> Result<(), Error> {
        self.sender
            .try_send((client_msg, sender))
            .map_err(|_| Error::MessageNotSent)
    }

    /// Disconnect and set status to 'Disconnected'
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

    /// Disconnect and set status to 'Stopped'
    pub async fn stop(&self) -> Result<(), Error> {
        self.schedule.schedule_for_stop(true);
        let status = self.status().await;
        if status.ne(&Status::Disconnected)
            && status.ne(&Status::Stopped)
            && status.ne(&Status::Terminated)
        {
            self.send_client_event(ClientEvent::Stop, None)?;
        }
        Ok(())
    }

    /// Disconnect and set status to 'Terminated'
    pub async fn shutdown(self) -> Result<(), Error> {
        self.schedule.schedule_for_termination(true);
        let status = self.status().await;
        if status.ne(&Status::Disconnected)
            && status.ne(&Status::Stopped)
            && status.ne(&Status::Terminated)
        {
            self.send_client_event(ClientEvent::Terminate, None)?;
        }
        Ok(())
    }

    async fn send_msg(&self, req: Request, wait: Option<Duration>) -> Result<usize, Error> {
        let next_id = self.last_id.fetch_add(1, Ordering::SeqCst);
        let msg = JsonRpcMsg::request(next_id, req.clone());

        {
            let mut inventory = self.inventory.lock().await;
            inventory.insert(next_id, req);
        }

        match wait {
            Some(timeout) => {
                let (tx, rx) = oneshot::channel::<bool>();
                self.send_client_event(ClientEvent::SendMsg(msg), Some(tx))?;
                match time::timeout(Some(timeout), rx).await {
                    Some(result) => match result {
                        Ok(val) => {
                            if val {
                                Ok(next_id)
                            } else {
                                Err(Error::MessageNotSent)
                            }
                        }
                        Err(_) => Err(Error::OneShotRecvError),
                    },
                    _ => Err(Error::RecvTimeout),
                }
            }
            None => {
                self.send_client_event(ClientEvent::SendMsg(msg), None)?;
                Ok(next_id)
            }
        }
    }

    async fn send_batch_msg(
        &self,
        reqs: Vec<Request>,
        wait: Option<Duration>,
    ) -> Result<(), Error> {
        let mut msgs = Vec::new();

        {
            let mut inventory = self.inventory.lock().await;
            for req in reqs.into_iter() {
                let next_id = self.last_id.fetch_add(1, Ordering::SeqCst);
                let msg = JsonRpcMsg::request(next_id, req.clone());
                msgs.push(msg);
                inventory.insert(next_id, req);
            }
        }

        match wait {
            Some(timeout) => {
                let (tx, rx) = oneshot::channel::<bool>();
                self.send_client_event(ClientEvent::SendBatch(msgs), Some(tx))?;
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
            None => self.send_client_event(ClientEvent::SendBatch(msgs), None),
        }
    }

    async fn get_response(
        &self,
        msg_id: usize,
        timeout: Option<Duration>,
    ) -> Result<Option<Response>, Error> {
        let mut notifications = self.notifications.subscribe();
        time::timeout(timeout, async {
            while let Ok(notification) = notifications.recv().await {
                if let ClientNotification::Response { id, response } = notification {
                    if msg_id == id {
                        return Some(response);
                    }
                }
            }

            None
        })
        .await
        .ok_or(Error::Timeout)
    }

    async fn call(
        &self,
        req: Request,
        timeout: Option<Duration>,
    ) -> Result<Option<Response>, Error> {
        let id = self.send_msg(req, timeout).await?;
        self.get_response(id, timeout).await
    }
}

impl Client {
    pub async fn ping(&self, timeout: Option<Duration>) -> Result<(), Error> {
        let req = Request::Ping;
        self.send_msg(req, timeout).await?;
        Ok(())
    }

    pub async fn block_header(
        &self,
        height: usize,
        timeout: Option<Duration>,
    ) -> Result<BlockHeader, Error> {
        let req = Request::GetBlockHeader { height };
        match self.call(req, timeout).await? {
            Some(Response::BlockHeader(header)) => Ok(header),
            _ => Err(Error::InvalidResponse),
        }
    }

    pub async fn block_headers(
        &self,
        start_height: usize,
        count: usize,
        timeout: Option<Duration>,
    ) -> Result<GetHeadersRes, Error> {
        let req = Request::GetBlockHeaders {
            start_height,
            count,
        };
        match self.call(req, timeout).await? {
            Some(Response::BlockHeaders(headers)) => Ok(headers),
            _ => Err(Error::InvalidResponse),
        }
    }

    async fn _block_headers_subscribe(&self, timeout: Option<Duration>) -> Result<(), Error> {
        let req = Request::BlockHeaderSubscribe;
        self.send_msg(req, timeout).await?;
        Ok(())
    }

    pub async fn block_headers_subscribe(&self, timeout: Option<Duration>) -> Result<(), Error> {
        self.subscriptions.set_headers(true);
        self._block_headers_subscribe(timeout).await
    }

    async fn _script_subscribe(
        &self,
        script: Script,
        timeout: Option<Duration>,
    ) -> Result<Option<ScriptStatus>, Error> {
        let req = Request::ScriptSubscribe(script);
        match self.call(req, timeout).await? {
            Some(Response::ScriptStatus(status)) => Ok(status),
            Some(Response::Null) => Ok(None),
            _ => Err(Error::InvalidResponse),
        }
    }

    pub async fn script_subscribe(
        &self,
        script: Script,
        timeout: Option<Duration>,
    ) -> Result<Option<ScriptStatus>, Error> {
        let mut scripts = self.subscriptions.scripts.lock().await;
        if scripts.contains(&script) {
            return Err(Error::AlreadySubscribed(script));
        }
        scripts.insert(script.clone());
        drop(scripts);
        self._script_subscribe(script, timeout).await
    }

    pub async fn script_unsubscribe(
        &self,
        script: Script,
        timeout: Option<Duration>,
    ) -> Result<bool, Error> {
        let mut scripts = self.subscriptions.scripts.lock().await;
        if !scripts.contains(&script) {
            return Err(Error::NotSubscribed(script));
        }
        scripts.remove(&script);
        drop(scripts);

        let req = Request::ScriptUnsubscribe(script);
        match self.call(req, timeout).await? {
            Some(Response::ScriptUnsubscribe(status)) => Ok(status),
            _ => Err(Error::InvalidResponse),
        }
    }

    async fn _batch_script_subscribe(
        &self,
        script: Vec<Script>,
        timeout: Option<Duration>,
    ) -> Result<(), Error> {
        let reqs = script.into_iter().map(Request::ScriptSubscribe).collect();
        self.send_batch_msg(reqs, timeout).await?;
        Ok(())
    }

    pub async fn batch_script_subscribe(
        &self,
        scripts: Vec<Script>,
        timeout: Option<Duration>,
    ) -> Result<(), Error> {
        let mut _scripts = self.subscriptions.scripts.lock().await;
        for script in scripts.iter() {
            if _scripts.contains(script) {
                return Err(Error::AlreadySubscribed(script.clone()));
            }
            _scripts.insert(script.clone());
        }
        drop(_scripts);
        self._batch_script_subscribe(scripts, timeout).await
    }

    /// Estimates the fee required in Bitcoin per kilobyte to confirm a transaction in number blocks.
    pub async fn estimate_fee(&self, blocks: u8, timeout: Option<Duration>) -> Result<f64, Error> {
        let req = Request::EstimateFee { blocks };
        match self.call(req, timeout).await? {
            Some(Response::EstimateFee(fee)) => Ok(fee),
            _ => Err(Error::InvalidResponse),
        }
    }

    pub async fn broadcast_tx(
        &self,
        tx: Transaction,
        timeout: Option<Duration>,
    ) -> Result<Txid, Error> {
        let req = Request::BroadcastTx(tx);
        match self.call(req, timeout).await? {
            Some(Response::BroadcastTx(txid)) => Ok(txid),
            _ => Err(Error::InvalidResponse),
        }
    }

    pub async fn get_transaction(
        &self,
        txid: Txid,
        timeout: Option<Duration>,
    ) -> Result<Transaction, Error> {
        let req = Request::GetTransaction(txid);
        match self.call(req, timeout).await? {
            Some(Response::Transaction(tx)) => Ok(tx),
            _ => Err(Error::InvalidResponse),
        }
    }

    pub async fn get_balance(
        &self,
        script: Script,
        timeout: Option<Duration>,
    ) -> Result<GetBalanceRes, Error> {
        let req = Request::GetBalance(script);
        match self.call(req, timeout).await? {
            Some(Response::Balance(balance)) => Ok(balance),
            _ => Err(Error::InvalidResponse),
        }
    }
}
