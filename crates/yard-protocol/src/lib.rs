//! YARD Protocol - RDP protocol handling via IronRDP.
//!
//! This crate provides the RDP protocol implementation using the IronRDP library.
//! It handles connection establishment, authentication, and channel management.

pub mod audin;
pub mod cliprdr;
pub mod connection;
pub mod messages;
pub mod rdpsnd;

pub use audin::{YardAudinHandler, create_audin_client};
pub use cliprdr::{YardCliprdrHandler, create_cliprdr_client};
pub use connection::spawn_network_thread;
pub use messages::{
    CertificateInfo, ConnectionConfig, ConnectionError, DecodedFrame, DesktopSize, FromNetwork,
    MouseButton, RdpMonitorInfo, RdpMonitorLayout, ToNetwork, VideoCodec, validate_monitor_layout,
};
pub use yard_core::prelude::*;
