//! YARD Core - Domain logic and state machines.
//!
//! This crate contains the core domain logic for YARD without any I/O operations.
//! It provides typed errors, state machines, and common types used across all crates.

pub mod config;
pub mod error;
pub mod prelude;

pub use config::{Config, ConnectionDefaults, DEFAULT_PORT};
pub use error::{Error, Result};
