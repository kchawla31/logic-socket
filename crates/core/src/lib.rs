//! Core data model and storage for logic-socket.

pub mod model;
pub mod store;

pub use model::*;
pub use store::{Change, RawDoc, Store, StoreError, Tx, new_id, now_ms};
