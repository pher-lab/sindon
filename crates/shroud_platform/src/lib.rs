//! shroud_platform — OS integration: window, clipboard, screen capture prevention.
//!
//! - [`window`]: `PlatformWindow` wraps winit + renderer + display protection
//! - [`clipboard`]: `SecureClipboard` with owner-scoped secret strings
//! - [`display_protection`]: `SetWindowDisplayAffinity` (Windows) —
//!   macOS/Linux are no-ops (no equivalent API)
//! - [`system_locale`]: best-effort OS locale tag (BCP-47)
//! - [`system_theme`]: OS light/dark preference enum (paired with
//!   `AppScope::system_theme` for reactive updates)

pub mod clipboard;
pub mod display_protection;
pub mod system_locale;
pub mod system_theme;
pub mod window;

pub use clipboard::SecureClipboard;
pub use display_protection::{DisplayProtection, DisplayProtectionLevel, DisplayProtectionResult};
pub use system_locale::system_locale;
pub use system_theme::SystemTheme;
pub use window::PlatformWindow;
