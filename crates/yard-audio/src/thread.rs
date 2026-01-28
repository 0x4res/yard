//! Audio thread management for PipeWire integration.
//!
//! This module provides the `AudioThread` struct which manages a dedicated
//! thread running the PipeWire main loop. The audio thread is isolated from
//! the main calloop thread as per project architecture requirements.

use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use tracing::{debug, error, info, warn};

use crate::error::AudioError;
use crate::messages::{AudioFormat, FromAudio, ToAudio};
use crate::ring_buffer::AudioRingBuffer;

/// Timeout for graceful shutdown (seconds).
const SHUTDOWN_TIMEOUT_SECS: u64 = 2;

/// Manages the dedicated PipeWire audio thread.
///
/// The audio thread runs independently from the main calloop event loop,
/// processing audio commands via the `ToAudio` channel. When dropped,
/// the thread is gracefully shut down.
///
/// # Example
///
/// ```ignore
/// use yard_audio::{AudioThread, ToAudio};
///
/// // Spawn the audio thread
/// let audio = AudioThread::spawn()?;
///
/// // Get a sender to communicate with the audio thread
/// let sender = audio.sender();
/// sender.send(ToAudio::SetVolume(0.8))?;
///
/// // Thread is automatically shut down when AudioThread is dropped
/// ```
pub struct AudioThread {
    /// Sender for commands to the audio thread.
    tx: Sender<ToAudio>,
    /// Receiver for captured audio data from the audio thread.
    capture_rx: Option<Receiver<FromAudio>>,
    /// Handle to the spawned thread.
    handle: Option<JoinHandle<()>>,
}

impl AudioThread {
    /// Spawns a new audio thread with PipeWire integration.
    ///
    /// Initializes PipeWire and starts the main loop in a dedicated thread.
    /// Returns an error if PipeWire is not available or fails to initialize.
    ///
    /// # Errors
    ///
    /// Returns `AudioError::PipeWireUnavailable` if PipeWire cannot be initialized.
    /// Returns `AudioError::ThreadSpawn` if the thread fails to spawn.
    #[cfg(target_os = "linux")]
    pub fn spawn() -> Result<Self, AudioError> {
        // Initialize PipeWire library
        // Note: pipewire::init() should be called once per process
        // It's safe to call multiple times but logs a warning
        pipewire::init();
        info!("PipeWire initialized");

        let (tx, rx) = mpsc::channel::<ToAudio>();
        // Channel for captured audio data (audio thread → network thread)
        let (capture_tx, capture_rx) = mpsc::channel::<FromAudio>();

        let handle = thread::Builder::new()
            .name("yard-audio".to_string())
            .spawn(move || {
                run_audio_loop(rx, capture_tx);
            })?;

        info!("Audio thread spawned");
        Ok(Self {
            tx,
            capture_rx: Some(capture_rx),
            handle: Some(handle),
        })
    }

    /// Spawns a stub audio thread (non-Linux platforms).
    ///
    /// On non-Linux platforms, this creates a minimal thread that only
    /// responds to shutdown commands, as PipeWire is not available.
    #[cfg(not(target_os = "linux"))]
    pub fn spawn() -> Result<Self, AudioError> {
        let (tx, rx) = mpsc::channel::<ToAudio>();
        // Channel for captured audio data (not used on non-Linux, but needed for API)
        let (capture_tx, capture_rx) = mpsc::channel::<FromAudio>();

        let handle = thread::Builder::new()
            .name("yard-audio-stub".to_string())
            .spawn(move || {
                debug!("Audio stub thread started (non-Linux)");
                // Just wait for shutdown
                loop {
                    match rx.recv() {
                        Ok(ToAudio::Shutdown) => {
                            debug!("Audio stub thread received shutdown");
                            break;
                        }
                        Ok(ToAudio::StartCapture { format: _, .. }) => {
                            // Send error on non-Linux platforms
                            warn!("Microphone capture not available on non-Linux platforms");
                            let _ = capture_tx.send(FromAudio::CaptureError(
                                "PipeWire not available on this platform".to_string(),
                            ));
                        }
                        Ok(_) => {
                            // Ignore other messages on non-Linux
                        }
                        Err(_) => {
                            // Channel closed
                            break;
                        }
                    }
                }
                debug!("Audio stub thread exiting");
            })?;

        warn!("Audio thread spawned in stub mode (PipeWire not available on this platform)");
        Ok(Self {
            tx,
            capture_rx: Some(capture_rx),
            handle: Some(handle),
        })
    }

