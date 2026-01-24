//! YARD Core - Domain logic and state machines.
//!
//! This crate contains the core domain logic for YARD without any I/O operations.
//! It provides typed errors, state machines, and common types used across all crates.

pub mod error;
pub mod prelude;

pub use error::{Error, Result};
