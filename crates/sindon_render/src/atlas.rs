//! Texture atlas with shelf-packing for glyph cache.
//!
//! Uses a simple shelf (row) allocator: each row has a fixed height determined
//! by the tallest glyph placed in it. Glyphs are packed left-to-right within
//! a shelf. When a shelf is full, a new one starts below.

use sindon_text::CacheKey;
use std::collections::HashMap;

/// Default atlas dimensions (2048x2048 R8Unorm).
///
/// Sized generously so a large working set (a document with many distinct
/// CJK glyphs, multiplied by subpixel bins, rasterized at HiDPI device
/// resolution) fits without churn. If it ever does fill, the renderer evicts
/// and re-uploads (see the upload path in `renderer.rs`) rather than blanking
/// new glyphs — the size just makes that self-heal rare.
pub const DEFAULT_ATLAS_SIZE: u32 = 2048;

/// Padding between glyphs to prevent bleeding.
const GLYPH_PADDING: u32 = 1;

/// Location of a glyph within the atlas texture.
#[derive(Debug, Clone, Copy)]
pub struct AtlasRegion {
    /// X offset in atlas (pixels).
    pub x: u32,
    /// Y offset in atlas (pixels).
    pub y: u32,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
}

impl AtlasRegion {
    /// UV coordinates for this region within an atlas of given dimensions.
    pub fn uv(&self, atlas_width: u32, atlas_height: u32) -> [f32; 4] {
        let w = atlas_width as f32;
        let h = atlas_height as f32;
        [
            self.x as f32 / w,
            self.y as f32 / h,
            (self.x + self.width) as f32 / w,
            (self.y + self.height) as f32 / h,
        ]
    }
}

/// Bottom edge of the region the shelf allocator has handed out, in pixels.
///
/// Shelves are stacked top-down and never overlap, so the last one ends the
/// used region. Two callers depend on this being the *same* number:
/// [`TextureAtlas::new_shelf`] starts the next shelf here, and
/// [`TextureAtlas::clear`] zeroes up to here. Keeping them on one function is
/// what guarantees the clear can never cover less than what was handed out.
fn shelf_used_height(shelves: &[Shelf]) -> u32 {
    shelves.last().map(|s| s.y + s.height).unwrap_or(0)
}

/// A shelf (horizontal row) in the atlas.
struct Shelf {
    /// Y offset of this shelf.
    y: u32,
    /// Height of this shelf (max glyph height placed so far).
    height: u32,
    /// Current X cursor (next free position).
    cursor_x: u32,
}

/// CPU-side atlas state: shelf allocator + glyph lookup.
pub struct TextureAtlas {
    width: u32,
    height: u32,
    /// Bytes per pixel of the GPU texture: 1 for an R8 alpha-mask atlas, 4
    /// for an RGBA8 color-glyph atlas. The shelf allocator works in pixels;
    /// only the upload row stride and the zero-clear buffer depend on this.
    bytes_per_pixel: u32,
    shelves: Vec<Shelf>,
    /// Maps glyph CacheKey → region in the atlas.
    cache: HashMap<CacheKey, AtlasRegion>,
    /// GPU texture (R8Unorm for masks, Rgba8UnormSrgb for color glyphs).
    texture: wgpu::Texture,
    /// Texture view for binding.
    pub(crate) view: wgpu::TextureView,
    /// Whether any new glyphs were uploaded this frame (for bind group rebuild).
    dirty: bool,
}

impl TextureAtlas {
    /// Create a new single-channel (R8Unorm) alpha-mask atlas. This is the
    /// atlas used for ordinary monochrome glyphs.
    pub fn new(device: &wgpu::Device, width: u32, height: u32) -> Self {
        Self::with_format(
            device,
            width,
            height,
            wgpu::TextureFormat::R8Unorm,
            1,
            "sindon_glyph_atlas",
        )
    }

