//! Image widget — paints a decoded raster into a layout box.
//!
//! Construct from PNG/JPEG bytes via [`Image::from_bytes`]. The decoded
//! pixels are wrapped in an [`Arc<DecodedImage>`] (cheap to clone) and
//! the renderer uploads each unique image to GPU exactly once via a
//! content-hash cache.
//!
//! # Sizing & fit
//!
//! The widget participates in flex layout with its intrinsic decoded
//! dimensions as a hint:
//!
//! - `.width(px)` and `.height(px)` pin either or both sides; pinning
//!   one alone derives the other from the aspect ratio of the decoded
//!   image.
//! - `.fit(ImageFit::…)` chooses how pixels map into the laid-out box
//!   when the box and the image's aspect ratio disagree. Default is
//!   [`ImageFit::Contain`].
//!
//! # Tint
//!
//! `.tint(color)` multiplies the sampled RGBA. The default tint
//! ([`Color::WHITE`]) leaves pixels untouched. Use the alpha component
//! to fade in/out without re-uploading texels.

use std::sync::Arc;

use crate::paint::PaintContext;
use crate::widget::{MeasureContext, Widget};
use shroud_core::{Color, Rect, Size};
use shroud_layout::FlexStyle;
use shroud_render::{DecodedImage, ImageError};

/// How an image's intrinsic pixels map into the laid-out box.
///
/// Mirrors CSS `object-fit`. Source pixels are never resampled to a
/// different image — the GPU does bilinear sampling at draw time, so
/// "fit" only affects the destination rect.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum ImageFit {
    /// Scale uniformly to fit inside the box, preserving aspect ratio.
    /// Leaves letterbox bars when the aspect ratios differ. Default.
    #[default]
    Contain,
    /// Scale uniformly to cover the box, preserving aspect ratio. Crops
    /// the overflowing axis (paint pushes a clip equal to the layout
    /// rect so the overflow is invisible).
    Cover,
    /// Stretch to fill the box exactly. Does not preserve aspect ratio.
    Fill,
    /// Use intrinsic decoded dimensions, centered. Clips to the layout
    /// rect if the image is larger than the box.
    None,
}

/// Image widget. Construct with [`Image::from_bytes`].
pub struct Image {
    image: Arc<DecodedImage>,
    width: Option<f32>,
    height: Option<f32>,
    fit: ImageFit,
    tint: Color,
}

impl Image {
    /// Decode PNG or JPEG bytes and build a widget around the result.
    ///
    /// Forwards decode errors from
    /// [`DecodedImage::from_bytes`](shroud_render::DecodedImage::from_bytes)
    /// so callers can branch on bad asset bytes without panicking.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ImageError> {
        let image = DecodedImage::from_bytes(bytes)?;
        Ok(Self::from_decoded(image))
    }

    /// Build a widget around an already-decoded image. Useful when one
    /// asset is shared across many widget instances — the `Arc` clone
    /// is cheap and dedup happens automatically in the renderer.
    pub fn from_decoded(image: Arc<DecodedImage>) -> Self {
        Self {
            image,
            width: None,
            height: None,
            fit: ImageFit::default(),
            tint: Color::WHITE,
        }
    }

    /// Pin the rendered width (pixels). The height is derived from the
    /// aspect ratio if not also pinned.
    pub fn width(mut self, px: f32) -> Self {
        self.width = Some(px);
        self
    }

    /// Pin the rendered height (pixels). The width is derived from the
    /// aspect ratio if not also pinned.
    pub fn height(mut self, px: f32) -> Self {
        self.height = Some(px);
        self
    }

    /// Choose how the image scales into the laid-out box. Default is
    /// [`ImageFit::Contain`].
    pub fn fit(mut self, fit: ImageFit) -> Self {
        self.fit = fit;
        self
    }

    /// Multiply sampled RGBA by `tint`. Use [`Color::WHITE`] (the
    /// default) for unmodified pixels; use `tint.a < 1.0` for a fade.
    pub fn tint(mut self, tint: Color) -> Self {
        self.tint = tint;
        self
    }

    /// Intrinsic decoded size in pixels, as `(width, height)`.
    pub fn intrinsic_size(&self) -> (f32, f32) {
        (self.image.width() as f32, self.image.height() as f32)
    }

    /// Aspect ratio (width / height) of the decoded image. Returns 1.0
    /// for the pathological 0-pixel case to avoid divide-by-zero
    /// downstream — `from_bytes` already rejects empty input, so this
    /// is just defense in depth.
    fn aspect(&self) -> f32 {
        let (w, h) = self.intrinsic_size();
        if h == 0.0 { 1.0 } else { w / h }
    }

    /// Resolve the size the widget reports to layout. Honors explicit
    /// pins; derives the missing axis from the aspect ratio when only
    /// one is pinned; falls back to intrinsic decoded size otherwise.
    fn resolved_size(&self) -> Size {
        let (iw, ih) = self.intrinsic_size();
        match (self.width, self.height) {
            (Some(w), Some(h)) => Size::new(w, h),
            (Some(w), None) => Size::new(w, w / self.aspect()),
            (None, Some(h)) => Size::new(h * self.aspect(), h),
            (None, None) => Size::new(iw, ih),
        }
    }
}

