// Copyright (c) 2023 Yuki Kishimoto
// Distributed under the MIT software license

use bitcoin::ScriptHash;
use thiserror::Error;

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
