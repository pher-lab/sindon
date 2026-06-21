//! Image decoding and renderer-facing types.
//!
//! `DecodedImage` holds RGBA8 pixels plus an [`ImageId`] derived from the
//! source bytes (FNV-1a 64-bit). The renderer keeps a GPU texture cache
//! keyed by `ImageId`, so the same bytes upload exactly once across all
//! paints — even when several widgets construct independent
//! `Arc<DecodedImage>` from the same blob.
//!
//! **Security**: image pixels live in plain GPU memory once uploaded. Do
//! not pipe secret pixel data through this path; a secure-image
//! counterpart (modeled on `SecureTextureAtlas`) is out of scope for the
//! initial implementation.

use std::sync::Arc;

use image::ImageReader;

/// Unique identifier for an image, derived from its source bytes.
///
/// Two identical byte streams share an ID and thus a single GPU texture.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct ImageId(pub u64);

/// Failure modes for [`DecodedImage::from_bytes`].
#[derive(Debug)]
pub enum ImageError {
    /// Input slice was empty.
    Empty,
    /// `image` crate rejected the bytes (unsupported format, corrupt
    /// header, truncated data, …). The wrapped string is the upstream
    /// `Display` for diagnostics.
    Decode(String),
    /// Encoding raw pixels to a container format failed — either the RGBA
    /// buffer length didn't match `width * height * 4`, or the `image`
    /// crate's encoder errored. The wrapped string is for diagnostics.
    Encode(String),
}

impl std::fmt::Display for ImageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImageError::Empty => write!(f, "image bytes are empty"),
            ImageError::Decode(s) => write!(f, "image decode error: {s}"),
            ImageError::Encode(s) => write!(f, "image encode error: {s}"),
        }
    }
}

impl std::error::Error for ImageError {}

/// CPU-side decoded image. Holds tightly-packed RGBA8 pixels and the
/// source dimensions, plus an [`ImageId`] for GPU cache dedup.
///
/// Construct via [`DecodedImage::from_bytes`]. The returned `Arc` is the
/// canonical handle that widgets store and that paint commands carry.
pub struct DecodedImage {
    id: ImageId,
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

impl DecodedImage {
    /// Decode PNG or JPEG bytes into an `Arc<DecodedImage>`.
    ///
    /// Format is auto-detected from the byte content. Empty input is
    /// rejected up front so callers don't have to special-case it.
    pub fn from_bytes(bytes: &[u8]) -> Result<Arc<Self>, ImageError> {
        if bytes.is_empty() {
            return Err(ImageError::Empty);
        }
        let id = ImageId(fnv1a_64(bytes));
        let reader = ImageReader::new(std::io::Cursor::new(bytes))
            .with_guessed_format()
            .map_err(|e| ImageError::Decode(e.to_string()))?;
        let dynamic = reader
            .decode()
            .map_err(|e| ImageError::Decode(e.to_string()))?;
        let rgba_img = dynamic.to_rgba8();
        let width = rgba_img.width();
        let height = rgba_img.height();
        Ok(Arc::new(Self {
            id,
            width,
            height,
            rgba: rgba_img.into_raw(),
        }))
    }

