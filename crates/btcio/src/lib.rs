//! Input-output with Bitcoin, implementing L1 chain trait.

pub mod btcio_status;
pub mod reader;
pub mod rpc;

use std::sync::RwLock;
use crate::btcio_status::BtcioStatus;

