//! Wayland window implementation for YARD.
//!
//! This module provides the WaylandWindow struct which handles:
//! - Window creation using xdg_shell
//! - Surface management and buffer handling
//! - Frame rendering via shared memory

#[cfg(target_os = "linux")]
mod linux {
    use calloop::EventLoop;
    use calloop::channel::Sender;
    use calloop_wayland_source::WaylandSource;
    use smithay_client_toolkit::compositor::{CompositorHandler, CompositorState};
    use smithay_client_toolkit::output::{OutputHandler, OutputState};
    use smithay_client_toolkit::reexports::client::globals::registry_queue_init;
    use smithay_client_toolkit::reexports::client::protocol::wl_keyboard::WlKeyboard;
    use smithay_client_toolkit::reexports::client::protocol::wl_output::WlOutput;
    use smithay_client_toolkit::reexports::client::protocol::wl_pointer::WlPointer;
    use smithay_client_toolkit::reexports::client::protocol::wl_seat::WlSeat;
    use smithay_client_toolkit::reexports::client::protocol::wl_shm::Format as WlShmFormat;
    use smithay_client_toolkit::reexports::client::protocol::wl_surface::WlSurface;
    use smithay_client_toolkit::reexports::client::{Connection, QueueHandle};
    use smithay_client_toolkit::reexports::csd_frame::WindowState;
    use smithay_client_toolkit::registry::{ProvidesRegistryState, RegistryState};
    use smithay_client_toolkit::seat::keyboard::{
        KeyEvent, KeyboardHandler, Keysym, Modifiers, RawModifiers,
    };
    use smithay_client_toolkit::seat::pointer::{PointerEvent, PointerEventKind, PointerHandler};
    use smithay_client_toolkit::seat::{Capability, SeatHandler, SeatState};
    use smithay_client_toolkit::shell::WaylandSurface;
    use smithay_client_toolkit::shell::xdg::XdgShell;
    use smithay_client_toolkit::shell::xdg::window::{
        Window, WindowConfigure, WindowDecorations, WindowHandler,
    };
    use smithay_client_toolkit::shm::slot::{Buffer, SlotPool};
    use smithay_client_toolkit::shm::{Shm, ShmHandler};
    use smithay_client_toolkit::{
        delegate_compositor, delegate_keyboard, delegate_output, delegate_pointer,
        delegate_registry, delegate_seat, delegate_shm, delegate_xdg_shell, delegate_xdg_window,
        registry_handlers,
    };
    use std::collections::{HashMap, HashSet};

    /// Information about a connected monitor (Story 3.1).
    ///
    /// This struct stores all relevant information about a Wayland output,
    /// including resolution, position, and refresh rate.
    #[derive(Debug, Clone)]
    pub struct MonitorInfo {
        /// Unique Wayland output ID.
        pub id: u32,
        /// Monitor name (e.g., "DP-1", "HDMI-A-1").
        pub name: String,
        /// Make/manufacturer if available.
        pub make: Option<String>,
        /// Model name if available.
        pub model: Option<String>,
        /// Current resolution width in pixels.
        pub width: u32,
        /// Current resolution height in pixels.
        pub height: u32,
        /// Physical position X offset in the compositor's coordinate space.
        pub x: i32,
        /// Physical position Y offset in the compositor's coordinate space.
        pub y: i32,
        /// Refresh rate in millihertz (e.g., 60000 = 60Hz).
        pub refresh_mhz: u32,
        /// Scale factor (1 = no scaling, 2 = HiDPI).
        pub scale: i32,
    }

    impl MonitorInfo {
        /// Returns the refresh rate in Hz (e.g., 60.0).
        pub fn refresh_hz(&self) -> f64 {
            self.refresh_mhz as f64 / 1000.0
        }
    }

    /// Messages sent from the Wayland window to the main application.
    #[derive(Debug)]
    pub enum WindowEvent {
        /// Window was closed by the user.
        CloseRequested,
        /// Window was resized.
        Resized { width: u32, height: u32 },
        /// Window needs to be redrawn.
        RedrawRequested,
        /// Fullscreen state changed.
        FullscreenChanged { is_fullscreen: bool },
        /// A keyboard shortcut was pressed.
        KeyboardShortcut(KeyboardShortcut),
        /// A key was pressed (scancode-based input).
        /// The scancode is an RDP scancode (translated from evdev).
        /// Used for special keys, function keys, and modifier combinations.
        KeyPressed { scancode: u16 },
        /// A key was released (scancode-based input).
        /// The scancode is an RDP scancode (translated from evdev).
        KeyReleased { scancode: u16 },
        /// A Unicode character was typed (character-based input).
        /// Used for international keyboard layouts and special characters.
        /// This bypasses scancode translation for better layout support.
        UnicodeKeyPressed { character: char },
        /// A Unicode character key was released.
        UnicodeKeyReleased { character: char },
        /// Mouse pointer moved.
        /// Coordinates are in surface-local space (f64 for subpixel precision).
        MouseMove { x: f64, y: f64 },
        /// Mouse button pressed or released.
        /// Button codes are Linux evdev button codes (BTN_LEFT=0x110, etc.).
        MouseButton {
            button: u32,
            pressed: bool,
            x: f64,
            y: f64,
        },
        /// Mouse wheel/scroll event.
        /// Value is scroll amount (positive = up/left, negative = down/right).
        MouseAxis {
            horizontal: bool,
            value: f64,
            x: f64,
            y: f64,
        },
    }

