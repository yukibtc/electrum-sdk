// Copyright (c) 2023 Yuki Kishimoto
// Distributed under the MIT software license

use std::fmt;
use std::str::FromStr;

use serde::de::Error as DeserializerError;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use thiserror::Error;

/// Errors
#[derive(Debug, Error)]
pub enum Error {
    /// Unknown method
    #[error("unknown method: {0}")]
    UnknownMethod(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Method {
    GetBlockHeader,
    GetBlockHeaders,
    BlockHeaderSubscribe,
    EstimateFee,
    GetTransaction,
    Version,
    Ping,
}

impl fmt::Display for Method {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::GetBlockHeader => write!(f, "blockchain.block.header"),
            Self::GetBlockHeaders => write!(f, "blockchain.block.headers"),
            Self::BlockHeaderSubscribe => write!(f, "blockchain.headers.subscribe"),
            Self::EstimateFee => write!(f, "blockchain.estimatefee"),
            Self::GetTransaction => write!(f, "blockchain.transaction.get"),
            Self::Version => write!(f, "server.version"),
            Self::Ping => write!(f, "server.ping"),
        }
    }
}

impl FromStr for Method {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "blockchain.block.header" => Ok(Self::GetBlockHeader),
            "blockchain.block.headers" => Ok(Self::GetBlockHeaders),
            "blockchain.headers.subscribe" => Ok(Self::BlockHeaderSubscribe),
            "blockchain.estimatefee" => Ok(Self::EstimateFee),
            "blockchain.transaction.get" => Ok(Self::GetTransaction),
            "server.version" => Ok(Self::Version),
            "server.ping" => Ok(Self::Ping),
            m => Err(Error::UnknownMethod(m.to_string())),
        }
    }
}

impl Serialize for Method {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Method {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let json_value = Value::deserialize(deserializer)?;
        let conditions: String =
            serde_json::from_value(json_value).map_err(DeserializerError::custom)?;
        Self::from_str(&conditions).map_err(DeserializerError::custom)
    }
}
