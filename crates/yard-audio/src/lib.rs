//! YARD Audio - PipeWire integration for audio I/O.
//!
//! This crate provides the audio pipeline using PipeWire.
//! It handles audio playback and microphone input for RDP sessions.
//!
//! # Architecture
//!
//! The audio subsystem runs in a dedicated thread, isolated from the main
//! calloop event loop. Communication happens via the [`ToAudio`] message enum.
//!
//! ```text
//! ┌─────────────────┐     ┌─────────────────┐
//! │   Main Thread   │     │  Audio Thread   │
//! │   (calloop)     │────▶│  (PipeWire)     │
//! └─────────────────┘     └─────────────────┘
//!         │                       ▲
//!         │      ToAudio          │
//!         └───────────────────────┘
//! ```
//!
//! # Usage
//!
//! ```ignore
//! use yard_audio::{AudioThread, ToAudio};
//!
//! // Spawn the audio thread
//! let audio = AudioThread::spawn()?;
//!
//! // Send commands via the sender
//! let sender = audio.sender();
//! sender.send(ToAudio::SetVolume(0.8))?;
//!
//! // Thread shuts down automatically when dropped
//! ```
//!
//! # Platform Support
//!
//! - **Linux**: Full PipeWire integration
//! - **Other platforms**: Stub implementation (no audio)
//!
//! **Note:** This crate requires Linux with PipeWire support for full functionality.

#![cfg_attr(not(target_os = "linux"), allow(unused_imports))]

mod error;
mod messages;
mod ring_buffer;
mod thread;

pub use error::AudioError;
pub use messages::{AudioFormat, FromAudio, ToAudio};
pub use ring_buffer::{AudioRingBuffer, DEFAULT_BUFFER_CAPACITY, RingBufferStats};
pub use thread::AudioThread;