    /// Keyboard shortcuts that the window can detect.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum KeyboardShortcut {
        /// Toggle fullscreen mode (Ctrl+Alt+Enter).
        ToggleFullscreen,
        /// Disconnect from remote session (Ctrl+Alt+End).
        Disconnect,
    }

    /// Configuration for creating a Wayland window.
    #[derive(Debug, Clone)]
    pub struct WindowConfig {
        /// Window title.
        pub title: String,
        /// Initial width in pixels.
        pub width: u32,
        /// Initial height in pixels.
        pub height: u32,
        /// Start in fullscreen mode.
        pub fullscreen: bool,
    }

    impl Default for WindowConfig {
        fn default() -> Self {
            Self {
                title: "YARD".to_string(),
                width: 1280,
                height: 720,
                fullscreen: false,
            }
        }
    }

    impl WindowConfig {
        /// Creates a window config with connection info in the title.
        pub fn with_connection_info(host: &str, port: u16, username: Option<&str>) -> Self {
            let title = match username {
                Some(user) => format!("YARD - {}:{} [{}]", host, port, user),
                None => format!("YARD - {}:{}", host, port),
            };
            Self {
                title,
                ..Default::default()
            }
        }

        /// Sets the window size.
        #[must_use]
        pub fn with_size(mut self, width: u32, height: u32) -> Self {
            self.width = width;
            self.height = height;
            self
        }

        /// Sets fullscreen mode.
        #[must_use]
        pub fn with_fullscreen(mut self, fullscreen: bool) -> Self {
            self.fullscreen = fullscreen;
            self
        }
    }

    /// The main Wayland window state.
    pub struct WaylandWindow {
        registry_state: RegistryState,
        output_state: OutputState,
        seat_state: SeatState,
        shm: Shm,
        pool: SlotPool,
        window: Window,
        width: u32,
        height: u32,
        buffer: Option<Buffer>,
        /// Retained frame content for partial updates.
        /// This preserves pixels from previous frames so partial updates
        /// only modify the changed regions.
        retained_content: Vec<u8>,
        event_tx: Sender<WindowEvent>,
        close_requested: bool,
        dirty: bool,
        /// Whether the window is currently in fullscreen mode.
        is_fullscreen: bool,
        /// Current keyboard modifiers state.
        modifiers: Modifiers,
        /// Whether a keyboard device is available.
        has_keyboard: bool,
        /// Whether a pointer device is available.
        has_pointer: bool,
        /// Current pointer position (surface-local coordinates).
        pointer_position: (f64, f64),
        /// Currently pressed mouse buttons (evdev codes).
        /// Used to release buttons when pointer leaves surface.
        pressed_buttons: Vec<u32>,
        /// Remote desktop width (for coordinate mapping).
        remote_width: u32,
        /// Remote desktop height (for coordinate mapping).
        remote_height: u32,
        /// Keys currently pressed via Unicode input (tracked by raw_code).
        /// Used to ensure release events match the input method of press events.
        unicode_keys_pressed: HashSet<u32>,
        /// Current XKB layout index (for logging layout changes).
        current_layout: u32,
        /// Detected monitors (Story 3.1).
        /// Maps WlOutput ID to MonitorInfo for quick lookup.
        monitors: HashMap<u32, MonitorInfo>,
        /// WlOutput references for fullscreen targeting.
        /// Kept separate because WlOutput doesn't implement Clone.
        outputs: HashMap<u32, WlOutput>,
    }

    impl WaylandWindow {
        /// Creates a new Wayland window.
        ///
        /// Returns the window state, event loop, and a channel receiver for window events.
        #[allow(clippy::type_complexity)]
        pub fn new(
            config: WindowConfig,
        ) -> Result<
            (
                EventLoop<'static, Self>,
                Self,
                calloop::channel::Channel<WindowEvent>,
            ),
            Box<dyn std::error::Error>,
        > {
            // Connect to Wayland
            let conn = Connection::connect_to_env()?;
            let (globals, event_queue) = registry_queue_init(&conn)?;
            let qh = event_queue.handle();

            // Create event loop
            let event_loop: EventLoop<Self> = EventLoop::try_new()?;
            let loop_handle = event_loop.handle();

            // Insert Wayland source
            WaylandSource::new(conn.clone(), event_queue).insert(loop_handle.clone())?;

            // Create channel for window events
            let (event_tx, event_rx) = calloop::channel::channel();

            // Initialize states
            let compositor = CompositorState::bind(&globals, &qh)?;
            let xdg_shell = XdgShell::bind(&globals, &qh)?;
            let shm = Shm::bind(&globals, &qh)?;
            let seat_state = SeatState::new(&globals, &qh);

            // Create surface and window
            let surface = compositor.create_surface(&qh);
            let window = xdg_shell.create_window(surface, WindowDecorations::ServerDefault, &qh);

            window.set_title(config.title);
            window.set_app_id("yard");
            window.set_min_size(Some((320, 240)));
            window.commit();

            // Create shared memory pool for buffers
            let pool = SlotPool::new((config.width * config.height * 4) as usize, &shm)?;

            // Initialize retained content buffer (black/transparent)
            let retained_size = (config.width * config.height * 4) as usize;
            let retained_content = vec![0u8; retained_size];

            // Request fullscreen if configured
            let start_fullscreen = config.fullscreen;
            if start_fullscreen {
                window.set_fullscreen(None);
            }

            let state = Self {
                registry_state: RegistryState::new(&globals),
                output_state: OutputState::new(&globals, &qh),
                seat_state,
                shm,
                pool,
                window,
                width: config.width,
                height: config.height,
                buffer: None,
                retained_content,
                event_tx,
                close_requested: false,
                dirty: true,
                is_fullscreen: false, // Will be updated when compositor confirms
                modifiers: Modifiers::default(),
                has_keyboard: false,
                has_pointer: false,
                pointer_position: (0.0, 0.0),
                pressed_buttons: Vec::new(),
                remote_width: config.width, // Default to window size, updated by set_remote_resolution
                remote_height: config.height,
                unicode_keys_pressed: HashSet::new(),
                current_layout: 0,
                monitors: HashMap::new(),
                outputs: HashMap::new(),
            };

            Ok((event_loop, state, event_rx))
        }

        /// Returns true if the window close was requested.
        pub fn close_requested(&self) -> bool {
            self.close_requested
        }

        /// Returns the current window dimensions.
        pub fn dimensions(&self) -> (u32, u32) {
            (self.width, self.height)
        }

        /// Returns true if the window is currently in fullscreen mode.
        pub fn is_fullscreen(&self) -> bool {
            self.is_fullscreen
        }

        /// Requests fullscreen mode on the specified output.
        ///
        /// Pass `None` to use the current output (compositor's choice).
        /// The actual fullscreen state change is confirmed via the configure event.
        pub fn set_fullscreen(&self, output: Option<&WlOutput>) {
            self.window.set_fullscreen(output);
        }

        /// Requests to exit fullscreen mode.
        ///
        /// The actual state change is confirmed via the configure event.
        pub fn unset_fullscreen(&self) {
            self.window.unset_fullscreen();
        }

        /// Toggles between fullscreen and windowed mode.
        pub fn toggle_fullscreen(&self) {
            if self.is_fullscreen {
                self.unset_fullscreen();
            } else {
                self.set_fullscreen(None);
            }
        }

        /// Sets the remote desktop resolution for coordinate mapping.
        ///
        /// Mouse coordinates are mapped from window space to remote desktop space.
        /// Call this when the connection is established with the server's desktop size.
        pub fn set_remote_resolution(&mut self, width: u32, height: u32) {
            self.remote_width = width;
            self.remote_height = height;
            tracing::debug!(
                "Remote resolution set to {}x{} (window: {}x{})",
                width,
                height,
                self.width,
                self.height
            );
        }

        /// Returns the remote desktop resolution (width, height).
        ///
        /// Used by the main thread to map window-local mouse coordinates
        /// to remote desktop coordinates.
        pub fn remote_resolution(&self) -> (u32, u32) {
            (self.remote_width, self.remote_height)
        }

        /// Returns information about all detected monitors (Story 3.1).
        ///
        /// This queries the OutputState for current monitor information.
        /// Call this after the Wayland connection is established and outputs
        /// have been enumerated.
        pub fn get_monitors(&self) -> Vec<MonitorInfo> {
            self.monitors.values().cloned().collect()
        }

        /// Returns the number of detected monitors.
        pub fn monitor_count(&self) -> usize {
            self.monitors.len()
        }

        /// Returns a reference to a specific WlOutput by its ID.
        ///
        /// Used for targeting fullscreen to a specific monitor.
        pub fn get_output(&self, id: u32) -> Option<&WlOutput> {
            self.outputs.get(&id)
        }

        /// Returns the primary monitor (the one at position 0,0).
        ///
        /// If no monitor is at 0,0, returns the first detected monitor.
        pub fn primary_monitor(&self) -> Option<MonitorInfo> {
            // Look for monitor at position (0, 0) - typically the primary
            self.monitors
                .values()
                .find(|m| m.x == 0 && m.y == 0)
                .cloned()
                .or_else(|| self.monitors.values().next().cloned())
        }

        /// Logs information about all detected monitors.
        ///
        /// Call this after initialization to see what monitors were detected.
        /// Useful for debugging multi-monitor setups.
        pub fn log_monitors(&self) {
            if self.monitors.is_empty() {
                tracing::warn!("No monitors detected");
                return;
            }

            tracing::info!("Detected {} monitor(s):", self.monitors.len());
            for monitor in self.monitors.values() {
                tracing::info!(
                    "  {} ({}): {}x{}@{:.1}Hz at ({}, {}), scale={}",
                    monitor.name,
                    monitor.id,
                    monitor.width,
                    monitor.height,
                    monitor.refresh_hz(),
                    monitor.x,
                    monitor.y,
                    monitor.scale
                );
            }
            // TODO(Story 3.1): Log warning if wlr-output-management is not available
            // for enhanced multi-monitor support (AC 2). Currently using basic wl_output.
        }

        /// Creates a MonitorInfo from OutputState info.
        ///
        /// Helper to avoid code duplication between new_output and update_output.
        fn create_monitor_info(
            id: u32,
            info: &smithay_client_toolkit::output::OutputInfo,
        ) -> MonitorInfo {
            MonitorInfo {
                id,
                name: info
                    .name
                    .clone()
                    .unwrap_or_else(|| format!("Output-{}", id)),
                make: info.make.clone(),
                model: info.model.clone(),
                width: info.logical_size.map(|(w, _)| w as u32).unwrap_or(0),
                height: info.logical_size.map(|(_, h)| h as u32).unwrap_or(0),
                x: info.location.0,
                y: info.location.1,
                refresh_mhz: info
                    .modes
                    .iter()
                    .find(|m| m.current)
                    .map(|m| m.refresh_rate as u32)
                    .unwrap_or(60000),
                scale: info.scale_factor,
            }
        }

        /// Draws a solid color frame (placeholder until real frames arrive).
        ///
        /// Errors are logged but do not propagate - this allows graceful degradation
        /// if buffer allocation fails temporarily.
        pub fn draw_solid(&mut self, r: u8, g: u8, b: u8) {
            if !self.dirty {
                return;
            }

            let width = self.width;
            let height = self.height;
            let stride = width * 4;

            // Get or create buffer
            let (buffer, canvas) = match self.pool.create_buffer(
                width as i32,
                height as i32,
                stride as i32,
                WlShmFormat::Argb8888,
            ) {
                Ok(result) => result,
                Err(e) => {
                    tracing::error!("Failed to create buffer for solid frame: {}", e);
                    return;
                }
            };

            // Fill with solid color (ARGB format)
            let color = [b, g, r, 255u8]; // BGRA order for ARGB8888
            for chunk in canvas.chunks_exact_mut(4) {
                chunk.copy_from_slice(&color);
            }

            // Update retained content with the solid color
            // This ensures partial updates work correctly on top of placeholder
            for chunk in self.retained_content.chunks_exact_mut(4) {
                chunk.copy_from_slice(&color);
            }

            // Attach and commit
            self.window
                .wl_surface()
                .attach(Some(buffer.wl_buffer()), 0, 0);
            self.window
                .wl_surface()
                .damage_buffer(0, 0, width as i32, height as i32);
            self.window.wl_surface().commit();

            self.buffer = Some(buffer);
            self.dirty = false;
        }

        /// Draws a full frame from raw BGRA pixel data.
        ///
        /// This replaces the entire window content with the provided frame.
        pub fn draw_frame(&mut self, data: &[u8], width: u32, height: u32) {
            self.draw_frame_at(data, width, height, 0, 0);
        }

        /// Draws a frame at a specific position (for partial updates).
        ///
        /// For full frames, use x=0, y=0 and width/height matching the window.
        /// For partial updates, specify the region position within the desktop.
        ///
        /// The `stride` is calculated as `width * 4` (BGRA format, no padding).
        /// If the source data has different stride, use `draw_frame_at_with_stride`.
        pub fn draw_frame_at(&mut self, data: &[u8], width: u32, height: u32, x: u32, y: u32) {
            self.draw_frame_at_with_stride(data, width, height, x, y, width * 4);
        }

        /// Draws a frame at a specific position with explicit stride.
        ///
        /// Use this when the source data has padding between rows (stride > width * 4).
        pub fn draw_frame_at_with_stride(
            &mut self,
            data: &[u8],
            width: u32,
            height: u32,
            x: u32,
            y: u32,
            stride: u32,
        ) {
            let expected_len = (stride * height) as usize;

            if data.len() < expected_len {
                tracing::warn!(
                    "Frame data size mismatch: expected at least {}, got {}",
                    expected_len,
                    data.len()
                );
                return;
            }

            // For partial updates, we need to update only a region of the existing buffer
            // For now, handle full-frame updates (where frame size matches window)
            // and partial updates by blitting into the existing buffer

            if x == 0 && y == 0 && width == self.width && height == self.height {
                // Full frame update - simple case
                self.draw_full_frame(data, width, height, stride);
            } else {
                // Partial update - blit into existing buffer
                self.draw_partial_frame(data, width, height, x, y, stride);
            }
        }

        /// Draws a full frame that replaces the entire window content.
        fn draw_full_frame(&mut self, data: &[u8], width: u32, height: u32, stride: u32) {
            let buffer_size = (stride * height) as usize;

            // Resize pool and retained content if needed
            if width != self.width || height != self.height {
                self.width = width;
                self.height = height;
                if let Err(e) = self.pool.resize(buffer_size) {
                    tracing::error!("Failed to resize buffer pool: {}", e);
                    return;
                }
                // Resize retained content buffer
                self.retained_content.resize(buffer_size, 0);
            }

            // Create buffer and copy data
            let (buffer, canvas) = match self.pool.create_buffer(
                width as i32,
                height as i32,
                stride as i32,
                WlShmFormat::Argb8888,
            ) {
                Ok(result) => result,
                Err(e) => {
                    tracing::error!("Failed to create buffer for frame: {}", e);
                    return;
                }
            };

            // Copy frame data (BGRA format matches ARGB8888 on little-endian)
            let copy_len = canvas.len().min(data.len());
            canvas[..copy_len].copy_from_slice(&data[..copy_len]);

            // Update retained content for future partial updates
            let retain_len = self.retained_content.len().min(data.len());
            self.retained_content[..retain_len].copy_from_slice(&data[..retain_len]);

            // Attach and commit
            self.window
                .wl_surface()
                .attach(Some(buffer.wl_buffer()), 0, 0);
            self.window
                .wl_surface()
                .damage_buffer(0, 0, width as i32, height as i32);
            self.window.wl_surface().commit();

            self.buffer = Some(buffer);
            self.dirty = false;
        }

        /// Draws a partial frame update at the specified position.
        ///
        /// This preserves existing content outside the updated region by copying
        /// from retained_content before blitting the new partial data.
        fn draw_partial_frame(
            &mut self,
            data: &[u8],
            width: u32,
            height: u32,
            x: u32,
            y: u32,
            stride: u32,
        ) {
            let window_stride = self.width * 4;
            let frame_stride = stride;

            // Create a new buffer (we can't modify the attached one)
            let (buffer, canvas) = match self.pool.create_buffer(
                self.width as i32,
                self.height as i32,
                window_stride as i32,
                WlShmFormat::Argb8888,
            ) {
                Ok(result) => result,
                Err(e) => {
                    tracing::error!("Failed to create buffer for partial frame: {}", e);
                    return;
                }
            };

            // Copy retained content to preserve pixels outside the update region
            // This implements proper double-buffering for partial updates
            // TODO: Optimization - only copy affected rows instead of full buffer
            // to reduce memory bandwidth for small partial updates
            let copy_len = canvas.len().min(self.retained_content.len());
            if copy_len > 0 {
                canvas[..copy_len].copy_from_slice(&self.retained_content[..copy_len]);
            }

            // Blit the partial update into the buffer at (x, y)
            // Note: frame_stride may include padding, but we only copy width*4 actual pixels
            let copy_width = (width as usize).saturating_mul(4);

            for row in 0..height {
                let dest_y = y.saturating_add(row);
                if dest_y >= self.height {
                    break;
                }

                // Source uses frame_stride for row offset (handles padding)
                let src_start = (row as usize).saturating_mul(frame_stride as usize);
                // But we only copy width*4 bytes (actual pixel data, no padding)
                let src_end = src_start.saturating_add(copy_width);

                if src_end > data.len() {
                    break;
                }

                let dest_start = (dest_y as usize)
                    .saturating_mul(window_stride as usize)
                    .saturating_add((x as usize).saturating_mul(4));
                let dest_end = dest_start.saturating_add(copy_width);

                if dest_end <= canvas.len() && src_end <= data.len() {
                    canvas[dest_start..dest_end].copy_from_slice(&data[src_start..src_end]);
                    // Also update retained content for future partial updates
                    if dest_end <= self.retained_content.len() {
                        self.retained_content[dest_start..dest_end]
                            .copy_from_slice(&data[src_start..src_end]);
                    }
                }
            }

            // Attach and commit with damage only for the updated region
            self.window
                .wl_surface()
                .attach(Some(buffer.wl_buffer()), 0, 0);
            self.window
                .wl_surface()
                .damage_buffer(x as i32, y as i32, width as i32, height as i32);
            self.window.wl_surface().commit();

            self.buffer = Some(buffer);
            self.dirty = false;
        }
    }

    // Implement required traits for smithay-client-toolkit

    impl CompositorHandler for WaylandWindow {
        fn scale_factor_changed(
            &mut self,
            _conn: &Connection,
            _qh: &QueueHandle<Self>,
            _surface: &WlSurface,
            _new_factor: i32,
        ) {
            // Handle scale factor changes if needed
        }

        fn transform_changed(
            &mut self,
            _conn: &Connection,
            _qh: &QueueHandle<Self>,
            _surface: &WlSurface,
            _new_transform: smithay_client_toolkit::reexports::client::protocol::wl_output::Transform,
        ) {
            // Handle transform changes if needed
        }

        fn frame(
            &mut self,
            _conn: &Connection,
            _qh: &QueueHandle<Self>,
            _surface: &WlSurface,
            _time: u32,
        ) {
            self.dirty = true;
            let _ = self.event_tx.send(WindowEvent::RedrawRequested);
        }

        fn surface_enter(
            &mut self,
            _conn: &Connection,
            _qh: &QueueHandle<Self>,
            _surface: &WlSurface,
            _output: &WlOutput,
        ) {
        }

        fn surface_leave(
            &mut self,
            _conn: &Connection,
            _qh: &QueueHandle<Self>,
            _surface: &WlSurface,
            _output: &WlOutput,
        ) {
        }
    }

    impl OutputHandler for WaylandWindow {
        fn output_state(&mut self) -> &mut OutputState {
            &mut self.output_state
        }

        fn new_output(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, output: WlOutput) {
            // Story 3.1: Detect new monitor
            let id = output.id().protocol_id();
            tracing::debug!("New output detected: id={}", id);

            // Get output info from OutputState
            if let Some(info) = self.output_state.info(&output) {
                let monitor = Self::create_monitor_info(id, info);

                tracing::info!(
                    "Monitor detected: {} ({}x{} at {},{}, {:.1}Hz, scale={})",
                    monitor.name,
                    monitor.width,
                    monitor.height,
                    monitor.x,
                    monitor.y,
                    monitor.refresh_hz(),
                    monitor.scale
                );

                // Store both the WlOutput and MonitorInfo together to keep them in sync
                self.outputs.insert(id, output.clone());
                self.monitors.insert(id, monitor);
            } else {
                // Don't store output without info - wait for update_output
                tracing::warn!(
                    "New output {} has no info available yet, waiting for update",
                    id
                );
            }
        }

        fn update_output(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, output: WlOutput) {
            // Story 3.1: Update monitor info when properties change
            let id = output.id().protocol_id();

            if let Some(info) = self.output_state.info(&output) {
                let monitor = Self::create_monitor_info(id, info);
                let is_new = !self.monitors.contains_key(&id);

                if is_new {
                    tracing::info!(
                        "Monitor detected (via update): {} ({}x{} at {},{}, {:.1}Hz, scale={})",
                        monitor.name,
                        monitor.width,
                        monitor.height,
                        monitor.x,
                        monitor.y,
                        monitor.refresh_hz(),
                        monitor.scale
                    );
                    // Store the WlOutput now that we have info
                    self.outputs.insert(id, output.clone());
                } else {
                    tracing::debug!(
                        "Monitor updated: {} ({}x{} at {},{}, {:.1}Hz)",
                        monitor.name,
                        monitor.width,
                        monitor.height,
                        monitor.x,
                        monitor.y,
                        monitor.refresh_hz()
                    );
                }

                self.monitors.insert(id, monitor);
            }
        }

        fn output_destroyed(
            &mut self,
            _conn: &Connection,
            _qh: &QueueHandle<Self>,
            output: WlOutput,
        ) {
            // Story 3.1: Handle monitor disconnect (hot-unplug)
            let id = output.id().protocol_id();

            if let Some(monitor) = self.monitors.remove(&id) {
                tracing::info!("Monitor disconnected: {} ({})", monitor.name, id);
            }
            self.outputs.remove(&id);

            // Log remaining monitors
            if !self.monitors.is_empty() {
                tracing::debug!(
                    "Remaining monitors: {:?}",
                    self.monitors.keys().collect::<Vec<_>>()
                );
            }
        }
    }

    impl WindowHandler for WaylandWindow {
        fn request_close(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _window: &Window) {
            self.close_requested = true;
            let _ = self.event_tx.send(WindowEvent::CloseRequested);
        }

        fn configure(
            &mut self,
            _conn: &Connection,
            _qh: &QueueHandle<Self>,
            _window: &Window,
            configure: WindowConfigure,
            _serial: u32,
        ) {
            let (new_width, new_height) = configure.new_size;

            // Use suggested size or keep current
            let width = new_width.map(|w| w.get()).unwrap_or(self.width);
            let height = new_height.map(|h| h.get()).unwrap_or(self.height);

            if width != self.width || height != self.height {
                self.width = width;
                self.height = height;
                // Resize retained content buffer to match new window size
                let new_size = (width as usize) * (height as usize) * 4;
                self.retained_content.resize(new_size, 0);
                self.dirty = true;
                let _ = self.event_tx.send(WindowEvent::Resized { width, height });
                tracing::debug!("Window resized to {}x{}", width, height);
                // TODO: Notify RDP server of resolution change via DISPLAYCONTROL (Epic 3)
            }

            // Check fullscreen state change
            let new_fullscreen = configure.state.contains(WindowState::FULLSCREEN);
            if new_fullscreen != self.is_fullscreen {
                self.is_fullscreen = new_fullscreen;
                let _ = self.event_tx.send(WindowEvent::FullscreenChanged {
                    is_fullscreen: new_fullscreen,
                });
                tracing::info!(
                    "Fullscreen state changed: {}",
                    if new_fullscreen { "entered" } else { "exited" }
                );
            }
        }
    }

    impl ShmHandler for WaylandWindow {
        fn shm_state(&mut self) -> &mut Shm {
            &mut self.shm
        }
    }

    impl ProvidesRegistryState for WaylandWindow {
        fn registry(&mut self) -> &mut RegistryState {
            &mut self.registry_state
        }

        registry_handlers![OutputState, SeatState];
    }

    impl SeatHandler for WaylandWindow {
        fn seat_state(&mut self) -> &mut SeatState {
            &mut self.seat_state
        }

        fn new_seat(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _seat: WlSeat) {
            // Seat will be handled when capabilities are announced
        }

        fn new_capability(
            &mut self,
            _conn: &Connection,
            qh: &QueueHandle<Self>,
            seat: WlSeat,
            capability: Capability,
        ) {
            if capability == Capability::Keyboard && !self.has_keyboard {
                tracing::debug!("Keyboard capability available, requesting keyboard");
                if self.seat_state.get_keyboard(qh, &seat, None).is_ok() {
                    self.has_keyboard = true;
                }
            }
            if capability == Capability::Pointer && !self.has_pointer {
                tracing::debug!("Pointer capability available, requesting pointer");
                if self.seat_state.get_pointer(qh, &seat).is_ok() {
                    self.has_pointer = true;
                }
            }
        }

        fn remove_capability(
            &mut self,
            _conn: &Connection,
            _qh: &QueueHandle<Self>,
            _seat: WlSeat,
            capability: Capability,
        ) {
            if capability == Capability::Keyboard {
                tracing::debug!("Keyboard capability removed");
                self.has_keyboard = false;
            }
            if capability == Capability::Pointer {
                tracing::debug!("Pointer capability removed");
                self.has_pointer = false;
            }
        }

        fn remove_seat(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _seat: WlSeat) {
            // Nothing to clean up
        }
    }

    impl KeyboardHandler for WaylandWindow {
        fn enter(
            &mut self,
            _conn: &Connection,
            _qh: &QueueHandle<Self>,
            _keyboard: &WlKeyboard,
            _surface: &WlSurface,
            _serial: u32,
            _raw: &[u32],
            _keysyms: &[Keysym],
        ) {
            // Keyboard focus gained
            tracing::trace!("Keyboard focus entered");
        }

        fn leave(
            &mut self,
            _conn: &Connection,
            _qh: &QueueHandle<Self>,
            _keyboard: &WlKeyboard,
            _surface: &WlSurface,
            _serial: u32,
        ) {
            // Keyboard focus lost - reset modifiers
            tracing::trace!("Keyboard focus left");
            self.modifiers = Modifiers::default();
        }

        fn press_key(
            &mut self,
            _conn: &Connection,
            _qh: &QueueHandle<Self>,
            _keyboard: &WlKeyboard,
            _serial: u32,
            event: KeyEvent,
        ) {
            // Check for client shortcuts BEFORE forwarding to remote
            // Ctrl+Alt+Enter toggles fullscreen (Story 2.5)
            if self.modifiers.ctrl && self.modifiers.alt && event.keysym == Keysym::Return {
                tracing::debug!("Ctrl+Alt+Enter detected - toggling fullscreen");
                let _ = self.event_tx.send(WindowEvent::KeyboardShortcut(
                    KeyboardShortcut::ToggleFullscreen,
                ));
                return; // Don't forward client shortcuts to remote
            }

            // Ctrl+Alt+End disconnects (FR43, Story 2.8)
            if self.modifiers.ctrl && self.modifiers.alt && event.keysym == Keysym::End {
                tracing::debug!("Ctrl+Alt+End detected - requesting disconnect");
                let _ = self
                    .event_tx
                    .send(WindowEvent::KeyboardShortcut(KeyboardShortcut::Disconnect));
                return; // Don't forward client shortcuts to remote
            }

            // Story 2.9: International keyboard support
            // Use Unicode input for printable characters when no Ctrl modifier is held
            // This ensures correct character input for non-US layouts (AZERTY, QWERTZ, etc.)
            // Note: Grapheme clusters (emoji with modifiers, combining diacritics) may have
            // multiple chars - these fall back to scancode which may not work correctly.
            if !self.modifiers.ctrl
                && !self.modifiers.alt
                && let Some(ref utf8) = event.utf8
            {
                // Get the first character if it's a single printable character
                let chars: Vec<char> = utf8.chars().collect();
                if chars.len() == 1 && !chars[0].is_control() {
                    let character = chars[0];
                    tracing::trace!(
                        "Unicode key pressed: '{}' (U+{:04X})",
                        character,
                        character as u32
                    );
                    // Track this key as sent via Unicode for correct release handling
                    self.unicode_keys_pressed.insert(event.raw_code);
                    let _ = self
                        .event_tx
                        .send(WindowEvent::UnicodeKeyPressed { character });
                    return;
                }
            }

            // Fallback to scancode for special keys, modifiers, or when Unicode not available
            if let Some(scancode) = crate::input::wayland_to_rdp_scancode(event.raw_code) {
                tracing::trace!(
                    "Key pressed: evdev {} -> RDP 0x{:04X}",
                    event.raw_code,
                    scancode
                );
                let _ = self.event_tx.send(WindowEvent::KeyPressed { scancode });
            } else {
                tracing::trace!("Unknown key pressed: evdev {}", event.raw_code);
            }
        }

        fn release_key(
            &mut self,
            _conn: &Connection,
            _qh: &QueueHandle<Self>,
            _keyboard: &WlKeyboard,
            _serial: u32,
            event: KeyEvent,
        ) {
            // Story 2.9: Check if this key was pressed via Unicode
            // We must release via the same method to avoid stuck keys, even if
            // modifiers changed between press and release.
            if self.unicode_keys_pressed.remove(&event.raw_code) {
                // Key was pressed via Unicode - release via Unicode
                if let Some(ref utf8) = event.utf8 {
                    let chars: Vec<char> = utf8.chars().collect();
                    if chars.len() == 1 && !chars[0].is_control() {
                        let character = chars[0];
                        tracing::trace!(
                            "Unicode key released: '{}' (U+{:04X})",
                            character,
                            character as u32
                        );
                        let _ = self
                            .event_tx
                            .send(WindowEvent::UnicodeKeyReleased { character });
                        return;
                    }
                }
                // Fallback: utf8 not available on release, send the last known character
                // This shouldn't happen normally, but handle gracefully
                tracing::warn!(
                    "Unicode key release without utf8 for raw_code {}",
                    event.raw_code
                );
            }

            // Fallback to scancode release (key was pressed via scancode)
            if let Some(scancode) = crate::input::wayland_to_rdp_scancode(event.raw_code) {
                tracing::trace!(
                    "Key released: evdev {} -> RDP 0x{:04X}",
                    event.raw_code,
                    scancode
                );
                let _ = self.event_tx.send(WindowEvent::KeyReleased { scancode });
            }
        }

        fn update_modifiers(
            &mut self,
            _conn: &Connection,
            _qh: &QueueHandle<Self>,
            _keyboard: &WlKeyboard,
            _serial: u32,
            modifiers: Modifiers,
            _raw_modifiers: RawModifiers,
            layout: u32,
        ) {
            self.modifiers = modifiers;

            // Story 2.9: Log keyboard layout changes (AC 3)
            if layout != self.current_layout {
                tracing::debug!(
                    "Keyboard layout changed: {} -> {} (Unicode input handles this automatically)",
                    self.current_layout,
                    layout
                );
                self.current_layout = layout;
            }
        }

        fn repeat_key(
            &mut self,
            _conn: &Connection,
            _qh: &QueueHandle<Self>,
            _keyboard: &WlKeyboard,
            _serial: u32,
            event: KeyEvent,
        ) {
            // Story 2.9: Use Unicode for repeated character keys
            // Note: Key repeat sends only press events (no release between repeats).
            // The release comes from release_key when the user actually releases.
            // We don't need to update unicode_keys_pressed since the original
            // press_key already tracked it.
            if !self.modifiers.ctrl
                && !self.modifiers.alt
                && let Some(ref utf8) = event.utf8
            {
                let chars: Vec<char> = utf8.chars().collect();
                if chars.len() == 1 && !chars[0].is_control() {
                    let character = chars[0];
                    let _ = self
                        .event_tx
                        .send(WindowEvent::UnicodeKeyPressed { character });
                    return;
                }
            }

            // Fallback to scancode for repeated special keys
            if let Some(scancode) = crate::input::wayland_to_rdp_scancode(event.raw_code) {
                let _ = self.event_tx.send(WindowEvent::KeyPressed { scancode });
            }
        }
    }

    impl PointerHandler for WaylandWindow {
        fn pointer_frame(
            &mut self,
            _conn: &Connection,
            _qh: &QueueHandle<Self>,
            _pointer: &WlPointer,
            events: &[PointerEvent],
        ) {
            // Process all pointer events in this frame
            for event in events {
                // Update stored position from event
                self.pointer_position = event.position;

                match event.kind {
                    PointerEventKind::Enter { .. } => {
                        tracing::trace!(
                            "Pointer entered at ({:.1}, {:.1})",
                            event.position.0,
                            event.position.1
                        );
                    }
                    PointerEventKind::Leave { .. } => {
                        tracing::trace!("Pointer left surface");
                        // Release any pressed buttons to prevent stuck buttons on remote
                        let (x, y) = self.pointer_position;
                        for button in self.pressed_buttons.drain(..) {
                            tracing::trace!("Releasing button {} due to pointer leave", button);
                            let _ = self.event_tx.send(WindowEvent::MouseButton {
                                button,
                                pressed: false,
                                x,
                                y,
                            });
                        }
                    }
                    PointerEventKind::Motion { .. } => {
                        let (x, y) = event.position;
                        let _ = self.event_tx.send(WindowEvent::MouseMove { x, y });
                    }
                    PointerEventKind::Press { button, .. } => {
                        let (x, y) = event.position;
                        tracing::trace!("Mouse button {} pressed at ({:.1}, {:.1})", button, x, y);
                        // Track pressed button for release on Leave
                        if !self.pressed_buttons.contains(&button) {
                            self.pressed_buttons.push(button);
                        }
                        let _ = self.event_tx.send(WindowEvent::MouseButton {
                            button,
                            pressed: true,
                            x,
                            y,
                        });
                    }
                    PointerEventKind::Release { button, .. } => {
                        let (x, y) = event.position;
                        tracing::trace!("Mouse button {} released at ({:.1}, {:.1})", button, x, y);
                        // Remove from tracked pressed buttons
                        self.pressed_buttons.retain(|&b| b != button);
                        let _ = self.event_tx.send(WindowEvent::MouseButton {
                            button,
                            pressed: false,
                            x,
                            y,
                        });
                    }
                    PointerEventKind::Axis {
                        horizontal,
                        vertical,
                        ..
                    } => {
                        let (x, y) = event.position;
                        // Handle vertical scroll (most common)
                        if vertical.discrete != 0 {
                            // Discrete scroll (wheel notches)
                            let _ = self.event_tx.send(WindowEvent::MouseAxis {
                                horizontal: false,
                                value: vertical.discrete as f64,
                                x,
                                y,
                            });
                        } else if vertical.absolute != 0.0 {
                            // Continuous scroll (touchpad)
                            let _ = self.event_tx.send(WindowEvent::MouseAxis {
                                horizontal: false,
                                value: vertical.absolute,
                                x,
                                y,
                            });
                        }
                        // Handle horizontal scroll
                        if horizontal.discrete != 0 {
                            let _ = self.event_tx.send(WindowEvent::MouseAxis {
                                horizontal: true,
                                value: horizontal.discrete as f64,
                                x,
                                y,
                            });
                        } else if horizontal.absolute != 0.0 {
                            let _ = self.event_tx.send(WindowEvent::MouseAxis {
                                horizontal: true,
                                value: horizontal.absolute,
                                x,
                                y,
                            });
                        }
                    }
                }
            }
        }
    }

    delegate_compositor!(WaylandWindow);
    delegate_output!(WaylandWindow);
    delegate_seat!(WaylandWindow);
    delegate_keyboard!(WaylandWindow);
    delegate_pointer!(WaylandWindow);
    delegate_shm!(WaylandWindow);
    delegate_xdg_shell!(WaylandWindow);
    delegate_xdg_window!(WaylandWindow);
    delegate_registry!(WaylandWindow);
}

