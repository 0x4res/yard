//! Audio thread management for PipeWire integration.
//!
//! This module provides the `AudioThread` struct which manages a dedicated
//! thread running the PipeWire main loop. The audio thread is isolated from
//! the main calloop thread as per project architecture requirements.
//!
//! # Architecture
//!
//! The audio thread manages two PipeWire streams:
//! - **Playback stream**: Reads from ring buffer, writes to default audio sink
//! - **Capture stream**: Reads from default audio source, sends via channel
//!
//! Device hot-plug is handled automatically by PipeWire when streams are
//! connected with `AUTOCONNECT` flag and no specific target node.

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
struct CaptureState {
    /// Whether capture is currently active.
    active: bool,
    /// Current capture format.
    format: Option<AudioFormat>,
    /// Frames per packet required by AUDIN.
    frames_per_packet: u32,
    /// Accumulator for partial frames.
    frame_accumulator: Vec<u8>,
}

#[cfg(target_os = "linux")]
impl Default for CaptureState {
    fn default() -> Self {
        Self {
            active: false,
            format: None,
            frames_per_packet: 480,
            frame_accumulator: Vec::with_capacity(4096),
        }
    }
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
/// Implements both playback and capture streams with automatic device hot-plug
/// handling via PipeWire's AUTOCONNECT mechanism.
#[cfg(target_os = "linux")]
fn run_audio_loop(rx: Receiver<ToAudio>, capture_tx: Sender<FromAudio>) {
    use pipewire::context::Context;
    use pipewire::main_loop::MainLoop;
    use pipewire::spa::utils::Direction;
    use pipewire::stream::{Stream, StreamFlags, StreamListener};
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::mpsc::TryRecvError;

    debug!("Audio thread starting PipeWire main loop");

    // Create PipeWire main loop (not Rc variant - we need ownership)
    let main_loop = match MainLoop::new(None) {
        Ok(ml) => ml,
        Err(e) => {
            error!("Failed to create PipeWire MainLoop: {}", e);
            return;
        }
    };

    // Create context
    let context = match Context::new(&main_loop) {
        Ok(ctx) => ctx,
        Err(e) => {
            error!("Failed to create PipeWire Context: {}", e);
            return;
        }
    };

    // Connect to PipeWire
    let core = match context.connect(None) {
        Ok(c) => c,
        Err(e) => {
            error!("Failed to connect to PipeWire: {}", e);
            return;
        }
    };

    info!("Connected to PipeWire daemon");

    // Shared state wrapped in RefCell for interior mutability in callbacks
    let capture_state = Rc::new(RefCell::new(CaptureState::default()));
    let playback_state = Rc::new(RefCell::new(PlaybackState::default()));
    let capture_tx = Rc::new(capture_tx);

    // Playback stream and listener (created lazily when first audio data arrives)
    // Both must be kept alive for callbacks to work
    let playback_stream: Rc<RefCell<Option<Stream>>> = Rc::new(RefCell::new(None));
    let playback_listener: Rc<RefCell<Option<StreamListener<()>>>> = Rc::new(RefCell::new(None));

    // Capture stream and listener (created when StartCapture is received)
    let capture_stream: Rc<RefCell<Option<Stream>>> = Rc::new(RefCell::new(None));
    let capture_listener: Rc<RefCell<Option<StreamListener<()>>>> = Rc::new(RefCell::new(None));

    let running = Rc::new(RefCell::new(true));

    // Clone references for the timer callback
    let rx_clone = Rc::new(RefCell::new(rx));
    let running_check = running.clone();
    let playback_state_msg = playback_state.clone();
    let capture_state_msg = capture_state.clone();
    let capture_tx_msg = capture_tx.clone();
    let playback_stream_msg = playback_stream.clone();
    let playback_listener_msg = playback_listener.clone();
    let capture_stream_msg = capture_stream.clone();
    let capture_listener_msg = capture_listener.clone();
    let core_clone = core.clone();
    let main_loop_weak = main_loop.downgrade();

    // Add a timer source to poll for messages
    // PipeWire's main loop doesn't have a direct way to wake from external events,
    // so we use a short-interval timer to check the message channel
    let _timer = main_loop.loop_().add_timer(move |_| {
        let rx = rx_clone.borrow();

        // Process all pending messages
        loop {
            match rx.try_recv() {
                Ok(ToAudio::Shutdown) => {
                    debug!("Audio thread received shutdown command");

                    // Stop capture if active
                    {
                        let mut cs = capture_state_msg.borrow_mut();
                        if cs.active {
                            cs.active = false;
                            let _ = capture_tx_msg.send(FromAudio::CaptureStopped);
                        }
                    }

                    // Disconnect streams
                    if let Some(stream) = playback_stream_msg.borrow_mut().take() {
                        stream.disconnect().ok();
                    }
                    if let Some(stream) = capture_stream_msg.borrow_mut().take() {
                        stream.disconnect().ok();
                    }

                    // Log final buffer stats
                    let stats = playback_state_msg.borrow().buffer.stats();
                    debug!(
                        "Audio shutdown - buffer stats: {} overflows, {} underruns, {} bytes written",
                        stats.overflows, stats.underruns, stats.bytes_written
                    );

                    *running_check.borrow_mut() = false;

                    // Quit the main loop
                    if let Some(ml) = main_loop_weak.upgrade() {
                        ml.quit();
                    }
                    return;
                }

                Ok(ToAudio::PlayAudio { data, format }) => {
                    let mut ps = playback_state_msg.borrow_mut();

                    // Check for format change
                    if ps.format.as_ref() != Some(&format) {
                        debug!(
                            "Audio format changed to {}Hz {}ch {}bit, clearing buffer",
                            format.sample_rate, format.channels, format.bits_per_sample
                        );
                        ps.buffer.clear();
                        ps.format = Some(format.clone());

                        // Create or recreate playback stream with new format
                        drop(ps); // Release borrow before creating stream
                        create_playback_stream(
                            &core_clone,
                            &format,
                            playback_stream_msg.clone(),
                            playback_listener_msg.clone(),
                            playback_state_msg.clone(),
                        );
                        ps = playback_state_msg.borrow_mut();
                    }

                    // Push audio data to ring buffer
                    let written = ps.buffer.push(&data);
                    if written < data.len() {
                        warn!(
                            "Ring buffer overflow: only wrote {} of {} bytes",
                            written,
                            data.len()
                        );
                    }

                    // Log stats periodically
                    let stats = ps.buffer.stats();
                    if stats.bytes_written % 100_000 < data.len() as u64 {
                        debug!(
                            "Ring buffer: {:.1}% full, {} overflows, {} underruns",
                            stats.fill_percentage() * 100.0,
                            stats.overflows,
                            stats.underruns
                        );
                    }
                }

                Ok(ToAudio::SetVolume(vol)) => {
                    debug!("SetVolume received: {} (not yet implemented)", vol);
                    // TODO: Implement volume control via PipeWire stream properties
                }

                Ok(ToAudio::StartCapture {
                    format,
                    frames_per_packet,
                }) => {
                    debug!(
                        "StartCapture received: {}Hz {}ch {}bit, {} frames/packet",
                        format.sample_rate, format.channels, format.bits_per_sample, frames_per_packet
                    );

                    {
                        let mut cs = capture_state_msg.borrow_mut();
                        cs.format = Some(format.clone());
                        cs.frames_per_packet = frames_per_packet;
                        cs.active = true;
                        cs.frame_accumulator.clear();
                    }

                    // Create capture stream
                    create_capture_stream(
                        &core_clone,
                        &format,
                        frames_per_packet,
                        capture_stream_msg.clone(),
                        capture_listener_msg.clone(),
                        capture_state_msg.clone(),
                        capture_tx_msg.clone(),
                    );

                    info!(
                        "Microphone capture started: {}Hz {}ch {}bit",
                        format.sample_rate, format.channels, format.bits_per_sample
                    );
                }

                Ok(ToAudio::StopCapture) => {
                    debug!("StopCapture received");
                    let mut cs = capture_state_msg.borrow_mut();
                    if cs.active {
                        cs.active = false;
                        cs.format = None;

                        // Disconnect capture stream
                        if let Some(stream) = capture_stream_msg.borrow_mut().take() {
                            stream.disconnect().ok();
                        }

                        let _ = capture_tx_msg.send(FromAudio::CaptureStopped);
                        info!("Microphone capture stopped");
                    }
                }

                Ok(ToAudio::GetBufferStats) => {
                    let stats = playback_state_msg.borrow().buffer.stats();
                    debug!(
                        "Buffer stats requested: {:.1}% full, {} overflows, {} underruns",
                        stats.fill_percentage() * 100.0,
                        stats.overflows,
                        stats.underruns
                    );
                    let _ = capture_tx_msg.send(FromAudio::BufferStats(stats));
                }

                Ok(ToAudio::ClearBuffer) => {
                    debug!("ClearBuffer received");
                    let mut ps = playback_state_msg.borrow_mut();
                    ps.buffer.clear();
                    ps.buffer.reset_stats();
                }

                Err(TryRecvError::Empty) => {
                    // No more messages
                    break;
                }

                Err(TryRecvError::Disconnected) => {
                    debug!("Audio message channel closed");
                    let mut cs = capture_state_msg.borrow_mut();
                    if cs.active {
                        let _ = capture_tx_msg.send(FromAudio::CaptureStopped);
                    }
                    *running_check.borrow_mut() = false;
                    if let Some(ml) = main_loop_weak.upgrade() {
                        ml.quit();
                    }
                    return;
                }
            }
        }
    });

    // Enable timer with 10ms interval for message polling
    if let Some(timer) = _timer {
        timer.update_timer(
            Some(Duration::from_millis(10)),
            Some(Duration::from_millis(10)),
        );
    }

    // Run the main loop until shutdown
    debug!("Starting PipeWire main loop");
    main_loop.run();

    debug!("PipeWire main loop exited");
}

/// Creates a PipeWire playback stream.
///
/// The stream targets the default audio sink and will automatically follow
/// device changes (hot-plug) thanks to the AUTOCONNECT flag.
///
/// Both the stream and listener are stored in the provided holders to keep
/// the callbacks alive for the lifetime of the stream.
#[cfg(target_os = "linux")]
fn create_playback_stream(
    core: &pipewire::core::Core,
    format: &AudioFormat,
    stream_holder: Rc<RefCell<Option<pipewire::stream::Stream>>>,
    listener_holder: Rc<RefCell<Option<pipewire::stream::StreamListener<()>>>>,
    playback_state: Rc<RefCell<PlaybackState>>,
) {
    use pipewire::properties::properties;
    use pipewire::spa::param::audio::{AudioFormat as SpaAudioFormat, AudioInfoRaw};
    use pipewire::spa::pod::serialize::PodSerializer;
    use pipewire::spa::utils::Direction;
    use pipewire::stream::{Stream, StreamFlags};

    // Disconnect existing stream and drop old listener
    listener_holder.borrow_mut().take();
    if let Some(old_stream) = stream_holder.borrow_mut().take() {
        old_stream.disconnect().ok();
    }

    // Create stream properties
    let props = properties! {
        *pipewire::keys::MEDIA_TYPE => "Audio",
        *pipewire::keys::MEDIA_CATEGORY => "Playback",
        *pipewire::keys::MEDIA_ROLE => "Game",
        *pipewire::keys::NODE_NAME => "yard-rdp-playback",
        *pipewire::keys::APP_NAME => "YARD RDP Client",
    };

    // Create the stream
    let stream = match Stream::new(core, "yard-playback", props) {
        Ok(s) => s,
        Err(e) => {
            error!("Failed to create playback stream: {}", e);
            return;
        }
    };

    // Clone state for callback
    let playback_state_cb = playback_state.clone();

    // Set up process callback - this reads from the ring buffer and writes to PipeWire
    let listener = stream
        .add_local_listener_with_user_data(())
        .process(move |stream, _user_data| {
            match stream.dequeue_buffer() {
                Some(mut buffer) => {
                    // Get the data slice from the buffer
                    let datas = buffer.datas_mut();
                    if datas.is_empty() {
                        return;
                    }

                    let data = &mut datas[0];
                    if let Some(slice) = data.data() {
                        // Read from ring buffer into PipeWire buffer
                        let mut ps = playback_state_cb.borrow_mut();
                        let read = ps.buffer.pop(slice);

                        // Set the chunk size to how much we actually wrote
                        let chunk = data.chunk_mut();
                        *chunk.size_mut() = read as u32;
                        *chunk.offset_mut() = 0;
                        *chunk.stride_mut() =
                            (ps.format.as_ref().map_or(4, |f| {
                                (f.channels as i32) * (f.bits_per_sample as i32 / 8)
                            })) as i32;
                    }
                }
                None => {
                    // No buffer available, skip this cycle
                }
            }
        })
        .state_changed(|_old, new| {
            debug!("Playback stream state changed to {:?}", new);
        })
        .register();

    let listener = match listener {
        Ok(l) => l,
        Err(e) => {
            error!("Failed to register playback stream listener: {:?}", e);
            return;
        }
    };

    // Build the format pod for the stream
    // PipeWire expects SPA format pods for audio negotiation
    let spa_format = match format.bits_per_sample {
        16 => SpaAudioFormat::S16LE,
        24 => SpaAudioFormat::S24LE,
        32 => SpaAudioFormat::S32LE,
        _ => SpaAudioFormat::S16LE, // Default to 16-bit
    };

    let audio_info = AudioInfoRaw::new()
        .set_format(spa_format)
        .set_rate(format.sample_rate)
        .set_channels(format.channels as u32);

    let mut params_buffer = [0u8; 1024];
    let pod = match PodSerializer::serialize(
        std::io::Cursor::new(&mut params_buffer[..]),
        &pipewire::spa::param::audio::AudioInfoRaw::build(&audio_info),
    ) {
        Ok((_, pod)) => pod,
        Err(e) => {
            error!("Failed to serialize audio format pod: {:?}", e);
            return;
        }
    };

    // Connect the stream to the PipeWire graph
    // Direction::Output means we're providing audio TO PipeWire (playback)
    // No target ID (None) means follow the default sink - enables hot-plug
    let flags = StreamFlags::AUTOCONNECT | StreamFlags::MAP_BUFFERS | StreamFlags::RT_PROCESS;

    if let Err(e) = stream.connect(Direction::Output, None, flags, &mut [&pod]) {
        error!("Failed to connect playback stream: {}", e);
        return;
    }

    info!(
        "Playback stream connected: {}Hz {}ch {}bit",
        format.sample_rate, format.channels, format.bits_per_sample
    );

    // Store the stream and listener - both must be kept alive for callbacks to work
    *stream_holder.borrow_mut() = Some(stream);
    *listener_holder.borrow_mut() = Some(listener);
}

/// Creates a PipeWire capture stream.
///
/// The stream targets the default audio source and will automatically follow
/// device changes (hot-plug) thanks to the AUTOCONNECT flag.
///
/// Both the stream and listener are stored in the provided holders to keep
/// the callbacks alive for the lifetime of the stream.
#[cfg(target_os = "linux")]
fn create_capture_stream(
    core: &pipewire::core::Core,
    format: &AudioFormat,
    frames_per_packet: u32,
    stream_holder: Rc<RefCell<Option<pipewire::stream::Stream>>>,
    listener_holder: Rc<RefCell<Option<pipewire::stream::StreamListener<()>>>>,
    capture_state: Rc<RefCell<CaptureState>>,
    capture_tx: Rc<Sender<FromAudio>>,
) {
    use pipewire::properties::properties;
    use pipewire::spa::param::audio::{AudioFormat as SpaAudioFormat, AudioInfoRaw};
    use pipewire::spa::pod::serialize::PodSerializer;
    use pipewire::spa::utils::Direction;
    use pipewire::stream::{Stream, StreamFlags};

    // Disconnect existing stream and drop old listener
    listener_holder.borrow_mut().take();
    if let Some(old_stream) = stream_holder.borrow_mut().take() {
        old_stream.disconnect().ok();
    }

    // Create stream properties
    let props = properties! {
        *pipewire::keys::MEDIA_TYPE => "Audio",
        *pipewire::keys::MEDIA_CATEGORY => "Capture",
        *pipewire::keys::MEDIA_ROLE => "Communication",
        *pipewire::keys::NODE_NAME => "yard-rdp-capture",
        *pipewire::keys::APP_NAME => "YARD RDP Client",
    };

    // Create the stream
    let stream = match Stream::new(core, "yard-capture", props) {
        Ok(s) => s,
        Err(e) => {
            error!("Failed to create capture stream: {}", e);
            let _ = capture_tx.send(FromAudio::CaptureError(format!(
                "Failed to create capture stream: {}",
                e
            )));
            return;
        }
    };

    // Clone state for callback
    let format_clone = format.clone();
    let bytes_per_frame = (format.channels as usize) * (format.bits_per_sample as usize / 8);
    let bytes_per_packet = (frames_per_packet as usize) * bytes_per_frame;
    let capture_tx_err = capture_tx.clone();

    // Set up process callback - this reads from PipeWire and sends to network thread
    let listener = stream
        .add_local_listener_with_user_data(())
        .process(move |stream, _user_data| {
            match stream.dequeue_buffer() {
                Some(buffer) => {
                    let datas = buffer.datas();
                    if datas.is_empty() {
                        return;
                    }

                    let data = &datas[0];
                    if let Some(slice) = data.data() {
                        let chunk = data.chunk();
                        let size = chunk.size() as usize;

                        if size == 0 {
                            return;
                        }

                        // Get actual audio data
                        let audio_data = &slice[..size.min(slice.len())];

                        let mut cs = capture_state.borrow_mut();
                        if !cs.active {
                            return;
                        }

                        // Accumulate frames until we have enough for a packet
                        cs.frame_accumulator.extend_from_slice(audio_data);

                        // Send complete packets
                        while cs.frame_accumulator.len() >= bytes_per_packet {
                            let packet: Vec<u8> =
                                cs.frame_accumulator.drain(..bytes_per_packet).collect();

                            let _ = capture_tx.send(FromAudio::CapturedData {
                                data: packet,
                                format: format_clone.clone(),
                            });
                        }
                    }
                }
                None => {
                    // No buffer available
                }
            }
        })
        .state_changed(move |_old, new| {
            debug!("Capture stream state changed to {:?}", new);
        })
        .register();

    let listener = match listener {
        Ok(l) => l,
        Err(e) => {
            error!("Failed to register capture stream listener: {:?}", e);
            let _ = capture_tx_err.send(FromAudio::CaptureError(
                "Failed to register capture stream listener".to_string(),
            ));
            return;
        }
    };

    // Build the format pod
    let spa_format = match format.bits_per_sample {
        16 => SpaAudioFormat::S16LE,
        24 => SpaAudioFormat::S24LE,
        32 => SpaAudioFormat::S32LE,
        _ => SpaAudioFormat::S16LE,
    };

    let audio_info = AudioInfoRaw::new()
        .set_format(spa_format)
        .set_rate(format.sample_rate)
        .set_channels(format.channels as u32);

    let mut params_buffer = [0u8; 1024];
    let pod = match PodSerializer::serialize(
        std::io::Cursor::new(&mut params_buffer[..]),
        &pipewire::spa::param::audio::AudioInfoRaw::build(&audio_info),
    ) {
        Ok((_, pod)) => pod,
        Err(e) => {
            error!("Failed to serialize audio format pod: {:?}", e);
            let _ = capture_tx_err.send(FromAudio::CaptureError(format!(
                "Failed to serialize audio format: {:?}",
                e
            )));
            return;
        }
    };

    // Connect the stream to the PipeWire graph
    // Direction::Input means we're receiving audio FROM PipeWire (capture)
    let flags = StreamFlags::AUTOCONNECT | StreamFlags::MAP_BUFFERS | StreamFlags::RT_PROCESS;

    if let Err(e) = stream.connect(Direction::Input, None, flags, &mut [&pod]) {
        error!("Failed to connect capture stream: {}", e);
        let _ = capture_tx_err.send(FromAudio::CaptureError(format!(
            "Failed to connect capture stream: {}",
            e
        )));
        return;
    }

    info!(
        "Capture stream connected: {}Hz {}ch {}bit, {} frames/packet",
        format.sample_rate, format.channels, format.bits_per_sample, frames_per_packet
    );

    // Store the stream and listener - both must be kept alive for callbacks to work
    *stream_holder.borrow_mut() = Some(stream);
    *listener_holder.borrow_mut() = Some(listener);
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
