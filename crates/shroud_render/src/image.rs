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
}

impl std::fmt::Display for ImageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImageError::Empty => write!(f, "image bytes are empty"),
            ImageError::Decode(s) => write!(f, "image decode error: {s}"),
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