#[cfg(target_os = "linux")]
pub use linux::*;

// Stub implementation for non-Linux platforms (for compilation only)
#[cfg(not(target_os = "linux"))]
mod stub {
    /// Stub MonitorInfo for non-Linux platforms.
    #[derive(Debug, Clone)]
    pub struct MonitorInfo {
        pub id: u32,
        pub name: String,
        pub make: Option<String>,
        pub model: Option<String>,
        pub width: u32,
        pub height: u32,
        pub x: i32,
        pub y: i32,
        pub refresh_mhz: u32,
        pub scale: i32,
    }

    impl MonitorInfo {
        pub fn refresh_hz(&self) -> f64 {
            self.refresh_mhz as f64 / 1000.0
        }
    }

    /// Stub WindowEvent for non-Linux platforms.
    #[derive(Debug)]
    pub enum WindowEvent {
        CloseRequested,
        Resized {
            width: u32,
            height: u32,
        },
        RedrawRequested,
        FullscreenChanged {
            is_fullscreen: bool,
        },
        KeyboardShortcut(KeyboardShortcut),
        KeyPressed {
            scancode: u16,
        },
        KeyReleased {
            scancode: u16,
        },
        UnicodeKeyPressed {
            character: char,
        },
        UnicodeKeyReleased {
            character: char,
        },
        MouseMove {
            x: f64,
            y: f64,
        },
        MouseButton {
            button: u32,
            pressed: bool,
            x: f64,
            y: f64,
        },
        MouseAxis {
            horizontal: bool,
            value: f64,
            x: f64,
            y: f64,
        },
    }

