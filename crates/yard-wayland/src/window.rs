//! Wayland window implementation for YARD.
//!
//! This module provides the WaylandWindow struct which handles:
//! - Window creation using xdg_shell
//! - Surface management and buffer handling
//! - Frame rendering via shared memory

#[cfg(target_os = "linux")]
mod linux {
    use std::sync::Arc;

    use calloop::channel::{Channel, Sender};
    use calloop::{EventLoop, LoopHandle};
    use calloop_wayland_source::WaylandSource;
    use smithay_client_toolkit::compositor::{CompositorHandler, CompositorState};
    use smithay_client_toolkit::output::{OutputHandler, OutputState};
    use smithay_client_toolkit::reexports::client::globals::registry_queue_init;
    use smithay_client_toolkit::reexports::client::protocol::wl_output::WlOutput;
    use smithay_client_toolkit::reexports::client::protocol::wl_surface::WlSurface;
    use smithay_client_toolkit::reexports::client::{Connection, QueueHandle};
    use smithay_client_toolkit::registry::{ProvidesRegistryState, RegistryState};
    use smithay_client_toolkit::shell::xdg::window::{
        Window, WindowConfigure, WindowDecorations, WindowHandler,
    };
    use smithay_client_toolkit::shell::xdg::XdgShell;
    use smithay_client_toolkit::shell::WaylandSurface;
    use smithay_client_toolkit::shm::slot::{Buffer, SlotPool};
    use smithay_client_toolkit::shm::{Shm, ShmHandler};
    use smithay_client_toolkit::{
        delegate_compositor, delegate_output, delegate_registry, delegate_shm, delegate_xdg_shell,
        delegate_xdg_window, registry_handlers,
    };

    /// Messages sent from the Wayland window to the main application.
    #[derive(Debug)]
    pub enum WindowEvent {
        /// Window was closed by the user.
        CloseRequested,
        /// Window was resized.
        Resized { width: u32, height: u32 },
        /// Window needs to be redrawn.
        RedrawRequested,
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
    }

    impl Default for WindowConfig {
        fn default() -> Self {
            Self {
                title: "YARD".to_string(),
                width: 1280,
                height: 720,
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
    }

    /// The main Wayland window state.
    pub struct WaylandWindow {
        registry_state: RegistryState,
        output_state: OutputState,
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
    }

    impl WaylandWindow {
        /// Creates a new Wayland window.
        ///
        /// Returns the window state, event loop, and a channel receiver for window events.
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

            // Create surface and window
            let surface = compositor.create_surface(&qh);
            let window = xdg_shell.create_window(
                surface,
                WindowDecorations::ServerDefault,
                &qh,
            );

            window.set_title(config.title);
            window.set_app_id("yard");
            window.set_min_size(Some((320, 240)));
            window.commit();

            // Create shared memory pool for buffers
            let pool = SlotPool::new(
                (config.width * config.height * 4) as usize,
                &shm,
            )?;

            // Initialize retained content buffer (black/transparent)
            let retained_size = (config.width * config.height * 4) as usize;
            let retained_content = vec![0u8; retained_size];

            let state = Self {
                registry_state: RegistryState::new(&globals),
                output_state: OutputState::new(&globals, &qh),
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
                smithay_client_toolkit::shm::wl_shm::Format::Argb8888,
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
                smithay_client_toolkit::shm::wl_shm::Format::Argb8888,
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
        fn draw_partial_frame(&mut self, data: &[u8], width: u32, height: u32, x: u32, y: u32, stride: u32) {
            let window_stride = self.width * 4;
            let frame_stride = stride;

            // Create a new buffer (we can't modify the attached one)
            let (buffer, canvas) = match self.pool.create_buffer(
                self.width as i32,
                self.height as i32,
                window_stride as i32,
                smithay_client_toolkit::shm::wl_shm::Format::Argb8888,
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

        fn new_output(
            &mut self,
            _conn: &Connection,
            _qh: &QueueHandle<Self>,
            _output: WlOutput,
        ) {
        }

        fn update_output(
            &mut self,
            _conn: &Connection,
            _qh: &QueueHandle<Self>,
            _output: WlOutput,
        ) {
        }

        fn output_destroyed(
            &mut self,
            _conn: &Connection,
            _qh: &QueueHandle<Self>,
            _output: WlOutput,
        ) {
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

        registry_handlers![OutputState];
    }

    delegate_compositor!(WaylandWindow);
    delegate_output!(WaylandWindow);
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
    /// Stub WindowEvent for non-Linux platforms.
    #[derive(Debug)]
    pub enum WindowEvent {
        CloseRequested,
        Resized { width: u32, height: u32 },
        RedrawRequested,
    }

    /// Stub WindowConfig for non-Linux platforms.
    #[derive(Debug, Clone)]
    pub struct WindowConfig {
        pub title: String,
        pub width: u32,
        pub height: u32,
    }

    impl Default for WindowConfig {
        fn default() -> Self {
            Self {
                title: "YARD".to_string(),
                width: 1280,
                height: 720,
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
            }
        }

        /// Sets the window size.
        #[must_use]
        pub fn with_size(mut self, width: u32, height: u32) -> Self {
            self.width = width;
            self.height = height;
            self
        }
    }

    /// Stub WaylandWindow for non-Linux platforms.
    pub struct WaylandWindow;

    impl WaylandWindow {
        pub fn new(
            _config: WindowConfig,
        ) -> Result<
            ((), Self, ()),
            Box<dyn std::error::Error>,
        > {
            Err("Wayland is only supported on Linux".into())
        }

        pub fn close_requested(&self) -> bool {
            false
        }

        pub fn dimensions(&self) -> (u32, u32) {
            (0, 0)
        }

        pub fn draw_solid(&mut self, _r: u8, _g: u8, _b: u8) {}

        pub fn draw_frame(&mut self, _data: &[u8], _width: u32, _height: u32) {}

        pub fn draw_frame_at(&mut self, _data: &[u8], _width: u32, _height: u32, _x: u32, _y: u32) {}

        pub fn draw_frame_at_with_stride(
            &mut self,
            _data: &[u8],
            _width: u32,
            _height: u32,
            _x: u32,
            _y: u32,
            _stride: u32,
        ) {}
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
        let config = WindowConfig::with_connection_info("host", 3389, Some("user"))
            .with_size(2560, 1440);
        assert_eq!(config.title, "YARD - host:3389 [user]");
        assert_eq!(config.width, 2560);
        assert_eq!(config.height, 1440);
    }
}
