//! shroud_render — wgpu-based 2D renderer for shroud.
//!
//! - [`renderer`]: `Renderer` submits rect/glyph draw commands via wgpu
//! - [`atlas`]: `TextureAtlas` for cached glyph bitmaps (non-secret)
//! - [`secure_atlas`]: `SecureTextureAtlas` — cleared each frame so GPU
//!   memory never retains sensitive glyph data between redraws

pub mod atlas;
pub mod image;
pub mod renderer;
pub mod secure_atlas;

pub use atlas::{AtlasRegion, TextureAtlas};
pub use image::{DecodedImage, ImageError, ImageId};
pub use renderer::{DrawGlyph, DrawImage, DrawRect, LayerSnapshot, RenderError, Renderer};
pub use secure_atlas::SecureTextureAtlas;