    /// Keyboard shortcuts that the window can detect.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum KeyboardShortcut {
        /// Toggle fullscreen mode (Ctrl+Alt+Enter).
        ToggleFullscreen,
        /// Disconnect from remote session (Ctrl+Alt+End).
        Disconnect,
    }

    /// Stub WindowConfig for non-Linux platforms.
    #[derive(Debug, Clone)]
    pub struct WindowConfig {
        pub title: String,
        pub width: u32,
        pub height: u32,
        pub fullscreen: bool,
    }

    impl Default for WindowConfig {
        fn default() -> Self {
            Self {
                title: "YARD".to_string(),
                width: 1280,
                height: 720,
                fullscreen: false,
            }
        }
    }

    impl WindowConfig {
        pub fn with_connection_info(host: &str, port: u16, username: Option<&str>) -> Self {
            let title = match username {
                Some(user) => format!("YARD - {}:{} [{}]", host, port, user),
                None => format!("YARD - {}:{}", host, port),
            };
            Self {
                title,
                width: 1280,
                height: 720,
                fullscreen: false,
            }
        }

        /// Sets the window size.
        #[must_use]
        pub fn with_size(mut self, width: u32, height: u32) -> Self {
            self.width = width;
            self.height = height;
            self
        }

        /// Sets fullscreen mode.
        #[must_use]
        pub fn with_fullscreen(mut self, fullscreen: bool) -> Self {
            self.fullscreen = fullscreen;
            self
        }
    }

    /// Stub WaylandWindow for non-Linux platforms.
    pub struct WaylandWindow;

    impl WaylandWindow {
        pub fn new(_config: WindowConfig) -> Result<((), Self, ()), Box<dyn std::error::Error>> {
            Err("Wayland is only supported on Linux".into())
        }

        pub fn close_requested(&self) -> bool {
            false
        }

        pub fn dimensions(&self) -> (u32, u32) {
            (0, 0)
        }

        pub fn is_fullscreen(&self) -> bool {
            false
        }

        pub fn set_fullscreen(&self, _output: Option<&()>) {}

        pub fn unset_fullscreen(&self) {}

        pub fn toggle_fullscreen(&self) {}

        pub fn set_remote_resolution(&mut self, _width: u32, _height: u32) {}

        pub fn remote_resolution(&self) -> (u32, u32) {
            (0, 0)
        }

        pub fn draw_solid(&mut self, _r: u8, _g: u8, _b: u8) {}

        pub fn draw_frame(&mut self, _data: &[u8], _width: u32, _height: u32) {}

        pub fn draw_frame_at(&mut self, _data: &[u8], _width: u32, _height: u32, _x: u32, _y: u32) {
        }

        pub fn draw_frame_at_with_stride(
            &mut self,
            _data: &[u8],
            _width: u32,
            _height: u32,
            _x: u32,
            _y: u32,
            _stride: u32,
        ) {
        }

        pub fn get_monitors(&self) -> Vec<MonitorInfo> {
            Vec::new()
        }

        pub fn monitor_count(&self) -> usize {
            0
        }

        pub fn get_output(&self, _id: u32) -> Option<&()> {
            None
        }

        pub fn primary_monitor(&self) -> Option<MonitorInfo> {
            None
        }
    }
}

