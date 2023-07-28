//! Return types
//!
//! This module contains definitions of all the complex data structures that are returned by calls

use std::convert::TryFrom;
use std::ops::Deref;

use bitcoin::blockdata::block;
use bitcoin::consensus::encode::deserialize;
use bitcoin::hashes::hex::FromHex;
use bitcoin::hashes::{sha256, Hash};
use bitcoin::{Script, Txid};
use serde::{de, Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

static JSONRPC_2_0: &str = "2.0";

//pub type Call = (String, Vec<Param>);

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
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Request {
    GetBlockHeader { height: usize },
    EstimateFee { blocks: u8 },
}

impl Request {
    /// Get req method
    pub fn method(&self) -> String {
        match self {
            Self::GetBlockHeader { .. } => "blockchain.block.header".to_string(),
            Self::EstimateFee { .. } => "blockchain.estimatefee".to_string(),
        }
    }

    /// Get req params
    pub fn params(&self) -> Vec<Param> {
        match self {
            Self::GetBlockHeader { height } => vec![Param::Usize(*height)],
            Self::EstimateFee { blocks } => vec![Param::U8(*blocks)],
        }
    }
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
        method: String,
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
        error: Option<String>,
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

    pub fn id(&self) -> usize {
        match self {
            Self::Request { id, .. } => *id,
            Self::Response { id, .. } => *id,
        }
    }

    pub fn is_request(&self) -> bool {
        matches!(self, Self::Request { .. })
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
#[derive(Debug, Deserialize)]
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
#[derive(Debug, Deserialize)]
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
#[derive(Debug, Deserialize)]
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
#[derive(Debug, Deserialize)]
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
#[derive(Debug, Deserialize)]
pub struct GetBalanceRes {
    /// Confirmed balance in Satoshis for the address.
    pub confirmed: u64,
    /// Unconfirmed balance in Satoshis for the address.
    ///
    /// Some servers (e.g. `electrs`) return this as a negative value.
    pub unconfirmed: i64,
}

/// Response to a [`transaction_get_merkle`](../client/struct.Client.html#method.transaction_get_merkle) request.
#[derive(Debug, Deserialize)]
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
#[derive(Debug, Deserialize)]
pub struct HeaderNotification {
    /// New block height.
    pub height: usize,
    /// Newly added header.
    #[serde(rename = "hex", deserialize_with = "from_hex_header")]
    pub header: block::BlockHeader,
}

/// Notification of a new block header with the header encoded as raw bytes
#[derive(Debug, Deserialize)]
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
#[derive(Debug, Deserialize)]
pub struct ScriptNotification {
    /// Address that generated this notification.
    pub scripthash: ScriptHash,
    /// The new status of the address.
    pub status: ScriptStatus,
}
