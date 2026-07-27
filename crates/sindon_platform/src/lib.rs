//! sindon_platform — OS integration: window, clipboard, screen capture prevention.
//!
//! - [`window`]: `PlatformWindow` wraps winit + renderer + display protection
//! - [`clipboard`]: `SecureClipboard` with owner-scoped secret strings
//! - [`display_protection`]: `SetWindowDisplayAffinity` (Windows) —
//!   macOS/Linux are no-ops (no equivalent API)
//! - [`dialog`]: native file/folder dialogs (`rfd` wrapper)
//! - [`storage`]: per-user JSON config helpers (atomic write via rename)
//! - [`system_locale()`]: best-effort OS locale tag (BCP-47)
//! - [`system_theme`]: OS light/dark preference enum (paired with
//!   `AppScope::system_theme` for reactive updates)
//! - [`caret_blink_time()`]: OS caret-blink half-period (`None` = don't blink)

pub mod caret;
pub mod clipboard;
pub mod dialog;
pub mod display_protection;
pub mod storage;
pub mod system_locale;
pub mod system_theme;
pub mod window;

pub use caret::caret_blink_time;
pub use clipboard::{ClipboardImage, SecureClipboard};
pub use dialog::FileDialog;
pub use display_protection::{DisplayProtection, DisplayProtectionLevel, DisplayProtectionResult};
pub use storage::{config_dir, read_json, write_json_atomic};
pub use system_locale::system_locale;
pub use system_theme::SystemTheme;
pub use window::PlatformWindow;