#[cfg(not(target_os = "linux"))]
pub use stub::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_window_config_default() {
        let config = WindowConfig::default();
        assert_eq!(config.title, "YARD");
        assert_eq!(config.width, 1280);
        assert_eq!(config.height, 720);
    }

    #[test]
    fn test_window_config_with_connection_info_no_username() {
        let config = WindowConfig::with_connection_info("server.example.com", 3389, None);
        assert_eq!(config.title, "YARD - server.example.com:3389");
        assert_eq!(config.width, 1280);
        assert_eq!(config.height, 720);
    }

    #[test]
    fn test_window_config_with_connection_info_with_username() {
        let config = WindowConfig::with_connection_info("server.example.com", 3389, Some("john"));
        assert_eq!(config.title, "YARD - server.example.com:3389 [john]");
    }

    #[test]
    fn test_window_config_with_custom_port() {
        let config = WindowConfig::with_connection_info("10.0.0.1", 13389, Some("admin"));
        assert_eq!(config.title, "YARD - 10.0.0.1:13389 [admin]");
    }

    #[test]
    fn test_window_config_with_unicode_username() {
        let config = WindowConfig::with_connection_info("server", 3389, Some("用户"));
        assert_eq!(config.title, "YARD - server:3389 [用户]");
    }

    #[test]
    fn test_window_config_with_size() {
        let config = WindowConfig::default().with_size(1920, 1080);
        assert_eq!(config.width, 1920);
        assert_eq!(config.height, 1080);
    }

    #[test]
    fn test_window_config_with_size_chained() {
        let config =
            WindowConfig::with_connection_info("host", 3389, Some("user")).with_size(2560, 1440);
        assert_eq!(config.title, "YARD - host:3389 [user]");
        assert_eq!(config.width, 2560);
        assert_eq!(config.height, 1440);
    }

    #[test]
    fn test_window_config_fullscreen_default() {
        let config = WindowConfig::default();
        assert!(!config.fullscreen);
    }

    #[test]
    fn test_window_config_with_fullscreen() {
        let config = WindowConfig::default().with_fullscreen(true);
        assert!(config.fullscreen);
    }

    #[test]
    fn test_window_config_with_fullscreen_chained() {
        let config = WindowConfig::with_connection_info("host", 3389, Some("user"))
            .with_size(1920, 1080)
            .with_fullscreen(true);
        assert_eq!(config.title, "YARD - host:3389 [user]");
        assert_eq!(config.width, 1920);
        assert_eq!(config.height, 1080);
        assert!(config.fullscreen);
    }

    // Story 3.1: Monitor detection tests
    #[test]
    fn test_monitor_info_refresh_hz() {
        let monitor = MonitorInfo {
            id: 1,
            name: "DP-1".to_string(),
            make: Some("Dell".to_string()),
            model: Some("U2723QE".to_string()),
            width: 3840,
            height: 2160,
            x: 0,
            y: 0,
            refresh_mhz: 60000,
            scale: 2,
        };
        assert!((monitor.refresh_hz() - 60.0).abs() < 0.001);
    }

    #[test]
    fn test_monitor_info_refresh_hz_144() {
        let monitor = MonitorInfo {
            id: 2,
            name: "HDMI-A-1".to_string(),
            make: None,
            model: None,
            width: 1920,
            height: 1080,
            x: 3840,
            y: 0,
            refresh_mhz: 144000,
            scale: 1,
        };
        assert!((monitor.refresh_hz() - 144.0).abs() < 0.001);
    }

    #[test]
    fn test_monitor_info_clone() {
        let monitor = MonitorInfo {
            id: 1,
            name: "DP-1".to_string(),
            make: Some("Dell".to_string()),
            model: Some("U2723QE".to_string()),
            width: 3840,
            height: 2160,
            x: 0,
            y: 0,
            refresh_mhz: 60000,
            scale: 2,
        };
        let cloned = monitor.clone();
        assert_eq!(cloned.id, monitor.id);
        assert_eq!(cloned.name, monitor.name);
        assert_eq!(cloned.width, monitor.width);
        assert_eq!(cloned.height, monitor.height);
    }
}
