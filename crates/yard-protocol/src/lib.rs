//! YARD Protocol - RDP protocol handling via IronRDP.
//!
//! This crate provides the RDP protocol implementation using the IronRDP library.
//! It handles connection establishment, authentication, and channel management.

pub mod connection;
pub mod messages;

pub use connection::spawn_network_thread;
pub use messages::{ConnectionConfig, ConnectionError, FromNetwork, ToNetwork};
pub use yard_core::prelude::*;
