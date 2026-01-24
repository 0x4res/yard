//! Error types for YARD core operations.

use thiserror::Error;

/// Core error type for YARD operations.
#[derive(Error, Debug)]
pub enum Error {
    /// Configuration-related errors.
    #[error("Configuration error: {0}")]
    Config(String),

    /// Invalid state transition attempted.
    #[error("Invalid state transition: {from} -> {to}")]
    InvalidStateTransition {
        /// The state being transitioned from.
        from: String,
        /// The state being transitioned to.
        to: String,
    },
}

/// A specialized Result type for YARD core operations.
pub type Result<T> = std::result::Result<T, Error>;
