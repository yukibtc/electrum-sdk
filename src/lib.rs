// Copyright (c) 2023 Yuki Kishimoto
// Distributed under the MIT software license

// #![warn(missing_docs)]

pub extern crate bitcoin;

mod api;
mod batch;
mod client;
mod error;
mod net;
mod types;

pub use self::api::ElectrumApi;
pub use self::batch::Batch;
pub use self::client::Client;
pub use self::error::Error;
pub use self::types::*;