/// Given the laid-out box and a target intrinsic aspect ratio, return
/// the rect inside the box that the image should be drawn into
/// according to `fit`. Plus a flag indicating whether the result can
/// exceed the layout box (in which case the caller must push a clip).
fn fit_rect(layout: Rect, intrinsic: (f32, f32), fit: ImageFit) -> (Rect, bool) {
    let (iw, ih) = intrinsic;
    let (lw, lh) = (layout.size.width, layout.size.height);
    if iw <= 0.0 || ih <= 0.0 || lw <= 0.0 || lh <= 0.0 {
        return (layout, false);
    }
    let ix = layout.origin.x;
    let iy = layout.origin.y;
    let img_ratio = iw / ih;
    let box_ratio = lw / lh;
    match fit {
        ImageFit::Fill => (layout, false),
        ImageFit::Contain => {
            // Scale uniformly to fit. Letterbox along whichever axis
            // doesn't bind.
            let (dw, dh) = if img_ratio > box_ratio {
                (lw, lw / img_ratio)
            } else {
                (lh * img_ratio, lh)
            };
            let dx = ix + (lw - dw) * 0.5;
            let dy = iy + (lh - dh) * 0.5;
            (Rect::new(dx, dy, dw, dh), false)
        }
        ImageFit::Cover => {
            // Scale uniformly to cover. The overflowing axis bleeds
            // outside the layout rect — caller clips.
            let (dw, dh) = if img_ratio > box_ratio {
                (lh * img_ratio, lh)
            } else {
                (lw, lw / img_ratio)
            };
            let dx = ix + (lw - dw) * 0.5;
            let dy = iy + (lh - dh) * 0.5;
            (Rect::new(dx, dy, dw, dh), true)
        }
        ImageFit::None => {
            // Center at intrinsic size; clip if larger than the box.
            let dx = ix + (lw - iw) * 0.5;
            let dy = iy + (lh - ih) * 0.5;
            let needs_clip = iw > lw || ih > lh;
            (Rect::new(dx, dy, iw, ih), needs_clip)
        }
    }
}

impl Widget for Image {
    fn style(&self) -> FlexStyle {
        let Size { width, height, .. } = self.resolved_size();
        // Pin both axes so flex doesn't shrink the image below its
        // intrinsic / requested size. Callers that want a flexible
        // image can wrap in a Container with their own size knobs and
        // a single explicit `.width()` / `.height()` on the image.
        FlexStyle::new().width(width).height(height)
    }

    fn measure(&self, _available_width: Option<f32>, _ctx: &mut MeasureContext) -> Option<Size> {
        Some(self.resolved_size())
    }

