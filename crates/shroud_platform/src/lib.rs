//! shroud_platform — OS integration: window, clipboard, screen capture prevention.
//!
//! - [`window`]: `PlatformWindow` wraps winit + renderer + display protection
//! - [`clipboard`]: `SecureClipboard` with owner-scoped secret strings
//! - [`display_protection`]: `SetWindowDisplayAffinity` (Windows) —
//!   macOS/Linux are no-ops (no equivalent API)

pub mod clipboard;
pub mod display_protection;
pub mod window;

pub use clipboard::SecureClipboard;
pub use display_protection::{DisplayProtection, DisplayProtectionLevel, DisplayProtectionResult};
pub use window::PlatformWindow;
