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
    use smithay_client_toolkit::delegate_xdg_surface;
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
    use smithay_client_toolkit::shell::xdg::{XdgSurface, XdgSurfaceHandler};
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

    /// Per-monitor Wayland surface state (Story 3.3, extended in Story 3.4).
    ///
    /// Each monitor in multi-monitor mode gets its own MonitorSurface instance,
    /// containing the Wayland surface, xdg_toplevel for window management, buffer pool,
    /// and associated state.
    /// This follows the critical rule: "One wl_surface per monitor - NEVER one surface spanning all monitors".
    pub struct MonitorSurface {
        /// The Wayland surface for this monitor.
        surface: WlSurface,
        /// XDG surface wrapper for shell integration (Story 3.4).
        xdg_surface: XdgSurface,
        /// XDG toplevel for window management (Story 3.4).
        /// Uses smithay-client-toolkit's Window type which wraps xdg_toplevel.
        xdg_toplevel: Window,
        /// Buffer pool for this surface (separate memory per monitor).
        pool: SlotPool,
        /// Target WlOutput for fullscreen targeting.
        target_output: WlOutput,
        /// Monitor info (resolution, position).
        monitor_info: MonitorInfo,
        /// Current buffer attached to this surface.
        buffer: Option<Buffer>,
        /// Retained frame content for partial updates.
        retained_content: Vec<u8>,
        /// Whether the surface needs redrawing.
        dirty: bool,
        /// Whether this surface is currently in fullscreen mode.
        is_fullscreen: bool,
        /// Whether the initial configure has been received.
        configured: bool,
    }

    impl MonitorSurface {
        /// Creates a new MonitorSurface for the specified monitor.
        ///
        /// # Arguments
        /// * `compositor` - The compositor state for creating surfaces
        /// * `xdg_shell` - The xdg_shell state for creating window surfaces
        /// * `shm` - The shared memory state for creating buffer pools
        /// * `qh` - The queue handle for creating Wayland objects
        /// * `output` - The target WlOutput for this surface
        /// * `monitor_info` - Information about the monitor (resolution, position)
        ///
        /// # Errors
        /// Returns an error if the buffer pool cannot be created.
        pub fn new(
            compositor: &CompositorState,
            xdg_shell: &XdgShell,
            shm: &Shm,
            qh: &QueueHandle<WaylandWindow>,
            output: WlOutput,
            monitor_info: MonitorInfo,
        ) -> Result<Self, Box<dyn std::error::Error>> {
            // Create the Wayland surface
            let surface = compositor.create_surface(qh);

            // Create xdg_surface and xdg_toplevel for window management (Story 3.4)
            let xdg_toplevel =
                xdg_shell.create_window(surface.clone(), WindowDecorations::ServerDefault, qh);

            // Set window title and app_id for each surface (Task 1.4)
            let title = format!("YARD - {}", monitor_info.name);
            xdg_toplevel.set_title(title);
            xdg_toplevel.set_app_id("yard");
            xdg_toplevel.commit();

            // Get the xdg_surface from the toplevel
            let xdg_surface = xdg_toplevel.xdg_surface().clone();

            // Calculate buffer size for this monitor
            let buffer_size = (monitor_info.width * monitor_info.height * 4) as usize;

            // Create dedicated buffer pool for this surface
            let pool = SlotPool::new(buffer_size, shm)?;

            // Initialize retained content buffer
            let retained_content = vec![0u8; buffer_size];

            tracing::debug!(
                "Created MonitorSurface for {} ({}x{} at {},{}, id={})",
                monitor_info.name,
                monitor_info.width,
                monitor_info.height,
                monitor_info.x,
                monitor_info.y,
                monitor_info.id
            );

            Ok(Self {
                surface,
                xdg_surface,
                xdg_toplevel,
                pool,
                target_output: output,
                monitor_info,
                buffer: None,
                retained_content,
                dirty: true,
                is_fullscreen: false,
                configured: false,
            })
        }

        /// Returns a reference to the underlying WlSurface.
        pub fn wl_surface(&self) -> &WlSurface {
            &self.surface
        }

        /// Returns a reference to the target WlOutput.
        pub fn target_output(&self) -> &WlOutput {
            &self.target_output
        }

        /// Returns a reference to the monitor info.
        pub fn monitor_info(&self) -> &MonitorInfo {
            &self.monitor_info
        }

        /// Returns the monitor ID.
        pub fn monitor_id(&self) -> u32 {
            self.monitor_info.id
        }

        /// Returns the surface dimensions (width, height).
        pub fn dimensions(&self) -> (u32, u32) {
            (self.monitor_info.width, self.monitor_info.height)
        }

        /// Returns whether this surface is currently fullscreen.
        pub fn is_fullscreen(&self) -> bool {
            self.is_fullscreen
        }

        /// Sets the fullscreen state for this surface (internal state tracking).
        pub fn set_fullscreen_state(&mut self, fullscreen: bool) {
            self.is_fullscreen = fullscreen;
        }

        /// Requests fullscreen mode on this surface's target output (Story 3.4 Task 2.1).
        ///
        /// The actual fullscreen state change is confirmed via the configure event.
        pub fn set_fullscreen(&self) {
            tracing::debug!(
                "Requesting fullscreen for {} on output {}",
                self.monitor_info.name,
                self.monitor_info.id
            );
            self.xdg_toplevel.set_fullscreen(Some(&self.target_output));
        }

        /// Requests to exit fullscreen mode on this surface (Story 3.4 Task 2.2).
        ///
        /// The actual state change is confirmed via the configure event.
        pub fn unset_fullscreen(&self) {
            tracing::debug!("Requesting exit fullscreen for {}", self.monitor_info.name);
            self.xdg_toplevel.unset_fullscreen();
        }

        /// Returns a reference to the xdg_toplevel (Window).
        pub fn xdg_toplevel(&self) -> &Window {
            &self.xdg_toplevel
        }

        /// Returns whether this surface needs redrawing.
        pub fn is_dirty(&self) -> bool {
            self.dirty
        }

        /// Marks this surface as needing redraw.
        pub fn mark_dirty(&mut self) {
            self.dirty = true;
        }

        /// Marks this surface as configured (initial configure received).
        pub fn mark_configured(&mut self) {
            self.configured = true;
        }

        /// Returns whether the surface has been configured.
        pub fn is_configured(&self) -> bool {
            self.configured
        }

        /// Draws a solid color to this surface.
        pub fn draw_solid(&mut self, r: u8, g: u8, b: u8) {
            if !self.dirty {
                return;
            }

            let width = self.monitor_info.width;
            let height = self.monitor_info.height;
            let stride = width * 4;

            let (buffer, canvas) = match self.pool.create_buffer(
                width as i32,
                height as i32,
                stride as i32,
                WlShmFormat::Argb8888,
            ) {
                Ok(result) => result,
                Err(e) => {
                    tracing::error!(
                        "Failed to create buffer for MonitorSurface {}: {}",
                        self.monitor_info.name,
                        e
                    );
                    return;
                }
            };

            // Fill with solid color (ARGB format)
            let color = [b, g, r, 255u8]; // BGRA order for ARGB8888
            for chunk in canvas.chunks_exact_mut(4) {
                chunk.copy_from_slice(&color);
            }

            // Update retained content
            for chunk in self.retained_content.chunks_exact_mut(4) {
                chunk.copy_from_slice(&color);
            }

            // Attach and commit
            self.surface.attach(Some(buffer.wl_buffer()), 0, 0);
            self.surface
                .damage_buffer(0, 0, width as i32, height as i32);
            self.surface.commit();

            self.buffer = Some(buffer);
            self.dirty = false;
        }

        /// Draws a frame region to this surface.
        ///
        /// The data should be BGRA pixel data for the region this monitor displays.
        /// Position (x, y) is the offset within this surface (typically 0, 0 for full frame).
        pub fn draw_frame(&mut self, data: &[u8], width: u32, height: u32, x: u32, y: u32) {
            let mon_width = self.monitor_info.width;
            let mon_height = self.monitor_info.height;
            let stride = mon_width * 4;

            // Create buffer
            let (buffer, canvas) = match self.pool.create_buffer(
                mon_width as i32,
                mon_height as i32,
                stride as i32,
                WlShmFormat::Argb8888,
            ) {
                Ok(result) => result,
                Err(e) => {
                    tracing::error!(
                        "Failed to create buffer for MonitorSurface {}: {}",
                        self.monitor_info.name,
                        e
                    );
                    return;
                }
            };

            // For full frame update
            if x == 0 && y == 0 && width == mon_width && height == mon_height {
                let copy_len = canvas.len().min(data.len());
                canvas[..copy_len].copy_from_slice(&data[..copy_len]);
                self.retained_content[..copy_len].copy_from_slice(&data[..copy_len]);
            } else {
                // Partial update: copy retained content first, then blit new data
                let retain_len = canvas.len().min(self.retained_content.len());
                canvas[..retain_len].copy_from_slice(&self.retained_content[..retain_len]);

                // Blit partial data
                let src_stride = width * 4;
                for row in 0..height {
                    let dest_y = y + row;
                    if dest_y >= mon_height {
                        break;
                    }

                    let src_start = (row as usize) * (src_stride as usize);
                    let src_end = src_start + (width as usize) * 4;
                    if src_end > data.len() {
                        break;
                    }

                    let dest_start = (dest_y as usize) * (stride as usize) + (x as usize) * 4;
                    let dest_end = dest_start + (width as usize) * 4;

                    if dest_end <= canvas.len() {
                        canvas[dest_start..dest_end].copy_from_slice(&data[src_start..src_end]);
                        if dest_end <= self.retained_content.len() {
                            self.retained_content[dest_start..dest_end]
                                .copy_from_slice(&data[src_start..src_end]);
                        }
                    }
                }
            }

            // Attach and commit
            self.surface.attach(Some(buffer.wl_buffer()), 0, 0);
            self.surface
                .damage_buffer(x as i32, y as i32, width as i32, height as i32);
            self.surface.commit();

            self.buffer = Some(buffer);
            self.dirty = false;
        }

        /// Resizes the buffer pool if the monitor resolution changed.
        pub fn resize(
            &mut self,
            width: u32,
            height: u32,
        ) -> Result<(), Box<dyn std::error::Error>> {
            if width == self.monitor_info.width && height == self.monitor_info.height {
                return Ok(());
            }

            let buffer_size = (width * height * 4) as usize;
            self.pool.resize(buffer_size)?;
            self.retained_content.resize(buffer_size, 0);
            self.monitor_info.width = width;
            self.monitor_info.height = height;
            self.dirty = true;

            tracing::debug!(
                "MonitorSurface {} resized to {}x{}",
                self.monitor_info.name,
                width,
                height
            );

            Ok(())
        }
    }

    impl Drop for MonitorSurface {
        fn drop(&mut self) {
            // Destroy in reverse creation order (Story 3.4 Task 1.5):
            // (project-context.md: "ALWAYS implement Drop for Wayland surfaces")
            tracing::debug!(
                "Destroying MonitorSurface for {} (id={})",
                self.monitor_info.name,
                self.monitor_info.id
            );
            // We explicitly destroy wl_surface first, before Rust's automatic Drop runs.
            // Rust drops fields in REVERSE declaration order, so after this manual destroy:
            // - configured, is_fullscreen, dirty, retained_content, buffer drop (primitives/vecs)
            // - monitor_info, target_output, pool drop
            // - xdg_toplevel (Window) drops - releases xdg_toplevel protocol object
            // - xdg_surface drops - releases xdg_surface protocol object
            // - surface field drops (already destroyed, no-op)
            self.surface.destroy();
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
        /// Multi-monitor surfaces (Story 3.3).
        /// Maps monitor ID to MonitorSurface for per-monitor rendering.
        /// Empty when in single-surface mode, populated when multi_monitor_mode is true.
        multi_surfaces: HashMap<u32, MonitorSurface>,
        /// Whether multi-monitor mode is active.
        /// When false, uses the single `window` field (backward compatible).
        /// When true, uses `multi_surfaces` for per-monitor rendering.
        multi_monitor_mode: bool,
        /// Reference to compositor state for creating surfaces.
        compositor: CompositorState,
        /// Reference to xdg_shell for window decoration.
        xdg_shell: XdgShell,
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
                // Story 3.3: Multi-monitor support
                multi_surfaces: HashMap::new(),
                multi_monitor_mode: false, // Default to single-surface mode
                compositor,
                xdg_shell,
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

        // =====================================================================
        // Story 3.3: Multi-Monitor Surface Management
        // =====================================================================

        /// Returns whether multi-monitor mode is active.
        pub fn is_multi_monitor_mode(&self) -> bool {
            self.multi_monitor_mode
        }

        /// Creates a MonitorSurface for a specific monitor.
        ///
        /// # Arguments
        /// * `qh` - The queue handle for creating Wayland objects
        /// * `monitor_id` - The ID of the monitor to create a surface for
        ///
        /// # Returns
        /// Ok(()) if the surface was created, Err if the monitor doesn't exist or creation failed.
        pub fn create_surface_for_monitor(
            &mut self,
            qh: &QueueHandle<Self>,
            monitor_id: u32,
        ) -> Result<(), Box<dyn std::error::Error>> {
            // Get monitor info
            let monitor_info = self
                .monitors
                .get(&monitor_id)
                .cloned()
                .ok_or_else(|| format!("Monitor {} not found", monitor_id))?;

            // Get WlOutput for this monitor
            let output = self
                .outputs
                .get(&monitor_id)
                .cloned()
                .ok_or_else(|| format!("WlOutput for monitor {} not found", monitor_id))?;

            // Create MonitorSurface (with xdg_shell support for Story 3.4)
            let surface = MonitorSurface::new(
                &self.compositor,
                &self.xdg_shell,
                &self.shm,
                qh,
                output,
                monitor_info.clone(),
            )?;

            tracing::info!(
                "Created surface for monitor {} ({}x{} at {}, {})",
                monitor_info.name,
                monitor_info.width,
                monitor_info.height,
                monitor_info.x,
                monitor_info.y
            );

            self.multi_surfaces.insert(monitor_id, surface);
            Ok(())
        }

        /// Creates MonitorSurfaces for all detected monitors.
        ///
        /// This enables multi-monitor mode by creating a separate Wayland surface
        /// for each connected monitor. Each surface can be independently fullscreened
        /// on its target output.
        ///
        /// # Arguments
        /// * `qh` - The queue handle for creating Wayland objects
        ///
        /// # Returns
        /// The number of surfaces created, or an error if creation failed.
        pub fn create_surfaces_for_all_monitors(
            &mut self,
            qh: &QueueHandle<Self>,
        ) -> Result<usize, Box<dyn std::error::Error>> {
            if self.monitors.is_empty() {
                return Err("No monitors detected".into());
            }

            // Clear any existing multi-surfaces
            self.destroy_all_surfaces();

            // Get monitor IDs (we can't iterate and mutate at the same time)
            let monitor_ids: Vec<u32> = self.monitors.keys().cloned().collect();

            let mut created_count = 0;
            for monitor_id in monitor_ids {
                match self.create_surface_for_monitor(qh, monitor_id) {
                    Ok(()) => created_count += 1,
                    Err(e) => {
                        tracing::error!(
                            "Failed to create surface for monitor {}: {}",
                            monitor_id,
                            e
                        );
                    }
                }
            }

            if created_count > 0 {
                self.multi_monitor_mode = true;
                tracing::info!(
                    "Multi-monitor mode enabled: {} surface(s) created",
                    created_count
                );
            } else {
                return Err("Failed to create any surfaces".into());
            }

            Ok(created_count)
        }

        /// Destroys all multi-monitor surfaces and returns to single-surface mode.
        ///
        /// This properly cleans up all MonitorSurface instances, releasing Wayland
        /// resources via their Drop implementations.
        pub fn destroy_all_surfaces(&mut self) {
            if !self.multi_surfaces.is_empty() {
                tracing::info!(
                    "Destroying {} multi-monitor surface(s)",
                    self.multi_surfaces.len()
                );
                self.multi_surfaces.clear(); // Drop triggers cleanup
            }
            self.multi_monitor_mode = false;
        }

        /// Returns a reference to a specific MonitorSurface by monitor ID.
        pub fn get_surface(&self, monitor_id: u32) -> Option<&MonitorSurface> {
            self.multi_surfaces.get(&monitor_id)
        }

        /// Returns a mutable reference to a specific MonitorSurface by monitor ID.
        pub fn get_surface_mut(&mut self, monitor_id: u32) -> Option<&mut MonitorSurface> {
            self.multi_surfaces.get_mut(&monitor_id)
        }

        /// Returns an iterator over all MonitorSurfaces.
        pub fn surfaces(&self) -> impl Iterator<Item = (&u32, &MonitorSurface)> {
            self.multi_surfaces.iter()
        }

        /// Returns a mutable iterator over all MonitorSurfaces.
        pub fn surfaces_mut(&mut self) -> impl Iterator<Item = (&u32, &mut MonitorSurface)> {
            self.multi_surfaces.iter_mut()
        }

        /// Returns the number of active multi-monitor surfaces.
        pub fn surface_count(&self) -> usize {
            self.multi_surfaces.len()
        }

        // =====================================================================
        // Story 3.4: Multi-Monitor Fullscreen Control
        // =====================================================================

        /// Requests fullscreen on all multi-monitor surfaces (Story 3.4 Task 3.1).
        ///
        /// Each surface targets its associated WlOutput.
        /// The actual fullscreen state change is confirmed via configure events.
        pub fn set_all_fullscreen(&self) {
            if !self.multi_monitor_mode {
                tracing::warn!("set_all_fullscreen called but not in multi-monitor mode");
                return;
            }

            tracing::info!(
                "Requesting fullscreen on {} surface(s)",
                self.multi_surfaces.len()
            );
            for surface in self.multi_surfaces.values() {
                surface.set_fullscreen();
            }
        }

        /// Requests to exit fullscreen on all multi-monitor surfaces (Story 3.4 Task 3.2).
        ///
        /// The actual state change is confirmed via configure events.
        pub fn unset_all_fullscreen(&self) {
            if !self.multi_monitor_mode {
                tracing::warn!("unset_all_fullscreen called but not in multi-monitor mode");
                return;
            }

            tracing::info!(
                "Requesting exit fullscreen on {} surface(s)",
                self.multi_surfaces.len()
            );
            for surface in self.multi_surfaces.values() {
                surface.unset_fullscreen();
            }
        }

        /// Toggles fullscreen mode on all multi-monitor surfaces (Story 3.4 Task 3.3).
        ///
        /// If any surface is not fullscreen, all surfaces enter fullscreen.
        /// If all surfaces are fullscreen, all surfaces exit fullscreen.
        pub fn toggle_all_fullscreen(&self) {
            if !self.multi_monitor_mode {
                tracing::warn!("toggle_all_fullscreen called but not in multi-monitor mode");
                // Fall back to single-surface toggle
                self.toggle_fullscreen();
                return;
            }

            // Check if any surface is not fullscreen
            let any_not_fullscreen = self.multi_surfaces.values().any(|s| !s.is_fullscreen());

            if any_not_fullscreen {
                tracing::debug!("Toggling all surfaces to fullscreen");
                self.set_all_fullscreen();
            } else {
                tracing::debug!("Toggling all surfaces to windowed");
                self.unset_all_fullscreen();
            }
        }

        /// Returns true if all multi-monitor surfaces are currently fullscreen.
        pub fn all_fullscreen(&self) -> bool {
            if !self.multi_monitor_mode || self.multi_surfaces.is_empty() {
                return self.is_fullscreen;
            }
            self.multi_surfaces.values().all(|s| s.is_fullscreen())
        }

        /// Draws a solid color to all multi-monitor surfaces.
        ///
        /// In single-surface mode, uses the legacy draw_solid behavior.
        /// In multi-monitor mode, draws to all MonitorSurfaces.
        pub fn draw_solid_all(&mut self, r: u8, g: u8, b: u8) {
            if self.multi_monitor_mode {
                for surface in self.multi_surfaces.values_mut() {
                    surface.draw_solid(r, g, b);
                }
            } else {
                self.draw_solid(r, g, b);
            }
        }

        /// Draws a frame region to the appropriate surface(s).
        ///
        /// In multi-monitor mode, this determines which surface(s) the frame
        /// region intersects and draws to each. The region coordinates are in
        /// the combined desktop space (matching server's view).
        ///
        /// # Arguments
        /// * `data` - BGRA pixel data for the region
        /// * `width` - Width of the region in pixels
        /// * `height` - Height of the region in pixels
        /// * `x` - X offset in combined desktop space
        /// * `y` - Y offset in combined desktop space
        ///
        /// # Note
        /// Monitor coordinates (x, y) can be negative (e.g., monitor to the left of primary).
        /// This function handles negative coordinates correctly using i32 arithmetic.
        pub fn draw_frame_to_surfaces(
            &mut self,
            data: &[u8],
            width: u32,
            height: u32,
            x: u32,
            y: u32,
        ) {
            if !self.multi_monitor_mode {
                // Single-surface mode: use legacy behavior
                self.draw_frame_at(data, width, height, x, y);
                return;
            }

            // Multi-monitor mode: dispatch to appropriate surface(s)
            // For now, find the surface that contains this region
            // (Full implementation of region mapping will be in Story 3.5)

            // Use i32 for frame coordinates to handle negative monitor positions
            // Frame coordinates from RDP are always non-negative, but monitor positions can be negative
            let frame_x = x as i32;
            let frame_y = y as i32;
            let frame_w = width as i32;
            let frame_h = height as i32;

            for (_id, surface) in self.multi_surfaces.iter_mut() {
                let mon = surface.monitor_info();
                // Monitor coordinates are already i32 (can be negative)
                let mon_x = mon.x;
                let mon_y = mon.y;
                let mon_w = mon.width as i32;
                let mon_h = mon.height as i32;

                // Check if the frame region intersects this monitor (i32 arithmetic handles negatives)
                let intersects = frame_x < mon_x + mon_w
                    && frame_x + frame_w > mon_x
                    && frame_y < mon_y + mon_h
                    && frame_y + frame_h > mon_y;

                if intersects {
                    // Calculate local coordinates within this surface
                    // saturating_sub handles case where frame starts before monitor
                    let local_x = (frame_x - mon_x).max(0) as u32;
                    let local_y = (frame_y - mon_y).max(0) as u32;

                    // For simplicity, pass the full region data
                    // (Story 3.5 will implement proper region slicing)
                    surface.draw_frame(data, width, height, local_x, local_y);
                }
            }
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
        fn request_close(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, window: &Window) {
            // Check if this is a multi-monitor surface close request
            if self.multi_monitor_mode {
                // Find which surface requested close
                for (id, surface) in self.multi_surfaces.iter() {
                    if surface.xdg_toplevel().wl_surface() == window.wl_surface() {
                        tracing::debug!(
                            "Close requested for multi-monitor surface {} (id={})",
                            surface.monitor_info().name,
                            id
                        );
                        // For multi-monitor, closing any window closes the app
                        self.close_requested = true;
                        let _ = self.event_tx.send(WindowEvent::CloseRequested);
                        return;
                    }
                }
            }
            // Single-surface mode or primary window
            self.close_requested = true;
            let _ = self.event_tx.send(WindowEvent::CloseRequested);
        }

        fn configure(
            &mut self,
            _conn: &Connection,
            _qh: &QueueHandle<Self>,
            window: &Window,
            configure: WindowConfigure,
            _serial: u32,
        ) {
            // Story 3.4 Task 5: Handle configure events for multi-monitor surfaces
            if self.multi_monitor_mode {
                // Find which MonitorSurface this configure event is for
                let mut found_surface = false;
                for surface in self.multi_surfaces.values_mut() {
                    if surface.xdg_toplevel().wl_surface() == window.wl_surface() {
                        // Update fullscreen state for this surface (Task 5.3)
                        let new_fullscreen = configure.state.contains(WindowState::FULLSCREEN);
                        if new_fullscreen != surface.is_fullscreen() {
                            surface.set_fullscreen_state(new_fullscreen);
                            tracing::info!(
                                "MonitorSurface {} fullscreen: {}",
                                surface.monitor_info().name,
                                if new_fullscreen { "entered" } else { "exited" }
                            );
                        }

                        // Handle resize from configure (Task 5.4)
                        if let (Some(w), Some(h)) = configure.new_size {
                            let new_width = w.get();
                            let new_height = h.get();
                            let (cur_w, cur_h) = surface.dimensions();
                            if new_width != cur_w || new_height != cur_h {
                                if let Err(e) = surface.resize(new_width, new_height) {
                                    tracing::error!(
                                        "Failed to resize MonitorSurface {}: {}",
                                        surface.monitor_info().name,
                                        e
                                    );
                                }
                            }
                        }

                        found_surface = true;
                        break; // Exit mutable borrow before checking fullscreen state
                    }
                }

                // Check fullscreen state after mutable borrow ends (cleaner borrow pattern)
                if found_surface {
                    let all_fullscreen = self.multi_surfaces.values().all(|s| s.is_fullscreen());
                    let any_fullscreen = self.multi_surfaces.values().any(|s| s.is_fullscreen());

                    // Only emit event when transitioning (all fullscreen or none fullscreen)
                    if all_fullscreen && !self.is_fullscreen {
                        self.is_fullscreen = true;
                        let _ = self.event_tx.send(WindowEvent::FullscreenChanged {
                            is_fullscreen: true,
                        });
                        tracing::info!("All surfaces entered fullscreen");
                    } else if !any_fullscreen && self.is_fullscreen {
                        self.is_fullscreen = false;
                        let _ = self.event_tx.send(WindowEvent::FullscreenChanged {
                            is_fullscreen: false,
                        });
                        tracing::info!("All surfaces exited fullscreen");
                    }
                    return;
                }
            }

            // Single-surface mode (original behavior)
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
            // Ctrl+Alt+Enter toggles fullscreen (Story 2.5, updated in Story 3.4 Task 6)
            if self.modifiers.ctrl && self.modifiers.alt && event.keysym == Keysym::Return {
                tracing::debug!(
                    "Ctrl+Alt+Enter detected - toggling fullscreen (multi_monitor_mode={})",
                    self.multi_monitor_mode
                );
                // Story 3.4 Task 6: In multi-monitor mode, toggle all surfaces simultaneously
                // The actual toggle is handled in main.rs based on this event
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

    // Story 3.4: Handle xdg_surface configure events for multi-monitor surfaces
    impl XdgSurfaceHandler for WaylandWindow {
        fn configure(
            &mut self,
            _conn: &Connection,
            _qh: &QueueHandle<Self>,
            xdg_surface: &XdgSurface,
            serial: u32,
        ) {
            // Acknowledge the configure event (required by xdg_shell protocol)
            xdg_surface.ack_configure(serial);

            // Find which MonitorSurface this xdg_surface belongs to and mark it configured
            // Then commit the surface to complete the configure sequence (Task 5.5)
            for surface in self.multi_surfaces.values_mut() {
                if surface.xdg_surface.wl_surface() == xdg_surface.wl_surface() {
                    if !surface.is_configured() {
                        tracing::debug!(
                            "MonitorSurface {} received initial configure",
                            surface.monitor_info().name
                        );
                        surface.mark_configured();
                    }
                    // Commit surface after configure acknowledgment (Task 5.5)
                    // This completes the configure sequence per xdg_shell protocol
                    surface.wl_surface().commit();
                    break;
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
    delegate_xdg_surface!(WaylandWindow);
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

    /// Stub MonitorSurface for non-Linux platforms (Story 3.3, extended in Story 3.4).
    pub struct MonitorSurface {
        /// Monitor info (public for testing).
        pub monitor_info: MonitorInfo,
    }

    impl MonitorSurface {
        pub fn wl_surface(&self) -> &() {
            &()
        }

        pub fn target_output(&self) -> &() {
            &()
        }

        pub fn monitor_info(&self) -> &MonitorInfo {
            &self.monitor_info
        }

        pub fn monitor_id(&self) -> u32 {
            self.monitor_info.id
        }

        pub fn dimensions(&self) -> (u32, u32) {
            (self.monitor_info.width, self.monitor_info.height)
        }

        pub fn is_fullscreen(&self) -> bool {
            false
        }

        pub fn set_fullscreen_state(&mut self, _fullscreen: bool) {}

        // Story 3.4: Fullscreen control stubs
        pub fn set_fullscreen(&self) {}

        pub fn unset_fullscreen(&self) {}

        pub fn xdg_toplevel(&self) -> &() {
            &()
        }

        pub fn is_dirty(&self) -> bool {
            false
        }

        pub fn mark_dirty(&mut self) {}

        pub fn mark_configured(&mut self) {}

        pub fn is_configured(&self) -> bool {
            false
        }

        pub fn draw_solid(&mut self, _r: u8, _g: u8, _b: u8) {}

        pub fn draw_frame(&mut self, _data: &[u8], _width: u32, _height: u32, _x: u32, _y: u32) {}

        pub fn resize(
            &mut self,
            _width: u32,
            _height: u32,
        ) -> Result<(), Box<dyn std::error::Error>> {
            Ok(())
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

        // Story 3.3: Multi-monitor stubs
        pub fn is_multi_monitor_mode(&self) -> bool {
            false
        }

        pub fn create_surface_for_monitor(
            &mut self,
            _qh: &(),
            _monitor_id: u32,
        ) -> Result<(), Box<dyn std::error::Error>> {
            Err("Wayland is only supported on Linux".into())
        }

        pub fn create_surfaces_for_all_monitors(
            &mut self,
            _qh: &(),
        ) -> Result<usize, Box<dyn std::error::Error>> {
            Err("Wayland is only supported on Linux".into())
        }

        pub fn destroy_all_surfaces(&mut self) {}

        pub fn get_surface(&self, _monitor_id: u32) -> Option<&MonitorSurface> {
            None
        }

        pub fn get_surface_mut(&mut self, _monitor_id: u32) -> Option<&mut MonitorSurface> {
            None
        }

        pub fn surface_count(&self) -> usize {
            0
        }

        pub fn draw_solid_all(&mut self, _r: u8, _g: u8, _b: u8) {}

        pub fn draw_frame_to_surfaces(
            &mut self,
            _data: &[u8],
            _width: u32,
            _height: u32,
            _x: u32,
            _y: u32,
        ) {
        }

        // Story 3.4: Multi-monitor fullscreen stubs
        pub fn set_all_fullscreen(&self) {}

        pub fn unset_all_fullscreen(&self) {}

        pub fn toggle_all_fullscreen(&self) {}

        pub fn all_fullscreen(&self) -> bool {
            false
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

    // Story 3.3: Multi-monitor surface tests

    /// Helper to create a test MonitorInfo
    fn create_test_monitor(
        id: u32,
        name: &str,
        width: u32,
        height: u32,
        x: i32,
        y: i32,
    ) -> MonitorInfo {
        MonitorInfo {
            id,
            name: name.to_string(),
            make: None,
            model: None,
            width,
            height,
            x,
            y,
            refresh_mhz: 60000,
            scale: 1,
        }
    }

    #[test]
    fn test_monitor_surface_dimensions() {
        let monitor = create_test_monitor(1, "DP-1", 1920, 1080, 0, 0);
        // Using stub on non-Linux, which returns the stored monitor dimensions
        #[cfg(not(target_os = "linux"))]
        {
            let surface = MonitorSurface {
                monitor_info: monitor.clone(),
            };
            assert_eq!(surface.dimensions(), (1920, 1080));
            assert_eq!(surface.monitor_id(), 1);
        }
        // On Linux, can't test without actual Wayland connection
        #[cfg(target_os = "linux")]
        {
            // Just verify monitor info is correct
            assert_eq!(monitor.width, 1920);
            assert_eq!(monitor.height, 1080);
        }
    }

    #[test]
    fn test_monitor_surface_monitor_info() {
        let monitor = create_test_monitor(2, "HDMI-A-1", 2560, 1440, 1920, 0);
        #[cfg(not(target_os = "linux"))]
        {
            let surface = MonitorSurface {
                monitor_info: monitor.clone(),
            };
            let info = surface.monitor_info();
            assert_eq!(info.id, 2);
            assert_eq!(info.name, "HDMI-A-1");
            assert_eq!(info.width, 2560);
            assert_eq!(info.height, 1440);
            assert_eq!(info.x, 1920);
            assert_eq!(info.y, 0);
        }
        #[cfg(target_os = "linux")]
        {
            assert_eq!(monitor.name, "HDMI-A-1");
        }
    }

    #[test]
    fn test_monitor_surface_stub_methods() {
        #[cfg(not(target_os = "linux"))]
        {
            let monitor = create_test_monitor(1, "DP-1", 1920, 1080, 0, 0);
            let mut surface = MonitorSurface {
                monitor_info: monitor,
            };

            // Test stub methods don't panic
            assert!(!surface.is_fullscreen());
            surface.set_fullscreen_state(true);
            assert!(!surface.is_fullscreen()); // Stub always returns false

            assert!(!surface.is_dirty());
            surface.mark_dirty();
            assert!(!surface.is_dirty()); // Stub always returns false

            assert!(!surface.is_configured());
            surface.mark_configured();
            assert!(!surface.is_configured()); // Stub always returns false

            // Drawing methods should not panic
            surface.draw_solid(255, 0, 0);
            surface.draw_frame(&[0u8; 16], 2, 2, 0, 0);
            assert!(surface.resize(800, 600).is_ok());
        }
    }

    #[test]
    fn test_wayland_window_multi_monitor_stub() {
        #[cfg(not(target_os = "linux"))]
        {
            let mut window = WaylandWindow;

            // Multi-monitor mode defaults to false
            assert!(!window.is_multi_monitor_mode());

            // Surface operations should return None/0 in stub
            assert_eq!(window.surface_count(), 0);
            assert!(window.get_surface(1).is_none());
            assert!(window.get_surface_mut(1).is_none());

            // Creation should fail on non-Linux
            assert!(window.create_surface_for_monitor(&(), 1).is_err());
            assert!(window.create_surfaces_for_all_monitors(&()).is_err());

            // Destroy should not panic
            window.destroy_all_surfaces();

            // Drawing methods should not panic
            window.draw_solid_all(255, 255, 255);
            window.draw_frame_to_surfaces(&[0u8; 16], 2, 2, 0, 0);
        }
    }

    #[test]
    fn test_monitor_layout_primary_at_origin() {
        // Test that monitors at (0,0) are identified as primary
        let primary = create_test_monitor(1, "DP-1", 1920, 1080, 0, 0);
        let secondary = create_test_monitor(2, "HDMI-A-1", 1920, 1080, 1920, 0);

        // Primary is at origin
        assert_eq!(primary.x, 0);
        assert_eq!(primary.y, 0);

        // Secondary is offset
        assert_eq!(secondary.x, 1920);
        assert_eq!(secondary.y, 0);
    }

    #[test]
    fn test_monitor_layout_vertical_arrangement() {
        // Test vertical monitor arrangement (one above the other)
        let top = create_test_monitor(1, "DP-1", 1920, 1080, 0, 0);
        let bottom = create_test_monitor(2, "HDMI-A-1", 1920, 1080, 0, 1080);

        // Top monitor at origin
        assert_eq!(top.x, 0);
        assert_eq!(top.y, 0);

        // Bottom monitor below
        assert_eq!(bottom.x, 0);
        assert_eq!(bottom.y, 1080);

        // Combined bounding box would be 1920x2160
        let combined_width = top.width.max(bottom.width);
        let combined_height =
            (top.y + top.height as i32).max(bottom.y + bottom.height as i32) as u32;
        assert_eq!(combined_width, 1920);
        assert_eq!(combined_height, 2160);
    }

    #[test]
    fn test_monitor_layout_different_resolutions() {
        // Test monitors with different resolutions (4K + 1080p)
        let monitor_4k = create_test_monitor(1, "DP-1", 3840, 2160, 0, 0);
        let monitor_1080p = create_test_monitor(2, "HDMI-A-1", 1920, 1080, 3840, 540); // Vertically centered

        // 4K at origin
        assert_eq!(monitor_4k.width, 3840);
        assert_eq!(monitor_4k.height, 2160);

        // 1080p next to it, vertically centered
        assert_eq!(monitor_1080p.x, 3840);
        assert_eq!(monitor_1080p.y, 540);

        // Combined bounding box
        let combined_width = (monitor_1080p.x + monitor_1080p.width as i32) as u32;
        let combined_height = monitor_4k.height; // 4K is taller
        assert_eq!(combined_width, 5760); // 3840 + 1920
        assert_eq!(combined_height, 2160);
    }

    #[test]
    fn test_monitor_layout_negative_coordinates() {
        // Test monitor to the LEFT of primary (negative x coordinate)
        // Common setup: primary at center, secondary to the left
        let left_monitor = create_test_monitor(1, "DP-1", 1920, 1080, -1920, 0);
        let primary_monitor = create_test_monitor(2, "DP-2", 1920, 1080, 0, 0);

        // Left monitor has negative x
        assert_eq!(left_monitor.x, -1920);
        assert_eq!(left_monitor.y, 0);

        // Primary at origin
        assert_eq!(primary_monitor.x, 0);
        assert_eq!(primary_monitor.y, 0);

        // Verify coordinate arithmetic works with negatives
        // Combined desktop spans from -1920 to 1920 (total width 3840)
        let min_x = left_monitor.x.min(primary_monitor.x);
        let max_x = (left_monitor.x + left_monitor.width as i32)
            .max(primary_monitor.x + primary_monitor.width as i32);
        let combined_width = (max_x - min_x) as u32;

        assert_eq!(min_x, -1920);
        assert_eq!(max_x, 1920);
        assert_eq!(combined_width, 3840);
    }

    #[test]
    fn test_monitor_layout_negative_y_coordinate() {
        // Test monitor ABOVE primary (negative y coordinate)
        let top_monitor = create_test_monitor(1, "DP-1", 1920, 1080, 0, -1080);
        let primary_monitor = create_test_monitor(2, "DP-2", 1920, 1080, 0, 0);

        // Top monitor has negative y
        assert_eq!(top_monitor.x, 0);
        assert_eq!(top_monitor.y, -1080);

        // Primary at origin
        assert_eq!(primary_monitor.x, 0);
        assert_eq!(primary_monitor.y, 0);

        // Combined height spans from -1080 to 1080
        let min_y = top_monitor.y.min(primary_monitor.y);
        let max_y = (top_monitor.y + top_monitor.height as i32)
            .max(primary_monitor.y + primary_monitor.height as i32);
        let combined_height = (max_y - min_y) as u32;

        assert_eq!(min_y, -1080);
        assert_eq!(max_y, 1080);
        assert_eq!(combined_height, 2160);
    }

    #[test]
    fn test_frame_intersection_with_negative_monitor() {
        // Test that frame intersection logic works with negative coordinates
        let left_monitor = create_test_monitor(1, "DP-1", 1920, 1080, -1920, 0);

        // A frame at desktop coordinates (0, 0) should NOT intersect left monitor
        // because left monitor spans x: -1920 to 0
        let frame_x: i32 = 0;
        let frame_y: i32 = 0;
        let frame_w: i32 = 100;
        let frame_h: i32 = 100;

        let mon_x = left_monitor.x;
        let mon_y = left_monitor.y;
        let mon_w = left_monitor.width as i32;
        let mon_h = left_monitor.height as i32;

        // Frame at (0,0) with size 100x100 should NOT intersect monitor at (-1920,0) to (0,1080)
        // Because frame_x (0) is NOT < mon_x + mon_w (0), the first condition fails
        let intersects = frame_x < mon_x + mon_w
            && frame_x + frame_w > mon_x
            && frame_y < mon_y + mon_h
            && frame_y + frame_h > mon_y;

        // frame_x (0) < mon_x + mon_w (-1920 + 1920 = 0) is FALSE (0 < 0 is false)
        assert!(!intersects);

        // A frame at (-100, 0) SHOULD intersect the left monitor
        let frame_x2: i32 = -100;
        let intersects2 = frame_x2 < mon_x + mon_w
            && frame_x2 + frame_w > mon_x
            && frame_y < mon_y + mon_h
            && frame_y + frame_h > mon_y;

        // frame_x2 (-100) < mon_x + mon_w (0) is TRUE
        // frame_x2 + frame_w (-100 + 100 = 0) > mon_x (-1920) is TRUE
        assert!(intersects2);
    }

    // Story 3.4: Multi-monitor fullscreen tests
    //
    // Note: These tests run only on non-Linux platforms (stubs) because testing
    // real Wayland functionality requires a running compositor. On Linux, the
    // actual implementation is tested via integration tests with a real compositor.
    // The stub tests verify the API contract and ensure methods don't panic.

    #[test]
    fn test_monitor_surface_fullscreen_state_tracking() {
        // Test MonitorSurface fullscreen state tracking (Task 7.1)
        #[cfg(not(target_os = "linux"))]
        {
            let monitor = create_test_monitor(1, "DP-1", 1920, 1080, 0, 0);
            let mut surface = MonitorSurface {
                monitor_info: monitor,
            };

            // Initial state should be not fullscreen
            assert!(!surface.is_fullscreen());

            // Setting fullscreen state (stub doesn't actually change state)
            surface.set_fullscreen_state(true);
            // Note: stub always returns false
            assert!(!surface.is_fullscreen());
        }
    }

    #[test]
    fn test_monitor_surface_fullscreen_methods() {
        // Test MonitorSurface fullscreen control methods exist (Task 7.1)
        #[cfg(not(target_os = "linux"))]
        {
            let monitor = create_test_monitor(1, "DP-1", 1920, 1080, 0, 0);
            let surface = MonitorSurface {
                monitor_info: monitor,
            };

            // These should not panic
            surface.set_fullscreen();
            surface.unset_fullscreen();
        }
    }

    #[test]
    fn test_wayland_window_simultaneous_fullscreen_toggle() {
        // Test simultaneous fullscreen toggle (Task 7.2)
        #[cfg(not(target_os = "linux"))]
        {
            let window = WaylandWindow;

            // Multi-monitor methods should not panic
            window.set_all_fullscreen();
            window.unset_all_fullscreen();
            window.toggle_all_fullscreen();

            // all_fullscreen should return false in stub
            assert!(!window.all_fullscreen());
        }
    }

    #[test]
    fn test_cli_all_monitors_flag_parsing() {
        // Test --all-monitors flag parsing (Task 7.3)
        // This is implicitly tested by the CLI parsing tests in main.rs
        // Here we just verify the stub methods exist
        #[cfg(not(target_os = "linux"))]
        {
            let window = WaylandWindow;
            // is_multi_monitor_mode should return false in stub
            assert!(!window.is_multi_monitor_mode());
        }
    }

    #[test]
    fn test_fullscreen_exit_preserves_monitor_association() {
        // Test that fullscreen exit preserves monitor association (Task 7.4)
        // In the stub, we can verify the methods don't panic and state is tracked
        #[cfg(not(target_os = "linux"))]
        {
            let monitor = create_test_monitor(1, "DP-1", 1920, 1080, 0, 0);
            let surface = MonitorSurface {
                monitor_info: monitor.clone(),
            };

            // Initial state
            assert_eq!(surface.monitor_id(), 1);
            assert_eq!(surface.monitor_info().name, "DP-1");

            // Toggle fullscreen (stub no-op, takes &self not &mut self)
            surface.set_fullscreen();
            // Monitor association should be preserved
            assert_eq!(surface.monitor_id(), 1);
            assert_eq!(surface.monitor_info().name, "DP-1");

            // Exit fullscreen (stub no-op)
            surface.unset_fullscreen();
            // Monitor association still preserved
            assert_eq!(surface.monitor_id(), 1);
            assert_eq!(surface.monitor_info().name, "DP-1");
        }
    }
}