    /// Returns a clone of the sender for communicating with the audio thread.
    ///
    /// Multiple senders can be created to allow both the main thread and
    /// network thread to send audio commands.
    pub fn sender(&self) -> Sender<ToAudio> {
        self.tx.clone()
    }

    /// Takes the receiver for captured audio data.
    ///
    /// This can only be called once - subsequent calls return `None`.
    /// The receiver is used by the AUDIN handler to get captured microphone
    /// data and send it to the RDP server.
    pub fn take_capture_receiver(&mut self) -> Option<Receiver<FromAudio>> {
        self.capture_rx.take()
    }

    /// Checks if the audio thread is still running.
    pub fn is_running(&self) -> bool {
        self.handle.as_ref().is_some_and(|h| !h.is_finished())
    }

    /// Sends a shutdown signal and waits for the thread to exit.
    ///
    /// This is called automatically by `Drop`, but can be called manually
    /// for explicit shutdown with error handling.
    ///
    /// # Errors
    ///
    /// Returns `AudioError::ShutdownTimeout` if the thread doesn't exit
    /// within the timeout period.
    /// Returns `AudioError::ThreadPanic` if the thread panicked.
    pub fn shutdown(&mut self) -> Result<(), AudioError> {
        // Send shutdown signal
        if self.tx.send(ToAudio::Shutdown).is_err() {
            debug!("Audio thread already terminated (channel closed)");
        }

        // Wait for thread to exit
        if let Some(handle) = self.handle.take() {
            // Use a polling approach with timeout
            let start = std::time::Instant::now();
            let timeout = Duration::from_secs(SHUTDOWN_TIMEOUT_SECS);

            while !handle.is_finished() {
                if start.elapsed() > timeout {
                    warn!(
                        "Audio thread did not exit within {} seconds",
                        SHUTDOWN_TIMEOUT_SECS
                    );
                    return Err(AudioError::ShutdownTimeout(SHUTDOWN_TIMEOUT_SECS));
                }
                thread::sleep(Duration::from_millis(10));
            }

            match handle.join() {
                Ok(()) => {
                    debug!("Audio thread shut down successfully");
                    Ok(())
                }
                Err(_) => {
                    error!("Audio thread panicked during shutdown");
                    Err(AudioError::ThreadPanic)
                }
            }
        } else {
            Ok(())
        }
    }
}

impl Drop for AudioThread {
    fn drop(&mut self) {
        if let Err(e) = self.shutdown() {
            error!("Error during audio thread shutdown: {}", e);
        }
    }
}

/// State for microphone capture.
#[cfg(target_os = "linux")]
#[derive(Default)]
struct CaptureState {
    /// Whether capture is currently active.
    active: bool,
    /// Current capture format.
    format: Option<AudioFormat>,
    /// Frames per packet.
    frames_per_packet: u32,
}

/// State for audio playback.
#[cfg(target_os = "linux")]
struct PlaybackState {
    /// Ring buffer for audio data.
    buffer: AudioRingBuffer,
    /// Current playback format.
    format: Option<AudioFormat>,
}

#[cfg(target_os = "linux")]
impl Default for PlaybackState {
    fn default() -> Self {
        Self {
            buffer: AudioRingBuffer::with_default_capacity(),
            format: None,
        }
    }
}

