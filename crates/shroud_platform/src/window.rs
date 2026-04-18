use std::sync::Arc;

use crate::display_protection::{DisplayProtection, DisplayProtectionResult};
use winit::dpi::{LogicalSize, PhysicalSize};
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowAttributes};

/// Wraps a winit `Window` with platform-specific security extensions.
pub struct PlatformWindow {
    window: Arc<Window>,
    display_protection: DisplayProtection,
}

impl PlatformWindow {
    /// Create a new window on the given event loop.
    pub fn new(event_loop: &ActiveEventLoop, title: &str, width: u32, height: u32) -> Self {
        let attrs = WindowAttributes::default()
            .with_title(title)
            .with_inner_size(LogicalSize::new(width, height));

        let window = event_loop
            .create_window(attrs)
            .expect("failed to create window");

        let window = Arc::new(window);
        let display_protection = DisplayProtection::new(Arc::clone(&window));

        Self {
            window,
            display_protection,
        }
    }

    /// Get a clone of the Arc<Window> for sharing with the renderer.
    pub fn arc(&self) -> Arc<Window> {
        Arc::clone(&self.window)
    }

    /// Get the current inner size in physical pixels.
    pub fn inner_size(&self) -> PhysicalSize<u32> {
        self.window.inner_size()
    }

    /// Request a redraw.
    pub fn request_redraw(&self) {
        self.window.request_redraw();
    }

    /// Access the display protection manager.
    pub fn display_protection(&self) -> &DisplayProtection {
        &self.display_protection
    }

    /// Mutable access to the display protection manager.
    pub fn display_protection_mut(&mut self) -> &mut DisplayProtection {
        &mut self.display_protection
    }

    /// Enable screen capture prevention (convenience method).
    ///
    /// Delegates to `DisplayProtection::enable()`.
    pub fn set_capture_prevention(&mut self, enabled: bool) -> DisplayProtectionResult {
        if enabled {
            self.display_protection.enable()
        } else {
            self.display_protection.disable()
        }
    }
}
