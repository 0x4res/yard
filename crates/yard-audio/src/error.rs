//! Audio error types for the yard-audio crate.
//!
//! Uses `thiserror` for typed errors as per project conventions.

use thiserror::Error;

/// Errors that can occur during audio operations.
#[derive(Error, Debug)]
pub enum AudioError {
    /// PipeWire is not available on the system.
    #[error("PipeWire is not available: {0}")]
    PipeWireUnavailable(String),

    /// Failed to create PipeWire main loop.
    #[error("Failed to create PipeWire main loop: {0}")]
    MainLoopCreation(String),

    /// Failed to spawn the audio thread.
    #[error("Failed to spawn audio thread: {0}")]
    ThreadSpawn(#[from] std::io::Error),

    /// The audio thread panicked unexpectedly.
    #[error("Audio thread panicked")]
    ThreadPanic,

    /// Timeout waiting for audio thread to shut down.
    #[error("Shutdown timeout after {0} seconds")]
    ShutdownTimeout(u64),

    /// Failed to create microphone capture stream.
    #[error("Microphone capture failed: {0}")]
    CaptureStreamCreation(String),

    /// Microphone access was denied.
    #[error("Microphone access denied: {0}")]
    CapturePermissionDenied(String),

    /// No microphone device available.
    #[error("No microphone device available")]
    NoMicrophoneDevice,
}