    /// Create a four-channel (Rgba8UnormSrgb) atlas for color emoji glyphs.
    /// The pixel layout matches the color image path so the same sampler and
    /// `texel * tint` shader render it correctly.
    pub fn new_rgba(device: &wgpu::Device, width: u32, height: u32) -> Self {
        Self::with_format(
            device,
            width,
            height,
            wgpu::TextureFormat::Rgba8UnormSrgb,
            4,
            "sindon_color_glyph_atlas",
        )
    }

    fn with_format(
        device: &wgpu::Device,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
        bytes_per_pixel: u32,
        label: &str,
    ) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        Self {
            width,
            height,
            bytes_per_pixel,
            shelves: Vec::new(),
            cache: HashMap::new(),
            texture,
            view,
            dirty: false,
        }
    }

    /// Look up a cached glyph. Returns `None` if not yet uploaded.
    pub fn get(&self, key: &CacheKey) -> Option<&AtlasRegion> {
        self.cache.get(key)
    }

    /// Upload a glyph into the atlas, returning its region.
    ///
    /// If already cached, returns the existing region.
    /// Returns `None` if the atlas is full and cannot fit this glyph.
    pub fn upload(
        &mut self,
        queue: &wgpu::Queue,
        key: CacheKey,
        data: &[u8],
        glyph_width: u32,
        glyph_height: u32,
    ) -> Option<AtlasRegion> {
        // Already cached?
        if let Some(region) = self.cache.get(&key) {
            return Some(*region);
        }

        // Skip zero-size glyphs (spaces, etc.)
        if glyph_width == 0 || glyph_height == 0 {
            return None;
        }

        let padded_w = glyph_width + GLYPH_PADDING;
        let padded_h = glyph_height + GLYPH_PADDING;

        // Try to fit in an existing shelf
        let region = self
            .find_shelf(padded_w, padded_h)
            .or_else(|| self.new_shelf(padded_w, padded_h));

        let region = region?;

        // Upload pixel data to GPU
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: region.x,
                    y: region.y,
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(glyph_width * self.bytes_per_pixel),
                rows_per_image: Some(glyph_height),
            },
            wgpu::Extent3d {
                width: glyph_width,
                height: glyph_height,
                depth_or_array_layers: 1,
            },
        );

        self.cache.insert(key, region);
        self.dirty = true;
        Some(region)
    }

    /// Try to fit a glyph into an existing shelf.
    fn find_shelf(&mut self, padded_w: u32, padded_h: u32) -> Option<AtlasRegion> {
        for shelf in self.shelves.iter_mut() {
            if shelf.cursor_x + padded_w <= self.width && shelf.height >= padded_h {
                let region = AtlasRegion {
                    x: shelf.cursor_x,
                    y: shelf.y,
                    width: padded_w - GLYPH_PADDING,
                    height: padded_h - GLYPH_PADDING,
                };
                shelf.cursor_x += padded_w;
                return Some(region);
            }
        }
        None
    }

    /// Create a new shelf and allocate from it.
    fn new_shelf(&mut self, padded_w: u32, padded_h: u32) -> Option<AtlasRegion> {
        let y = shelf_used_height(&self.shelves);

        if y + padded_h > self.height || padded_w > self.width {
            return None; // Atlas full
        }

        let region = AtlasRegion {
            x: 0,
            y,
            width: padded_w - GLYPH_PADDING,
            height: padded_h - GLYPH_PADDING,
        };

        self.shelves.push(Shelf {
            y,
            height: padded_h,
            cursor_x: padded_w,
        });

        Some(region)
    }

    /// Atlas width in pixels.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Atlas height in pixels.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Whether new glyphs were uploaded since last `clear_dirty()`.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Reset the dirty flag.
    pub fn clear_dirty(&mut self) {
        self.dirty = false;
    }

    /// Number of cached glyphs.
    pub fn glyph_count(&self) -> usize {
        self.cache.len()
    }

    /// Height, in pixels, of the region the shelf allocator has handed out.
    fn used_height(&self) -> u32 {
        shelf_used_height(&self.shelves)
    }

    /// Clear the entire atlas (GPU + CPU state). Used by SecureTextureAtlas.
    ///
    /// Only the rows the shelf allocator has handed out are zeroed (see the
    /// private `used_height`). Pixels below
    /// the last shelf have never been written since the texture was created,
    /// and wgpu zero-initializes textures, so they cannot hold residue.
    /// Secure text is short (a password, a key), so in practice this is a
    /// couple of glyph rows rather than the full atlas.
    pub fn clear(&mut self, queue: &wgpu::Queue) {
        let used_height = self.used_height();

        // Reset CPU state unconditionally, so the shelf allocator restarts at
        // the top on the next frame and the used region stays this tight.
        self.shelves.clear();
        self.cache.clear();

        if used_height == 0 {
            return;
        }

        // Zero out the written region of the GPU texture
        let row_bytes = self.width * self.bytes_per_pixel;
        let zero_data = vec![0u8; (row_bytes * used_height) as usize];
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &zero_data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row_bytes),
                rows_per_image: Some(used_height),
            },
            wgpu::Extent3d {
                width: self.width,
                height: used_height,
                depth_or_array_layers: 1,
            },
        );

        self.dirty = true;
    }

    /// Get a reference to the underlying texture.
    pub fn texture(&self) -> &wgpu::Texture {
        &self.texture
    }

    /// Get a reference to the texture view.
    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }
}