    fn paint(&self, layout: Rect, ctx: &mut PaintContext) {
        if layout.size.width <= 0.0 || layout.size.height <= 0.0 {
            return;
        }
        let intrinsic = self.intrinsic_size();
        let (dest, needs_clip) = fit_rect(layout, intrinsic, self.fit);
        if needs_clip {
            ctx.push_clip(layout);
        }
        ctx.draw_image(dest, Arc::clone(&self.image), self.tint);
        if needs_clip {
            ctx.pop_clip();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn red_2x4_png() -> Vec<u8> {
        // 2x4 (intentionally non-square so aspect-ratio bugs show up)
        let mut img = image::RgbaImage::new(2, 4);
        for px in img.pixels_mut() {
            *px = image::Rgba([255, 0, 0, 255]);
        }
        let mut out = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }

    #[test]
    fn from_bytes_rejects_garbage() {
        assert!(Image::from_bytes(&[1, 2, 3, 4]).is_err());
    }

    #[test]
    fn intrinsic_size_matches_decoded() {
        let img = Image::from_bytes(&red_2x4_png()).unwrap();
        assert_eq!(img.intrinsic_size(), (2.0, 4.0));
    }

    #[test]
    fn pinned_width_derives_height_via_aspect() {
        let img = Image::from_bytes(&red_2x4_png()).unwrap().width(20.0);
        let size = img.resolved_size();
        assert_eq!(size.width, 20.0);
        assert!(
            (size.height - 40.0).abs() < 0.001,
            "expected height 40, got {}",
            size.height
        );
    }

    #[test]
    fn pinned_height_derives_width_via_aspect() {
        let img = Image::from_bytes(&red_2x4_png()).unwrap().height(40.0);
        let size = img.resolved_size();
        assert!(
            (size.width - 20.0).abs() < 0.001,
            "expected width 20, got {}",
            size.width
        );
        assert_eq!(size.height, 40.0);
    }

    #[test]
    fn both_pinned_overrides_aspect() {
        let img = Image::from_bytes(&red_2x4_png())
            .unwrap()
            .width(100.0)
            .height(50.0);
        let size = img.resolved_size();
        assert_eq!(size.width, 100.0);
        assert_eq!(size.height, 50.0);
    }

    #[test]
    fn fit_contain_letterboxes_taller_image() {
        // Box is wider (200x100 -> ratio 2.0) than image (2x4 -> 0.5).
        // Contain: scale by 100/4 = 25, dest = 50x100, centered in box.
        let layout = Rect::new(0.0, 0.0, 200.0, 100.0);
        let (dest, clip) = fit_rect(layout, (2.0, 4.0), ImageFit::Contain);
        assert!(!clip);
        assert!((dest.size.width - 50.0).abs() < 0.001);
        assert!((dest.size.height - 100.0).abs() < 0.001);
        assert!((dest.origin.x - 75.0).abs() < 0.001);
        assert!((dest.origin.y - 0.0).abs() < 0.001);
    }

    #[test]
    fn fit_cover_overflows_and_requests_clip() {
        // Same dims; cover scales so the box is fully covered, image
        // bleeds out vertically.
        let layout = Rect::new(0.0, 0.0, 200.0, 100.0);
        let (dest, clip) = fit_rect(layout, (2.0, 4.0), ImageFit::Cover);
        assert!(clip);
        assert!((dest.size.width - 200.0).abs() < 0.001);
        assert!((dest.size.height - 400.0).abs() < 0.001);
    }

    #[test]
    fn fit_fill_uses_layout_verbatim() {
        let layout = Rect::new(10.0, 20.0, 200.0, 100.0);
        let (dest, clip) = fit_rect(layout, (2.0, 4.0), ImageFit::Fill);
        assert!(!clip);
        assert_eq!(dest.origin.x, 10.0);
        assert_eq!(dest.origin.y, 20.0);
        assert_eq!(dest.size.width, 200.0);
        assert_eq!(dest.size.height, 100.0);
    }

    #[test]
    fn fit_none_centers_and_clips_when_oversize() {
        // Image larger than box: needs clip.
        let layout = Rect::new(0.0, 0.0, 10.0, 10.0);
        let (dest, clip) = fit_rect(layout, (20.0, 30.0), ImageFit::None);
        assert!(clip);
        assert_eq!(dest.size.width, 20.0);
        assert_eq!(dest.size.height, 30.0);
        assert_eq!(dest.origin.x, -5.0);
        assert_eq!(dest.origin.y, -10.0);
    }

    #[test]
    fn paint_emits_one_image_command() {
        let img = Image::from_bytes(&red_2x4_png()).unwrap().width(40.0);
        let mut ctx = PaintContext::default();
        img.paint(Rect::new(0.0, 0.0, 40.0, 80.0), &mut ctx);
        assert_eq!(ctx.images.len(), 1);
        assert_eq!(ctx.images[0].width, 40.0);
        assert_eq!(ctx.images[0].height, 80.0);
    }

    #[test]
    fn paint_skips_zero_layout() {
        let img = Image::from_bytes(&red_2x4_png()).unwrap();
        let mut ctx = PaintContext::default();
        img.paint(Rect::new(0.0, 0.0, 0.0, 100.0), &mut ctx);
        assert_eq!(ctx.images.len(), 0);
    }

    #[test]
    fn paint_uses_provided_tint() {
        let img = Image::from_bytes(&red_2x4_png())
            .unwrap()
            .width(40.0)
            .tint(Color::rgba(1.0, 1.0, 1.0, 0.5));
        let mut ctx = PaintContext::default();
        img.paint(Rect::new(0.0, 0.0, 40.0, 80.0), &mut ctx);
        assert_eq!(ctx.images[0].tint.a, 0.5);
    }
}
