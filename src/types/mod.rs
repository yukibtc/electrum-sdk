// Copyright (c) 2023 Yuki Kishimoto
// Distributed under the MIT software license

use std::convert::TryFrom;
use std::ops::Deref;

use bitcoin::blockdata::block;
use bitcoin::consensus::encode::{deserialize, serialize};
use bitcoin::hashes::hex::{FromHex, ToHex};
use bitcoin::hashes::{sha256, Hash};
use bitcoin::{BlockHeader, Script, Transaction, Txid};
use serde::{de, Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

mod method;

pub use self::method::Method;

static JSONRPC_2_0: &str = "2.0";

/// Errors
#[derive(Debug, Error)]
pub enum Error {
    /// Wraps `std::io::Error`
    #[error(transparent)]
    IO(#[from] std::io::Error),
    /// Wraps `serde_json::error::Error`
    #[error(transparent)]
    JSON(#[from] serde_json::error::Error),
    /// Wraps `bitcoin::hashes::hex::Error`
    #[error(transparent)]
    Hex(#[from] bitcoin::hashes::hex::Error),
    /// Error during the deserialization of a Bitcoin data structure
    #[error(transparent)]
    Bitcoin(#[from] bitcoin::consensus::encode::Error),
    /// Error returned by the Electrum server
    #[error("Electrum server error: {0}")]
    Protocol(serde_json::Value),
    /// Already subscribed to the notifications of an address
    #[error("Already subscribed to the notifications of an address")]
    AlreadySubscribed(ScriptHash),
    /// Not subscribed to the notifications of an address
    #[error("Not subscribed to the notifications of an address")]
    NotSubscribed(ScriptHash),
    /// Error during the deserialization of a response from the server
    #[error("Error during the deserialization of a response from the server: {0}")]
    InvalidResponse(serde_json::Value),
    /// Unknown method
    #[error("unknown method: {0}")]
    UnknownMethod(String),
    /// Unexpected msg
    #[error("unexpected method: {0}")]
    UnexpectedMethod(Method),
    /// Unexpected msg
    #[error("unexpected msg")]
    UnexpectedMsg,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
/// A single parameter of a [`Request`](struct.Request.html)
pub enum Param {
    /// Integer parameter
    U8(u8),
    /// Integer parameter
    U32(u32),
    /// Integer parameter
    Usize(usize),
    /// String parameter
    String(String),
    /// Boolean parameter
    Bool(bool),
    /// Bytes array parameter
    Bytes(Vec<u8>),
}

/// Request
#[derive(Debug, Clone)]
pub enum Request {
    GetBlockHeader { height: usize },
    GetBlockHeaders { start_height: usize, count: usize },
    BlockHeaderSubscribe,
    ScriptSubscribe(Script),
    ScriptUnsubscribe(Script),
    EstimateFee { blocks: u8 },
    BroadcastTx(Transaction),
    GetTransaction(Txid),
    Version { name: String, version: f32 },
    Ping,
}

impl Request {
    /// Get req method
    pub fn method(&self) -> Method {
        match self {
            Self::GetBlockHeader { .. } => Method::GetBlockHeader,
            Self::GetBlockHeaders { .. } => Method::GetBlockHeaders,
            Self::BlockHeaderSubscribe => Method::BlockHeaderSubscribe,
            Self::ScriptSubscribe(..) => Method::ScriptSubscribe,
            Self::ScriptUnsubscribe(..) => Method::ScriptUnsubscribe,
            Self::EstimateFee { .. } => Method::EstimateFee,
            Self::BroadcastTx(..) => Method::BroadcastTx,
            Self::GetTransaction(..) => Method::GetTransaction,
            Self::Version { .. } => Method::Version,
            Self::Ping => Method::Ping,
        }
    }

    /// Get req params
    pub fn params(&self) -> Vec<Param> {
        match self {
            Self::GetBlockHeader { height } => vec![Param::Usize(*height)],
            Self::GetBlockHeaders {
                start_height,
                count,
            } => vec![Param::Usize(*start_height), Param::Usize(*count)],
            Self::BlockHeaderSubscribe => Vec::new(),
            Self::ScriptSubscribe(script) => {
                let script_hash = script.to_electrum_scripthash();
                vec![Param::String(script_hash.to_hex())]
            }
            Self::ScriptUnsubscribe(script) => {
                let script_hash = script.to_electrum_scripthash();
                vec![Param::String(script_hash.to_hex())]
            }
            Self::EstimateFee { blocks } => vec![Param::U8(*blocks)],
            Self::BroadcastTx(tx) => {
                let buffer: Vec<u8> = serialize(tx);
                vec![Param::Bytes(buffer)]
            }
            Self::GetTransaction(txid) => vec![Param::String(txid.to_string())],
            Self::Version { name, version } => vec![
                Param::String(name.clone()),
                Param::String(version.to_string()),
            ],
            Self::Ping => Vec::new(),
        }
    }
}

/// Response
#[derive(Debug, Clone, PartialEq)]
pub enum Response {
    BlockHeader(BlockHeader),
    BlockHeaders(GetHeadersRes),
    HeaderNotification(HeaderNotification),
    ScriptStatus(Option<ScriptStatus>),
    ScriptUnsubscribe(bool),
    EstimateFee(f64),
    BroadcastTx(Txid),
    Transaction(Transaction),
    Pong,
    Null,
}

/// Notification
#[derive(Debug, Clone)]
pub enum Notification {
    Headers(Vec<HeaderNotification>),
    Script(ScriptNotification),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum JsonRpcMsg {
    /// Request
    Request {
        /// Request id
        id: usize,
        /// JSON-RPC version
        jsonrpc: String,
        /// Method
        method: Method,
        /// params
        params: Vec<Param>,
    },
    /// Response
    Response {
        /// Request id
        id: usize,
        /// JSON-RPC version
        jsonrpc: String,
        /// Result
        result: Option<Value>,
        /// Reason, if failed
        error: Option<Value>,
    },
    /// Notification
    Notification {
        /// JSON-RPC version
        jsonrpc: String,
        /// Method
        method: Method,
        /// Params
        params: Value,
    },
}

impl JsonRpcMsg {
    /// Compose `Request` message
    pub fn request(id: usize, req: Request) -> Self {
        Self::Request {
            id,
            jsonrpc: JSONRPC_2_0.to_string(),
            method: req.method(),
            params: req.params(),
        }
    }

    /// Deserialize from JSON string
    pub fn from_json<S>(json: S) -> Result<Self, Error>
    where
        S: Into<String>,
    {
        Ok(serde_json::from_str(&json.into())?)
    }

    /// Serialize as JSON
    pub fn as_json(&self) -> String {
        serde_json::json!(self).to_string()
    }

    /// Serialize as bytes
    pub fn as_bytes(&self) -> Result<Vec<u8>, Error> {
        let mut bytes = serde_json::to_vec(self)?;
        bytes.extend_from_slice(b"\n");
        Ok(bytes)
    }

    pub fn id(&self) -> Option<usize> {
        match self {
            Self::Request { id, .. } => Some(*id),
            Self::Response { id, .. } => Some(*id),
            Self::Notification { .. } => None,
        }
    }

    pub fn is_request(&self) -> bool {
        matches!(self, Self::Request { .. })
    }

    pub fn is_response(&self) -> bool {
        matches!(self, Self::Response { .. })
    }

    pub fn is_notification(&self) -> bool {
        matches!(self, Self::Notification { .. })
    }

    pub fn to_response(&self, req: &Request) -> Result<Response, Error> {
        if let Self::Response { result, error, .. } = self {
            if let Some(result) = result.clone() {
                match req {
                    Request::GetBlockHeader { .. } => {
                        let data: String = serde_json::from_value(result)?;
                        let data: Vec<u8> = Vec::<u8>::from_hex(&data)?;
                        let header: BlockHeader = deserialize(&data)?;
                        Ok(Response::BlockHeader(header))
                    }
                    Request::GetBlockHeaders { .. } => {
                        let mut deserialized: GetHeadersRes = serde_json::from_value(result)?;
                        for i in 0..deserialized.count {
                            let (start, end) = (i * 80, (i + 1) * 80);
                            deserialized
                                .headers
                                .push(deserialize(&deserialized.raw_headers[start..end])?);
                        }
                        deserialized.raw_headers.clear();

                        Ok(Response::BlockHeaders(deserialized))
                    }
                    Request::BlockHeaderSubscribe => {
                        let notification: RawHeaderNotification = serde_json::from_value(result)?;
                        let notification = HeaderNotification::try_from(notification)?;
                        Ok(Response::HeaderNotification(notification))
                    }
                    Request::ScriptSubscribe(..) => {
                        let status: Option<ScriptStatus> = serde_json::from_value(result)?;
                        Ok(Response::ScriptStatus(status))
                    }
                    Request::ScriptUnsubscribe(..) => {
                        let status: bool = serde_json::from_value(result)?;
                        Ok(Response::ScriptUnsubscribe(status))
                    }
                    Request::EstimateFee { .. } => {
                        let fee: f64 = serde_json::from_value(result)?;
                        Ok(Response::EstimateFee(fee))
                    }
                    Request::BroadcastTx(..) => {
                        let txid: Txid = serde_json::from_value(result)?;
                        Ok(Response::BroadcastTx(txid))
                    }
                    Request::GetTransaction(..) => {
                        let data: String = serde_json::from_value(result)?;
                        let data: Vec<u8> = Vec::<u8>::from_hex(&data)?;
                        let tx: Transaction = deserialize(&data)?;
                        Ok(Response::Transaction(tx))
                    }
                    Request::Ping => Ok(Response::Pong),
                    Request::Version { .. } => Ok(Response::Null),
                }
            } else if let Some(e) = error {
                Err(Error::Protocol(e.clone()))
            } else {
                Ok(Response::Null)
            }
        } else {
            Err(Error::UnexpectedMsg)
        }
    }

    pub fn to_notification(&self) -> Result<Notification, Error> {
        if let Self::Notification { method, params, .. } = self {
            match method {
                Method::BlockHeaderSubscribe => {
                    let raw: Vec<RawHeaderNotification> = serde_json::from_value(params.clone())?;
                    let mut notifications = Vec::new();
                    for n in raw.into_iter() {
                        notifications.push(HeaderNotification::try_from(n)?);
                    }
                    Ok(Notification::Headers(notifications))
                }
                Method::ScriptSubscribe => {
                    let notification: ScriptNotification = serde_json::from_value(params.clone())?;
                    Ok(Notification::Script(notification))
                }
                m => Err(Error::UnexpectedMethod(*m)),
            }
        } else {
            Err(Error::UnexpectedMsg)
        }
    }
}

#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
pub struct Hex32Bytes(#[serde(deserialize_with = "from_hex")] [u8; 32]);

impl Deref for Hex32Bytes {
    type Target = [u8; 32];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<[u8; 32]> for Hex32Bytes {
    fn from(other: [u8; 32]) -> Hex32Bytes {
        Hex32Bytes(other)
    }
}

/// Format used by the Electrum server to identify an address. The reverse sha256 hash of the
/// scriptPubKey. Documented [here](https://electrumx.readthedocs.io/en/latest/protocol-basics.html#script-hashes).
pub type ScriptHash = Hex32Bytes;

/// Binary blob that condenses all the activity of an address. Used to detect changes without
/// having to compare potentially long lists of transactions.
pub type ScriptStatus = Hex32Bytes;

/// Trait used to convert a struct into the Electrum representation of an address
pub trait ToElectrumScriptHash {
    /// Transforms the current struct into a `ScriptHash`
    fn to_electrum_scripthash(&self) -> ScriptHash;
}

impl ToElectrumScriptHash for Script {
    fn to_electrum_scripthash(&self) -> ScriptHash {
        let mut result = sha256::Hash::hash(self.as_bytes()).into_inner();
        result.reverse();
        result.into()
    }
}

fn from_hex<'de, T, D>(deserializer: D) -> Result<T, D::Error>
where
    T: FromHex,
    D: de::Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    T::from_hex(&s).map_err(de::Error::custom)
}

fn from_hex_array<'de, T, D>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    T: FromHex + std::fmt::Debug,
    D: de::Deserializer<'de>,
{
    let arr = Vec::<String>::deserialize(deserializer)?;

    let results: Vec<Result<T, _>> = arr
        .into_iter()
        .map(|s| T::from_hex(&s).map_err(de::Error::custom))
        .collect();

    let mut answer = Vec::new();
    for x in results.into_iter() {
        answer.push(x?);
    }

    Ok(answer)
}

fn from_hex_header<'de, D>(deserializer: D) -> Result<block::BlockHeader, D::Error>
where
    D: de::Deserializer<'de>,
{
    let vec: Vec<u8> = from_hex(deserializer)?;
    deserialize(&vec).map_err(de::Error::custom)
}

/// Response to a [`script_get_history`](../client/struct.Client.html#method.script_get_history) request.
#[derive(Debug, Clone, Deserialize)]
pub struct GetHistoryRes {
    /// Confirmation height of the transaction. 0 if unconfirmed, -1 if unconfirmed while some of
    /// its inputs are unconfirmed too.
    pub height: i32,
    /// Txid of the transaction.
    pub tx_hash: Txid,
    /// Fee of the transaction.
    pub fee: Option<u64>,
}

/// Response to a [`script_list_unspent`](../client/struct.Client.html#method.script_list_unspent) request.
#[derive(Debug, Clone, Deserialize)]
pub struct ListUnspentRes {
    /// Confirmation height of the transaction that created this output.
    pub height: usize,
    /// Txid of the transaction
    pub tx_hash: Txid,
    /// Index of the output in the transaction.
    pub tx_pos: usize,
    /// Value of the output.
    pub value: u64,
}

/// Response to a [`server_features`](../client/struct.Client.html#method.server_features) request.
#[derive(Debug, Clone, Deserialize)]
pub struct ServerFeaturesRes {
    /// Server version reported.
    pub server_version: String,
    /// Hash of the genesis block.
    #[serde(deserialize_with = "from_hex")]
    pub genesis_hash: [u8; 32],
    /// Minimum supported version of the protocol.
    pub protocol_min: String,
    /// Maximum supported version of the protocol.
    pub protocol_max: String,
    /// Hash function used to create the [`ScriptHash`](type.ScriptHash.html).
    pub hash_function: Option<String>,
    /// Pruned height of the server.
    pub pruning: Option<i64>,
}

/// Response to a [`server_features`](../client/struct.Client.html#method.server_features) request.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct GetHeadersRes {
    /// Maximum number of headers returned in a single response.
    pub max: usize,
    /// Number of headers in this response.
    pub count: usize,
    /// Raw headers concatenated. Normally cleared before returning.
    #[serde(rename(deserialize = "hex"), deserialize_with = "from_hex")]
    pub raw_headers: Vec<u8>,
    /// Array of block headers.
    #[serde(skip)]
    pub headers: Vec<block::BlockHeader>,
}

/// Response to a [`script_get_balance`](../client/struct.Client.html#method.script_get_balance) request.
#[derive(Debug, Clone, Deserialize)]
pub struct GetBalanceRes {
    /// Confirmed balance in Satoshis for the address.
    pub confirmed: u64,
    /// Unconfirmed balance in Satoshis for the address.
    ///
    /// Some servers (e.g. `electrs`) return this as a negative value.
    pub unconfirmed: i64,
}

/// Response to a [`transaction_get_merkle`](../client/struct.Client.html#method.transaction_get_merkle) request.
#[derive(Debug, Clone, Deserialize)]
pub struct GetMerkleRes {
    /// Height of the block that confirmed the transaction
    pub block_height: usize,
    /// Position in the block of the transaction.
    pub pos: usize,
    /// The merkle path of the transaction.
    #[serde(deserialize_with = "from_hex_array")]
    pub merkle: Vec<[u8; 32]>,
}

/// Notification of a new block header
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct HeaderNotification {
    /// New block height.
    pub height: usize,
    /// Newly added header.
    #[serde(rename = "hex", deserialize_with = "from_hex_header")]
    pub header: block::BlockHeader,
}

/// Notification of a new block header with the header encoded as raw bytes
#[derive(Debug, Clone, Deserialize)]
pub struct RawHeaderNotification {
    /// New block height.
    pub height: usize,
    /// Newly added header.
    #[serde(rename = "hex", deserialize_with = "from_hex")]
    pub header: Vec<u8>,
}

impl TryFrom<RawHeaderNotification> for HeaderNotification {
    type Error = Error;

    fn try_from(raw: RawHeaderNotification) -> Result<Self, Self::Error> {
        Ok(HeaderNotification {
            height: raw.height,
            header: deserialize(&raw.header)?,
        })
    }
}

/// Notification of the new status of a script
#[derive(Debug, Clone, Deserialize)]
pub struct ScriptNotification {
    /// Address that generated this notification.
    pub scripthash: ScriptHash,
    /// The new status of the address.
    pub status: ScriptStatus,
}
