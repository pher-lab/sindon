//! Secure texture atlas — cleared every frame after rendering.
//!
//! Glyphs from `SecureText` / `SecureInput` are uploaded here instead of
//! the standard atlas. After each frame's render pass completes, the entire
//! GPU texture is zeroed and the CPU-side cache is discarded. This prevents
//! sensitive text glyph data from persisting in GPU memory across frames.

use crate::atlas::{AtlasRegion, DEFAULT_ATLAS_SIZE, TextureAtlas};
use shroud_text::CacheKey;

/// A texture atlas that is cleared every frame.
///
/// Uses the same shelf-packing as [`TextureAtlas`], but after each frame:
/// 1. GPU texture is zeroed
/// 2. CPU glyph cache is cleared
/// 3. Shelf allocator is reset
///
/// This means every glyph must be re-uploaded each frame — a performance
/// cost accepted for security. Secure text is typically short (passwords,
/// keys, tokens).
pub struct SecureTextureAtlas {
    inner: TextureAtlas,
}

impl SecureTextureAtlas {
    /// Create a new secure atlas (defaults to 512x512, smaller than standard).
    pub fn new(device: &wgpu::Device) -> Self {
        Self {
            inner: TextureAtlas::new(device, DEFAULT_ATLAS_SIZE / 2, DEFAULT_ATLAS_SIZE / 2),
        }
    }

    /// Create with custom dimensions.
    pub fn with_size(device: &wgpu::Device, width: u32, height: u32) -> Self {
        Self {
            inner: TextureAtlas::new(device, width, height),
        }
    }

    /// Look up a glyph in this frame's cache.
    pub fn get(&self, key: &CacheKey) -> Option<&AtlasRegion> {
        self.inner.get(key)
    }

    /// Upload a glyph for this frame.
    pub fn upload(
        &mut self,
        queue: &wgpu::Queue,
        key: CacheKey,
        data: &[u8],
        width: u32,
        height: u32,
    ) -> Option<AtlasRegion> {
        self.inner.upload(queue, key, data, width, height)
    }

    /// Clear the atlas. Call this after every frame's render pass.
    ///
    /// Zeros the GPU texture and resets all CPU state.
    pub fn clear_after_frame(&mut self, queue: &wgpu::Queue) {
        self.inner.clear(queue);
    }

    /// Whether any glyphs were uploaded this frame.
    pub fn is_dirty(&self) -> bool {
        self.inner.is_dirty()
    }

    /// Reset the dirty flag.
    pub fn clear_dirty(&mut self) {
        self.inner.clear_dirty();
    }

    /// Get the texture view (for bind group creation).
    pub fn view(&self) -> &wgpu::TextureView {
        self.inner.view()
    }

    /// Atlas width.
    pub fn width(&self) -> u32 {
        self.inner.width()
    }

    /// Atlas height.
    pub fn height(&self) -> u32 {
        self.inner.height()
    }

    /// Number of cached glyphs (should be 0 after clear).
    pub fn glyph_count(&self) -> usize {
        self.inner.glyph_count()
    }

    /// Access the underlying TextureAtlas (for geometry building).
    pub fn as_atlas(&self) -> &TextureAtlas {
        &self.inner
    }
}

impl std::fmt::Debug for SecureTextureAtlas {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecureTextureAtlas")
            .field("inner", &self.inner)
            .finish()
    }
}
