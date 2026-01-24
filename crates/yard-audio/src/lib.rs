//! YARD Audio - PipeWire integration for audio I/O.
//!
//! This crate provides the audio pipeline using PipeWire.
//! It handles audio playback and microphone input for RDP sessions.
//!
//! **Note:** This crate requires Linux with PipeWire support.

#![cfg_attr(not(target_os = "linux"), allow(unused_imports))]

pub use yard_core::prelude::*;
