//! Secure texture atlas — cleared every frame after rendering.
//!
//! Glyphs from `SecureText` / `SecureInput` are uploaded here instead of
//! the standard atlas. After each frame's render pass completes, the entire
//! GPU texture is zeroed and the CPU-side cache is discarded. This prevents
//! sensitive text glyph data from persisting in GPU memory across frames.

use crate::atlas::{AtlasRegion, DEFAULT_ATLAS_SIZE, TextureAtlas};
use sindon_text::CacheKey;

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
    /// Whether secret pixels have reached the GPU texture since the last
    /// *completed* clear. Sticky and deliberately pessimistic: set the moment
    /// an upload lands, and lowered only once the GPU has been observed to
    /// finish zeroing (see [`mark_cleared`](Self::mark_cleared)).
    ///
    /// A texture that has never been uploaded to cannot hold residue — wgpu
    /// zero-initializes textures — so a frame that leaves this `false` has
    /// nothing to clear and nothing to wait for.
    held_secret: bool,
}

impl SecureTextureAtlas {
    /// Create a new secure atlas (half the standard atlas dimensions).
    pub fn new(device: &wgpu::Device) -> Self {
        Self {
            inner: TextureAtlas::new(device, DEFAULT_ATLAS_SIZE / 2, DEFAULT_ATLAS_SIZE / 2),
            held_secret: false,
        }
    }

    /// Create with custom dimensions.
    pub fn with_size(device: &wgpu::Device, width: u32, height: u32) -> Self {
        Self {
            inner: TextureAtlas::new(device, width, height),
            held_secret: false,
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
        let region = self.inner.upload(queue, key, data, width, height);
        if region.is_some() {
            // Pixels are on the texture (or were, earlier this frame — the
            // cache is emptied by every clear, so a hit means the same frame).
            self.held_secret = true;
        }
        region
    }

    /// Whether this atlas may hold secret pixels right now.
    ///
    /// `false` means the frame that just rendered drew no secure glyphs *and*
    /// the previous clear completed, so there is nothing to zero.
    pub fn held_secret(&self) -> bool {
        self.held_secret
    }

    /// Clear the atlas. Call this after a frame's render pass.
    ///
    /// Zeros the written region of the GPU texture and resets all CPU state.
    /// This only *queues* the zeroing; it is not on the GPU until the next
    /// submit. Call [`mark_cleared`](Self::mark_cleared) once that has been
    /// observed to complete.
    pub fn clear_after_frame(&mut self, queue: &wgpu::Queue) {
        self.inner.clear(queue);
    }

    /// Record that the GPU has finished the queued zeroing.
    ///
    /// ⚠ Call this **only** after the submission carrying the clear is known
    /// to have completed. Calling it early — or on a wait that timed out —
    /// would let a later frame skip a clear that never actually ran.
    pub fn mark_cleared(&mut self) {
        self.held_secret = false;
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