    pub fn id(&self) -> ImageId {
        self.id
    }
    pub fn width(&self) -> u32 {
        self.width
    }
    pub fn height(&self) -> u32 {
        self.height
    }
    pub fn rgba(&self) -> &[u8] {
        &self.rgba
    }
}

/// Encode tightly-packed RGBA8 pixels into PNG bytes.
///
/// The inverse of [`DecodedImage::from_bytes`]'s decode step: it turns the
/// raw RGBA a clipboard image or screenshot arrives as into a
/// self-describing PNG that can be stored or fed back through
/// `from_bytes`. `rgba` must be exactly `width * height * 4` bytes.
///
/// Co-located with the decoder so the framework owns one image codec; the
/// platform clipboard stays free of the `image` dependency and hands over
/// raw pixels for callers to encode here.
pub fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, ImageError> {
    let buf = image::RgbaImage::from_raw(width, height, rgba.to_vec()).ok_or_else(|| {
        ImageError::Encode(format!(
            "rgba buffer is {} bytes, expected {} for {width}x{height}",
            rgba.len(),
            (width as usize) * (height as usize) * 4,
        ))
    })?;
    let mut out = Vec::new();
    buf.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .map_err(|e| ImageError::Encode(e.to_string()))?;
    Ok(out)
}

/// FNV-1a 64-bit. Fixed seed → deterministic across processes (unlike
/// `DefaultHasher`), which means the cache stays warm when the same
/// asset is decoded twice from independent owners.
fn fnv1a_64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// One downsampled mip level: dimensions plus tightly-packed RGBA8.
pub(crate) struct MipLevel {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// sRGB-encoded channel byte → linear-light float in `[0, 1]`.
#[inline]
fn srgb_to_linear(c: u8) -> f32 {
    let s = c as f32 / 255.0;
    if s <= 0.040_45 {
        s / 12.92
    } else {
        ((s + 0.055) / 1.055).powf(2.4)
    }
}

/// Linear-light float → sRGB-encoded channel byte.
#[inline]
fn linear_to_srgb(l: f32) -> u8 {
    let l = l.clamp(0.0, 1.0);
    let s = if l <= 0.003_130_8 {
        l * 12.92
    } else {
        1.055 * l.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0).round().clamp(0.0, 255.0) as u8
}

/// Halve `src` (RGBA8, `sw`×`sh`) to a new buffer, averaging each 2×2
/// source block in **premultiplied linear-light** space. Premultiplying
/// keeps partially-transparent edges from bleeding the (undefined) color
/// of fully-transparent texels into their neighbours; filtering in linear
/// light matches what the GPU sampler does at draw time on the
/// `Rgba8UnormSrgb` texture, so brightness stays consistent across levels.
/// Odd dimensions clamp the trailing sample to the last in-bounds texel.
fn downsample_half(src: &[u8], sw: u32, sh: u32) -> (u32, u32, Vec<u8>) {
    let dw = (sw / 2).max(1);
    let dh = (sh / 2).max(1);
    let mut dst = vec![0u8; (dw as usize) * (dh as usize) * 4];
    for dy in 0..dh {
        let sy0 = dy * 2;
        let sy1 = (sy0 + 1).min(sh - 1);
        for dx in 0..dw {
            let sx0 = dx * 2;
            let sx1 = (sx0 + 1).min(sw - 1);
            let (mut r, mut g, mut b, mut a) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
            for &(x, y) in &[(sx0, sy0), (sx1, sy0), (sx0, sy1), (sx1, sy1)] {
                let i = ((y * sw + x) as usize) * 4;
                let af = src[i + 3] as f32 / 255.0;
                r += srgb_to_linear(src[i]) * af;
                g += srgb_to_linear(src[i + 1]) * af;
                b += srgb_to_linear(src[i + 2]) * af;
                a += af;
            }
            let (mut rl, mut gl, mut bl, al) = (r * 0.25, g * 0.25, b * 0.25, a * 0.25);
            if al > 0.0 {
                // Un-premultiply back to straight-alpha for the u8 store.
                rl /= al;
                gl /= al;
                bl /= al;
            }
            let o = ((dy * dw + dx) as usize) * 4;
            dst[o] = linear_to_srgb(rl);
            dst[o + 1] = linear_to_srgb(gl);
            dst[o + 2] = linear_to_srgb(bl);
            dst[o + 3] = (al * 255.0).round().clamp(0.0, 255.0) as u8;
        }
    }
    (dw, dh, dst)
}

/// Build mip levels `1..N` for an RGBA8 image, halving until 1×1. Level 0
/// (the original) is **not** included — the caller already has it.
///
/// Without a mip chain, the GPU's bilinear minification averages only a
/// 2×2 texel neighbourhood per output pixel, so an image drawn much
/// smaller than its decoded size aliases badly (the "rough" look). With
/// the chain uploaded and a `Linear` mipmap filter, the sampler trilinearly
/// blends the appropriately pre-filtered level instead. Generated lazily at
/// upload time (once per unique `ImageId`) and dropped after the texels
/// reach the GPU, so no extra plaintext copy lingers in CPU memory.
pub(crate) fn build_mip_chain(rgba0: &[u8], width: u32, height: u32) -> Vec<MipLevel> {
    let mut levels: Vec<MipLevel> = Vec::new();
    if width == 0 || height == 0 {
        return levels;
    }
    let (mut w, mut h) = (width, height);
    while w > 1 || h > 1 {
        // The first step reads the borrowed original; later steps read the
        // previously generated level. The borrow ends when `downsample_half`
        // returns, so the subsequent `push` is unobstructed.
        let src: &[u8] = match levels.last() {
            Some(prev) => &prev.rgba,
            None => rgba0,
        };
        let (nw, nh, data) = downsample_half(src, w, h);
        w = nw;
        h = nh;
        levels.push(MipLevel {
            width: w,
            height: h,
            rgba: data,
        });
    }
    levels
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Smallest valid 1x1 RGBA PNG (red).
    /// Generated with `image` once and embedded here so tests don't need
    /// disk I/O.
    fn red_1x1_png() -> Vec<u8> {
        let mut img = image::RgbaImage::new(1, 1);
        img.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));
        let mut out = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }

    #[test]
    fn decode_rejects_empty() {
        assert!(matches!(
            DecodedImage::from_bytes(&[]),
            Err(ImageError::Empty)
        ));
    }

    #[test]
    fn decode_rejects_garbage() {
        assert!(matches!(
            DecodedImage::from_bytes(&[0x00, 0x01, 0x02, 0x03, 0x04]),
            Err(ImageError::Decode(_))
        ));
    }

    #[test]
    fn decode_round_trip_png() {
        let bytes = red_1x1_png();
        let img = DecodedImage::from_bytes(&bytes).unwrap();
        assert_eq!(img.width(), 1);
        assert_eq!(img.height(), 1);
        assert_eq!(img.rgba(), &[255, 0, 0, 255]);
    }

