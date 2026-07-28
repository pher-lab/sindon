//! Secure clipboard integration.
//!
//! Provides read/write to the system clipboard with:
//! - Auto-clear timer: clipboard content from secure fields is cleared
//!   after a configurable duration
//! - SecureString-aware paste: read clipboard directly into a SecureString

use sindon_security::SecureString;
use std::time::{Duration, Instant};
use zeroize::Zeroizing;

/// Default auto-clear duration (10 seconds).
pub const DEFAULT_AUTO_CLEAR_SECS: u64 = 10;

/// Secure clipboard manager.
///
/// Wraps system clipboard access and tracks when secure data was copied
/// so it can be automatically cleared.
pub struct SecureClipboard {
    /// When secure content was last written, if any.
    secure_write_time: Option<Instant>,
    /// Duration after which secure clipboard content is auto-cleared.
    auto_clear_duration: Duration,
}

impl SecureClipboard {
    /// Create a new clipboard manager with default auto-clear (10s).
    pub fn new() -> Self {
        Self {
            secure_write_time: None,
            auto_clear_duration: Duration::from_secs(DEFAULT_AUTO_CLEAR_SECS),
        }
    }

    /// Set the auto-clear duration.
    pub fn with_auto_clear(mut self, duration: Duration) -> Self {
        self.auto_clear_duration = duration;
        self
    }

    /// Write normal (non-sensitive) text to the clipboard.
    pub fn write(&mut self, text: &str) -> Result<(), ClipboardError> {
        let mut board = arboard::Clipboard::new().map_err(|_| ClipboardError::Unavailable)?;
        board
            .set_text(text)
            .map_err(|_| ClipboardError::WriteFailed)?;
        Ok(())
    }

    /// Write secure text to the clipboard with auto-clear timer.
    ///
    /// Starts a timer — call `tick()` periodically to check and clear.
    pub fn write_secure(&mut self, text: &SecureString) -> Result<(), ClipboardError> {
        let mut board = arboard::Clipboard::new().map_err(|_| ClipboardError::Unavailable)?;
        text.expose(|s| board.set_text(s).map_err(|_| ClipboardError::WriteFailed))?;
        self.secure_write_time = Some(Instant::now());
        Ok(())
    }

    /// Read clipboard text into a SecureString.
    ///
    /// `arboard::get_text` returns a plain `String`, so plaintext exists
    /// briefly on the heap. We wrap it in `Zeroizing` so the buffer is
    /// wiped on drop. The pre-realloc capacity of the `String` is what we
    /// can guarantee; if `arboard` resized internally before returning,
    /// older buffers are out of our reach.
    pub fn read_secure(&self) -> Result<SecureString, ClipboardError> {
        let mut board = arboard::Clipboard::new().map_err(|_| ClipboardError::Unavailable)?;
        let text = Zeroizing::new(board.get_text().map_err(|_| ClipboardError::ReadFailed)?);
        Ok(SecureString::new(&text))
    }

    /// Read clipboard as plain text.
    pub fn read(&self) -> Result<String, ClipboardError> {
        let mut board = arboard::Clipboard::new().map_err(|_| ClipboardError::Unavailable)?;
        board.get_text().map_err(|_| ClipboardError::ReadFailed)
    }

    /// Read an image from the clipboard as tightly-packed RGBA8 pixels.
    ///
    /// Returns [`ClipboardError::ReadFailed`] when the clipboard holds no
    /// image (the usual case when text was copied). Pixels are plaintext on
    /// the heap until dropped — the same exposure the text path carries — so
    /// this is for ordinary (non-secret) image content such as a screenshot
    /// pasted into a note, not for secret pixel data.
    pub fn read_image(&self) -> Result<ClipboardImage, ClipboardError> {
        let mut board = arboard::Clipboard::new().map_err(|_| ClipboardError::Unavailable)?;
        let img = board.get_image().map_err(|_| ClipboardError::ReadFailed)?;
        Ok(ClipboardImage {
            width: img.width as u32,
            height: img.height as u32,
            rgba: img.bytes.into_owned(),
        })
    }

    /// Check if auto-clear timer has expired and clear if needed.
    ///
    /// Call this periodically (e.g., once per frame or per second).
    /// Returns `true` if the clipboard was cleared.
    pub fn tick(&mut self) -> bool {
        if let Some(write_time) = self.secure_write_time
            && write_time.elapsed() >= self.auto_clear_duration
        {
            self.force_clear();
            return true;
        }
        false
    }

    /// Force-clear the clipboard immediately.
    pub fn force_clear(&mut self) {
        if let Ok(mut board) = arboard::Clipboard::new() {
            let _ = board.set_text("");
        }
        self.secure_write_time = None;
    }

    /// Whether a secure auto-clear timer is currently active.
    pub fn is_timer_active(&self) -> bool {
        self.secure_write_time.is_some()
    }

    /// Time remaining before auto-clear, if timer is active.
    pub fn time_remaining(&self) -> Option<Duration> {
        self.secure_write_time.map(|t| {
            self.auto_clear_duration
                .checked_sub(t.elapsed())
                .unwrap_or(Duration::ZERO)
        })
    }
}

impl Default for SecureClipboard {
    fn default() -> Self {
        Self::new()
    }
}

/// An image read from the system clipboard.
///
/// Holds tightly-packed RGBA8 pixels (`rgba.len() == width * height * 4`)
/// in the order arboard delivers them — the same raw form a screenshot or
/// an image copied from a browser arrives in. Encode it (e.g. via
/// `sindon_render::encode_png`) before persisting; it is not a
/// self-describing image file on its own.
pub struct ClipboardImage {
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
    /// Tightly-packed RGBA8 pixels, row-major, top-left origin.
    pub rgba: Vec<u8>,
}

/// Clipboard errors.
#[derive(Debug)]
pub enum ClipboardError {
    /// System clipboard not available.
    Unavailable,
    /// Failed to write to clipboard.
    WriteFailed,
    /// Failed to read from clipboard.
    ReadFailed,
}

impl std::fmt::Display for ClipboardError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable => write!(f, "clipboard unavailable"),
            Self::WriteFailed => write!(f, "failed to write to clipboard"),
            Self::ReadFailed => write!(f, "failed to read from clipboard"),
        }
    }
}

impl std::error::Error for ClipboardError {}