/// Runs the PipeWire main loop in the audio thread.
///
/// Uses a polling approach with short iteration timeouts to handle
/// both PipeWire events and incoming messages from other threads.
#[cfg(target_os = "linux")]
fn run_audio_loop(rx: Receiver<ToAudio>, capture_tx: Sender<FromAudio>) {
    use pipewire::main_loop::MainLoopRc;
    use std::sync::mpsc::TryRecvError;

    debug!("Audio thread starting PipeWire main loop");

    // Create PipeWire main loop
    let main_loop = match MainLoopRc::new(None) {
        Ok(ml) => ml,
        Err(e) => {
            error!("Failed to create PipeWire MainLoop: {}", e);
            return;
        }
    };

    let mut capture_state = CaptureState::default();
    let mut playback_state = PlaybackState::default();
    let mut running = true;

    // Polling loop: iterate PipeWire with short timeout, then check messages
    // This avoids needing to send MainLoopRc across threads (it's !Send)
    while running {
        // Process PipeWire events with a short timeout (10ms)
        // This allows us to check for incoming messages frequently
        main_loop.loop_().iterate(Duration::from_millis(10));

        // Check for incoming messages (non-blocking)
        loop {
            match rx.try_recv() {
                Ok(ToAudio::Shutdown) => {
                    debug!("Audio thread received shutdown command");
                    if capture_state.active {
                        capture_state.active = false;
                        let _ = capture_tx.send(FromAudio::CaptureStopped);
                    }
                    // Log final buffer stats
                    let stats = playback_state.buffer.stats();
                    debug!(
                        "Audio shutdown - buffer stats: {} overflows, {} underruns, {} bytes written",
                        stats.overflows, stats.underruns, stats.bytes_written
                    );
                    running = false;
                    break;
                }
                Ok(ToAudio::PlayAudio { data, format }) => {
                    // Check for format change
                    if playback_state.format.as_ref() != Some(&format) {
                        debug!(
                            "Audio format changed to {}Hz {}ch {}bit, clearing buffer",
                            format.sample_rate, format.channels, format.bits_per_sample
                        );
                        playback_state.buffer.clear();
                        playback_state.format = Some(format);
                    }

                    // Push audio data to ring buffer
                    let written = playback_state.buffer.push(&data);
                    if written < data.len() {
                        // This shouldn't happen with overflow handling, but log if it does
                        warn!(
                            "Ring buffer overflow: only wrote {} of {} bytes",
                            written,
                            data.len()
                        );
                    }

                    // Log stats periodically (every ~100 pushes based on buffer fill)
                    let stats = playback_state.buffer.stats();
                    if stats.bytes_written % 100_000 < data.len() as u64 {
                        debug!(
                            "Ring buffer: {:.1}% full, {} overflows, {} underruns",
                            stats.fill_percentage() * 100.0,
                            stats.overflows,
                            stats.underruns
                        );
                    }

                    // TODO: Create PipeWire playback stream that reads from ring buffer
                }
                Ok(ToAudio::SetVolume(vol)) => {
                    debug!("SetVolume received: {} (not yet implemented)", vol);
                }
                Ok(ToAudio::StartCapture {
                    format,
                    frames_per_packet,
                }) => {
                    debug!(
                        "StartCapture received: {}Hz {}ch {}bit, {} frames/packet",
                        format.sample_rate,
                        format.channels,
                        format.bits_per_sample,
                        frames_per_packet
                    );

                    capture_state.format = Some(format);
                    capture_state.frames_per_packet = frames_per_packet;
                    capture_state.active = true;

                    // TODO: Create PipeWire capture stream
                    info!(
                        "Microphone capture requested: {}Hz {}ch {}bit",
                        format.sample_rate, format.channels, format.bits_per_sample
                    );
                }
                Ok(ToAudio::StopCapture) => {
                    debug!("StopCapture received");
                    if capture_state.active {
                        capture_state.active = false;
                        capture_state.format = None;
                        let _ = capture_tx.send(FromAudio::CaptureStopped);
                        info!("Microphone capture stopped");
                    }
                }
                Ok(ToAudio::GetBufferStats) => {
                    let stats = playback_state.buffer.stats();
                    debug!(
                        "Buffer stats requested: {:.1}% full, {} overflows, {} underruns",
                        stats.fill_percentage() * 100.0,
                        stats.overflows,
                        stats.underruns
                    );
                    let _ = capture_tx.send(FromAudio::BufferStats(stats));
                }
                Ok(ToAudio::ClearBuffer) => {
                    debug!("ClearBuffer received");
                    playback_state.buffer.clear();
                    playback_state.buffer.reset_stats();
                }
                Err(TryRecvError::Empty) => {
                    // No more messages, continue with PipeWire iteration
                    break;
                }
                Err(TryRecvError::Disconnected) => {
                    debug!("Audio message channel closed");
                    if capture_state.active {
                        let _ = capture_tx.send(FromAudio::CaptureStopped);
                    }
                    running = false;
                    break;
                }
            }
        }
    }

    debug!("PipeWire main loop exited");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_audio_format_creation() {
        use crate::messages::AudioFormat;
        let format = AudioFormat::new(48000, 2, 16);
        assert_eq!(format.sample_rate, 48000);
    }

    #[test]
    fn test_to_audio_message_variants() {
        use crate::messages::AudioFormat;

        let shutdown = ToAudio::Shutdown;
        assert!(matches!(shutdown, ToAudio::Shutdown));

        let play = ToAudio::PlayAudio {
            data: vec![0; 100],
            format: AudioFormat::default(),
        };
        assert!(matches!(play, ToAudio::PlayAudio { .. }));

        let volume = ToAudio::SetVolume(0.5);
        assert!(matches!(volume, ToAudio::SetVolume(_)));

        let start_capture = ToAudio::StartCapture {
            format: AudioFormat::new(48000, 1, 16),
            frames_per_packet: 480,
        };
        assert!(matches!(start_capture, ToAudio::StartCapture { .. }));

        let stop_capture = ToAudio::StopCapture;
        assert!(matches!(stop_capture, ToAudio::StopCapture));
    }

    // Note: Tests requiring actual PipeWire are marked #[ignore]
    // They should be run locally on a Linux system with PipeWire

    #[test]
    #[ignore = "Requires PipeWire on Linux"]
    fn test_audio_thread_spawn_and_shutdown() {
        let audio = AudioThread::spawn().expect("Failed to spawn audio thread");
        assert!(audio.is_running());

        let sender = audio.sender();
        sender.send(ToAudio::SetVolume(0.5)).unwrap();

        // Drop triggers shutdown
        drop(audio);
    }

    #[test]
    #[ignore = "Requires PipeWire on Linux"]
    fn test_audio_thread_explicit_shutdown() {
        let mut audio = AudioThread::spawn().expect("Failed to spawn audio thread");
        assert!(audio.is_running());

        audio.shutdown().expect("Shutdown failed");
        assert!(!audio.is_running());
    }

    #[test]
    #[ignore = "Requires PipeWire on Linux"]
    fn test_audio_thread_multiple_senders() {
        let audio = AudioThread::spawn().expect("Failed to spawn audio thread");

        let sender1 = audio.sender();
        let sender2 = audio.sender();

        sender1.send(ToAudio::SetVolume(0.3)).unwrap();
        sender2.send(ToAudio::SetVolume(0.7)).unwrap();

        drop(audio);
    }

    // Non-Linux stub tests can run anywhere
    #[cfg(not(target_os = "linux"))]
    mod stub_tests {
        use super::*;

        #[test]
        fn test_stub_audio_thread_spawn() {
            let audio = AudioThread::spawn().expect("Failed to spawn stub audio thread");
            assert!(audio.is_running());
            drop(audio);
        }
    }
}
