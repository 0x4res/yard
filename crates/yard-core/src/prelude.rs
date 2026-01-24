//! Common types re-exported for use across YARD crates.
//!
//! Usage: `use yard_core::prelude::*;`

pub use crate::error::{Error, Result};

// Re-export commonly used external types
pub use thiserror::Error;