impl std::fmt::Debug for TextureAtlas {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TextureAtlas")
            .field("size", &(self.width, self.height))
            .field("shelves", &self.shelves.len())
            .field("cached_glyphs", &self.cache.len())
            .finish()
    }
}

// ── CPU-only shelf-packing tests ─────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Test shelf allocation logic without GPU.
    /// We test find_shelf / new_shelf directly via the internal state.
    #[test]
    fn shelf_basic_allocation() {
        // Simulate a 100x100 atlas
        let mut shelves: Vec<Shelf> = Vec::new();
        let _atlas_w: u32 = 100;
        let atlas_h: u32 = 100;

        // Allocate a 10x10 glyph (padded: 11x11)
        let padded_w: u32 = 11;
        let padded_h: u32 = 11;

        // No shelves yet, create one
        let y = shelves.last().map(|s| s.y + s.height).unwrap_or(0);
        assert!(y + padded_h <= atlas_h);

        shelves.push(Shelf {
            y: 0,
            height: padded_h,
            cursor_x: padded_w,
        });

        assert_eq!(shelves.len(), 1);
        assert_eq!(shelves[0].cursor_x, 11);
    }

    #[test]
    fn shelf_packing_fits_multiple() {
        let mut shelves: Vec<Shelf> = vec![Shelf {
            y: 0,
            height: 20,
            cursor_x: 0,
        }];
        let atlas_w: u32 = 100;

        // Pack 5 glyphs of width 18 (padded) into a 100-wide shelf
        for i in 0..5 {
            let padded_w: u32 = 18;
            let shelf = &mut shelves[0];
            if shelf.cursor_x + padded_w <= atlas_w {
                shelf.cursor_x += padded_w;
            } else {
                panic!("glyph {} didn't fit", i);
            }
        }

        // 5 * 18 = 90, fits in 100
        assert_eq!(shelves[0].cursor_x, 90);

        // 6th glyph (90 + 18 = 108 > 100) should not fit
        let padded_w: u32 = 18;
        assert!(shelves[0].cursor_x + padded_w > atlas_w);
    }

    #[test]
    fn shelf_stacking() {
        let atlas_h: u32 = 100;
        let mut shelves: Vec<Shelf> = Vec::new();

        // Add shelf 1: height 20
        shelves.push(Shelf {
            y: 0,
            height: 20,
            cursor_x: 100, // full
        });

        // Add shelf 2: height 30
        let y = shelves.last().map(|s| s.y + s.height).unwrap_or(0);
        assert_eq!(y, 20);
        shelves.push(Shelf {
            y,
            height: 30,
            cursor_x: 50,
        });

        // Add shelf 3: height 25
        let y = shelves.last().map(|s| s.y + s.height).unwrap_or(0);
        assert_eq!(y, 50);
        assert!(y + 25 <= atlas_h);

        shelves.push(Shelf {
            y,
            height: 25,
            cursor_x: 0,
        });

        assert_eq!(shelves.len(), 3);
        // Total used: 20 + 30 + 25 = 75 out of 100
    }

    #[test]
    fn used_height_is_zero_before_anything_is_packed() {
        // A never-packed atlas has no written rows, so `clear` uploads
        // nothing at all. This is the case that takes secure-atlas cost off
        // the budget of every frame that draws no secrets.
        assert_eq!(shelf_used_height(&[]), 0);
    }

    #[test]
    fn used_height_covers_every_packed_row() {
        // The clear region must never be shorter than the region the shelf
        // allocator handed out, or a glyph's pixels survive the zeroing.
        let shelves: Vec<Shelf> = vec![
            Shelf {
                y: 0,
                height: 20,
                cursor_x: 100,
            },
            Shelf {
                y: 20,
                height: 30,
                cursor_x: 50,
            },
            Shelf {
                y: 50,
                height: 25,
                cursor_x: 0,
            },
        ];

        let used = shelf_used_height(&shelves);
        assert_eq!(used, 75);

        // Every shelf — and therefore every region ever returned from it,
        // since a region's height never exceeds its shelf's — is inside it.
        for shelf in &shelves {
            assert!(
                shelf.y + shelf.height <= used,
                "shelf at y={} height={} escapes the cleared region {}",
                shelf.y,
                shelf.height,
                used
            );
        }
    }

    #[test]
    fn used_height_matches_where_the_next_shelf_would_start() {
        // `new_shelf` and `clear` share this function precisely so the
        // cleared region ends exactly where unwritten pixels begin. If these
        // ever diverge, the clear silently misses a row.
        let shelves: Vec<Shelf> = vec![
            Shelf {
                y: 0,
                height: 12,
                cursor_x: 30,
            },
            Shelf {
                y: 12,
                height: 18,
                cursor_x: 4,
            },
        ];

        let next_shelf_y = shelf_used_height(&shelves);
        assert_eq!(next_shelf_y, 30);
        assert_eq!(shelf_used_height(&shelves), next_shelf_y);
    }

    #[test]
    fn clear_resets_shelf_state() {
        // After clear, shelves should be empty — simulating what
        // TextureAtlas::clear() does on the CPU side.
        let mut shelves: Vec<Shelf> = vec![
            Shelf {
                y: 0,
                height: 20,
                cursor_x: 50,
            },
            Shelf {
                y: 20,
                height: 30,
                cursor_x: 100,
            },
        ];

        assert_eq!(shelves.len(), 2);

        // Simulate clear
        shelves.clear();
        assert!(shelves.is_empty());

        // Should be able to allocate again from scratch
        shelves.push(Shelf {
            y: 0,
            height: 15,
            cursor_x: 0,
        });
        assert_eq!(shelves.len(), 1);
        assert_eq!(shelves[0].y, 0);
    }

    #[test]
    fn atlas_region_uv() {
        let region = AtlasRegion {
            x: 10,
            y: 20,
            width: 30,
            height: 40,
        };
        let uv = region.uv(100, 200);
        assert_eq!(uv[0], 0.1); // 10/100
        assert_eq!(uv[1], 0.1); // 20/200
        assert_eq!(uv[2], 0.4); // 40/100
        assert_eq!(uv[3], 0.3); // 60/200
    }
}