    #[test]
    fn same_bytes_same_id() {
        let bytes = red_1x1_png();
        let a = DecodedImage::from_bytes(&bytes).unwrap();
        let b = DecodedImage::from_bytes(&bytes).unwrap();
        assert_eq!(a.id(), b.id(), "content hash must be deterministic");
    }

    #[test]
    fn encode_png_round_trips_through_decode() {
        // A 2x1 image: one red pixel, one green pixel. Encoding to PNG and
        // decoding back must recover the exact pixels and dimensions — the
        // clipboard-paste path relies on this round-trip.
        let rgba = vec![255, 0, 0, 255, 0, 255, 0, 255];
        let png = encode_png(2, 1, &rgba).unwrap();
        let img = DecodedImage::from_bytes(&png).unwrap();
        assert_eq!(img.width(), 2);
        assert_eq!(img.height(), 1);
        assert_eq!(img.rgba(), rgba.as_slice());
    }

    #[test]
    fn encode_png_rejects_mismatched_buffer() {
        // 3 bytes can't be a 1x1 RGBA pixel (needs 4) — caught before the
        // encoder runs so a malformed clipboard payload is a clean error,
        // not a panic.
        assert!(matches!(
            encode_png(1, 1, &[0, 0, 0]),
            Err(ImageError::Encode(_))
        ));
    }

    #[test]
    fn mip_chain_single_pixel_is_empty() {
        // 1×1 already minified as far as it goes — no extra levels.
        assert!(build_mip_chain(&[10, 20, 30, 255], 1, 1).is_empty());
    }

    #[test]
    fn mip_chain_halves_to_one_by_one() {
        // 4×4 → 2×2 → 1×1: two generated levels with halving dimensions.
        let rgba = vec![128u8; 4 * 4 * 4];
        let levels = build_mip_chain(&rgba, 4, 4);
        let dims: Vec<(u32, u32)> = levels.iter().map(|m| (m.width, m.height)).collect();
        assert_eq!(dims, vec![(2, 2), (1, 1)]);
        // Each level is tightly packed RGBA8.
        for m in &levels {
            assert_eq!(m.rgba.len(), (m.width as usize) * (m.height as usize) * 4);
        }
    }

    #[test]
    fn mip_chain_non_square_clamps_min_one() {
        // 4×2 → 2×1 → 1×1. The thin axis bottoms out at 1 while the wide
        // axis keeps halving.
        let rgba = vec![200u8; 4 * 2 * 4];
        let levels = build_mip_chain(&rgba, 4, 2);
        let dims: Vec<(u32, u32)> = levels.iter().map(|m| (m.width, m.height)).collect();
        assert_eq!(dims, vec![(2, 1), (1, 1)]);
    }

    #[test]
    fn mip_chain_preserves_solid_color() {
        // Averaging a uniform field returns (within rounding) the same color
        // — the linear round-trip must not drift on a flat input.
        let px = [200u8, 50, 100, 255];
        let mut rgba = Vec::new();
        for _ in 0..(2 * 2) {
            rgba.extend_from_slice(&px);
        }
        let levels = build_mip_chain(&rgba, 2, 2);
        assert_eq!(levels.len(), 1);
        let one = &levels[0];
        for (c, &expected) in px.iter().enumerate() {
            let got = one.rgba[c] as i32;
            assert!(
                (got - expected as i32).abs() <= 1,
                "channel {c}: got {got}, expected ~{expected}",
            );
        }
    }

    #[test]
    fn mip_chain_premultiplied_avoids_color_bleed() {
        // 2×1: a fully-transparent red next to an opaque blue. Filtering in
        // premultiplied space must drop the transparent red's color so the
        // averaged texel is blue (not a muddy purple). Straight-alpha
        // averaging would leak red into the result.
        let rgba = [
            255, 0, 0, 0, /* transparent red */ 0, 0, 255, 255, /* opaque blue */
        ];
        let levels = build_mip_chain(&rgba, 2, 1);
        assert_eq!(levels.len(), 1);
        let one = &levels[0].rgba;
        assert_eq!(one[0], 0, "no red bleed from the transparent texel");
        assert_eq!(one[2], 255, "blue survives un-premultiply");
        assert!(
            (one[3] as i32 - 128).abs() <= 1,
            "alpha is the mean of 0 and 255, got {}",
            one[3]
        );
    }

    #[test]
    fn different_bytes_different_id() {
        let bytes = red_1x1_png();
        let mut other = image::RgbaImage::new(1, 1);
        other.put_pixel(0, 0, image::Rgba([0, 255, 0, 255]));
        let mut other_bytes = Vec::new();
        other
            .write_to(
                &mut std::io::Cursor::new(&mut other_bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        let a = DecodedImage::from_bytes(&bytes).unwrap();
        let b = DecodedImage::from_bytes(&other_bytes).unwrap();
        assert_ne!(a.id(), b.id());
    }
}
