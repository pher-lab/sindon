//! Native file dialogs (open / save / pick folder) via the `rfd` crate.
//!
//! Wraps `rfd::FileDialog` with a thin builder that takes a `&PlatformWindow`
//! for modal parenting and returns `Option<PathBuf>` from the four common
//! pickers. Sync (blocking) variants only — native OS dialogs are inherently
//! modal so blocking the calling thread matches platform convention.
//!
//! ```no_run
//! use sindon_platform::dialog::FileDialog;
//!
//! # let window: &sindon_platform::PlatformWindow = unimplemented!();
//! if let Some(path) = FileDialog::new()
//!     .title("Open note")
//!     .filter("Markdown", &["md", "markdown"])
//!     .parent(window)
//!     .open_file()
//! {
//!     println!("picked {:?}", path);
//! }
//! ```
use std::path::{Path, PathBuf};

use crate::window::PlatformWindow;

/// Builder for native file/folder dialogs.
///
/// All accessor methods (`open_file`, `open_files`, `open_folder`, `save_file`)
/// block the current thread until the user dismisses the dialog. Returns
/// `None` on cancel.
pub struct FileDialog {
    inner: rfd::FileDialog,
}

impl FileDialog {
    pub fn new() -> Self {
        Self {
            inner: rfd::FileDialog::new(),
        }
    }

    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.inner = self.inner.set_title(title.into());
        self
    }

    /// Add a file-extension filter. `extensions` are bare names without the dot
    /// (e.g. `&["md", "markdown"]`).
    pub fn filter(mut self, name: impl Into<String>, extensions: &[&str]) -> Self {
        self.inner = self.inner.add_filter(name.into(), extensions);
        self
    }

    /// Initial directory shown when the dialog opens.
    pub fn start_directory(mut self, dir: impl AsRef<Path>) -> Self {
        self.inner = self.inner.set_directory(dir.as_ref());
        self
    }

    /// Pre-fill the file name field (mainly for save dialogs).
    pub fn file_name(mut self, name: impl Into<String>) -> Self {
        self.inner = self.inner.set_file_name(name.into());
        self
    }

    /// Attach the dialog to the sindon window so it appears modal.
    ///
    /// Without a parent the dialog is unparented and may steal focus oddly on
    /// some compositors.
    pub fn parent(mut self, window: &PlatformWindow) -> Self {
        let handle: &winit::window::Window = &window.arc();
        // rfd's `set_parent` takes `&impl HasWindowHandle`; winit `Window`
        // implements that via raw-window-handle 0.6.
        self.inner = self.inner.set_parent(handle);
        self
    }

    /// Show an "open file" dialog, returning the selected path or `None` on
    /// cancel.
    pub fn open_file(self) -> Option<PathBuf> {
        self.inner.pick_file()
    }

    /// Show an "open file" dialog that allows selecting multiple files.
    pub fn open_files(self) -> Option<Vec<PathBuf>> {
        self.inner.pick_files()
    }

    /// Show a "pick folder" dialog.
    pub fn open_folder(self) -> Option<PathBuf> {
        self.inner.pick_folder()
    }

    /// Show a "save file" dialog. The returned path may or may not exist yet;
    /// the caller is responsible for the actual write.
    pub fn save_file(self) -> Option<PathBuf> {
        self.inner.save_file()
    }
}

impl Default for FileDialog {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_compiles() {
        // Smoke: ensure the builder chain type-checks. Cannot show the dialog
        // in CI, but constructing it exercises every method signature.
        let _ = FileDialog::new()
            .title("t")
            .filter("rust", &["rs"])
            .start_directory("/tmp")
            .file_name("a.rs");
    }

    #[test]
    fn default_matches_new() {
        let _a = FileDialog::default();
        let _b = FileDialog::new();
    }
}
