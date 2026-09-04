//! Clipboard image to something small enough to keep forever.
//!
//! Screenshots arrive as multi-megabyte PNGs. What we want is a legible reminder
//! of what was on screen, not an archive-quality copy, so everything is scaled to
//! a 1200px long edge and re-encoded as lossy WebP. A 4K screenshot lands around
//! 80 KB, which is what keeps a decade of daily captures inside a git repo.

use anyhow::{Context, Result};
use image::{imageops::FilterType, DynamicImage, RgbaImage};
use sha2::{Digest, Sha256};

/// Long edge of a stored image. Smaller images are never upscaled.
///
/// Measured on a real 3840x2160 desktop capture: 1200px is 62 KB but its text
/// is no longer readable, which defeats both the point of keeping it and the
/// OCR at M6. 1600px stays legible at 104 KB. 1920px is comfortably sharp at
/// 145 KB. Full-screen 4K is the worst case — a window or region capture is
/// usually below the limit and never scaled at all.
pub const MAX_EDGE: u32 = 1600;
/// Long edge of the grid thumbnail.
pub const THUMB_EDGE: u32 = 320;
/// WebP quality. 75 keeps UI text readable, which matters because OCR reads
/// these later (M6) — the same reason we do not use JPEG, whose ringing around
/// text costs both legibility and OCR accuracy.
pub const QUALITY: f32 = 75.0;

/// Encoding limits, so a user on a different screen can trade size for detail.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Options {
    pub max_edge: u32,
    pub thumb_edge: u32,
    pub quality: f32,
}

impl Default for Options {
    fn default() -> Self {
        Options { max_edge: MAX_EDGE, thumb_edge: THUMB_EDGE, quality: QUALITY }
    }
}

/// Hex characters of the content hash used in filenames.
///
/// 12 gives 48 bits. Identical images intentionally collide and dedupe; what
/// must not happen is two *different* images sharing a name and one silently
/// standing in for the other. Six characters, at 24 bits, reaches a coin-flip
/// chance of that within a few thousand images.
const HASH_LEN: usize = 12;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attachment {
    /// Content hash of the source image, before any scaling or encoding, so the
    /// same screenshot pasted twice always yields the same name.
    pub hash: String,
    pub webp: Vec<u8>,
    pub thumb: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// Process raw RGBA pixels, as handed over by a clipboard.
pub fn from_rgba(width: u32, height: u32, rgba: &[u8], opts: Options) -> Result<Attachment> {
    let expected = width as usize * height as usize * 4;
    anyhow::ensure!(
        rgba.len() == expected,
        "clipboard image is {} bytes, expected {expected} for {width}x{height}",
        rgba.len()
    );
    let img = RgbaImage::from_raw(width, height, rgba.to_vec())
        .context("clipboard image dimensions do not match its data")?;
    Ok(build(DynamicImage::ImageRgba8(img), &hash_of(width, height, rgba), opts))
}

/// Width and height of a stored WebP, without keeping the decoded pixels.
///
/// The image window is sized to its picture, so it needs this before it opens.
pub fn dimensions(webp: &[u8]) -> Option<(u32, u32)> {
    let decoded = webp::Decoder::new(webp).decode()?;
    Some((decoded.width(), decoded.height()))
}

/// Process an already-encoded image, as delivered by a webview paste event.
pub fn from_encoded(bytes: &[u8], opts: Options) -> Result<Attachment> {
    let img = image::load_from_memory(bytes).context("decoding pasted image")?;
    let rgba = img.to_rgba8();
    let hash = hash_of(rgba.width(), rgba.height(), rgba.as_raw());
    Ok(build(img, &hash, opts))
}

fn build(img: DynamicImage, hash: &str, opts: Options) -> Attachment {
    let full = fit(&img, opts.max_edge);
    let thumb = fit(&img, opts.thumb_edge);
    Attachment {
        hash: hash.to_string(),
        webp: encode(&full, opts.quality),
        thumb: encode(&thumb, opts.quality),
        width: full.width(),
        height: full.height(),
    }
}

/// Scale so the long edge is at most `edge`, preserving aspect ratio. Never
/// upscales: a small image is already as good as it will get.
fn fit(img: &DynamicImage, edge: u32) -> DynamicImage {
    if img.width() <= edge && img.height() <= edge {
        return img.clone();
    }
    img.resize(edge, edge, FilterType::Lanczos3)
}

fn encode(img: &DynamicImage, quality: f32) -> Vec<u8> {
    let rgba = img.to_rgba8();
    webp::Encoder::from_rgba(rgba.as_raw(), rgba.width(), rgba.height())
        .encode(quality)
        .to_vec()
}

fn hash_of(width: u32, height: u32, rgba: &[u8]) -> String {
    let mut hasher = Sha256::new();
    // Dimensions are hashed too, so two different images that happen to share a
    // pixel buffer length cannot collide.
    hasher.update(width.to_le_bytes());
    hasher.update(height.to_le_bytes());
    hasher.update(rgba);
    hex(&hasher.finalize()[..])
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().take(HASH_LEN.div_ceil(2)).map(|b| format!("{b:02x}")).collect::<String>()[..HASH_LEN]
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A gradient rather than a flat fill: flat colours compress to almost
    /// nothing and would make the size assertions meaningless.
    fn gradient(w: u32, h: u32) -> Vec<u8> {
        let mut px = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                px.extend_from_slice(&[(x % 256) as u8, (y % 256) as u8, ((x + y) % 256) as u8, 255]);
            }
        }
        px
    }

    #[test]
    fn downscales_to_the_long_edge_keeping_aspect() {
        let (w, h) = (3840, 2160);
        let a = from_rgba(w, h, &gradient(w, h), Options::default()).unwrap();
        assert_eq!(a.width, MAX_EDGE);
        assert_eq!(a.height, 900); // 2160 * 1600/3840
    }

    #[test]
    fn never_upscales_a_small_image() {
        let a = from_rgba(64, 48, &gradient(64, 48), Options::default()).unwrap();
        assert_eq!((a.width, a.height), (64, 48));
    }

    #[test]
    fn a_4k_screenshot_stays_small_enough_to_keep_forever() {
        let (w, h) = (3840, 2160);
        let a = from_rgba(w, h, &gradient(w, h), Options::default()).unwrap();
        // The whole storage argument in DESIGN.md rests on this staying true.
        assert!(a.webp.len() < 400_000, "full image was {} bytes", a.webp.len());
        assert!(a.thumb.len() < 30_000, "thumbnail was {} bytes", a.thumb.len());
    }

    #[test]
    fn output_is_valid_webp() {
        let a = from_rgba(200, 100, &gradient(200, 100), Options::default()).unwrap();
        let decoded = webp::Decoder::new(&a.webp).decode().expect("decodes");
        assert_eq!((decoded.width(), decoded.height()), (200, 100));
    }

    #[test]
    fn reads_back_the_stored_dimensions() {
        let a = from_rgba(2400, 1200, &gradient(2400, 1200), Options::default()).unwrap();
        assert_eq!(dimensions(&a.webp), Some((a.width, a.height)));
        assert_eq!(dimensions(b"not a webp"), None);
    }

    #[test]
    fn the_same_image_always_hashes_the_same_and_differs_from_others() {
        let a = from_rgba(64, 64, &gradient(64, 64), Options::default()).unwrap();
        let b = from_rgba(64, 64, &gradient(64, 64), Options::default()).unwrap();
        let c = from_rgba(65, 64, &gradient(65, 64), Options::default()).unwrap();

        assert_eq!(a.hash, b.hash, "identical images must dedupe");
        assert_ne!(a.hash, c.hash);
        assert_eq!(a.hash.len(), HASH_LEN);
        assert!(a.hash.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn encoded_and_raw_paths_agree() {
        let px = gradient(120, 90);
        let raw = from_rgba(120, 90, &px, Options::default()).unwrap();

        let mut png = std::io::Cursor::new(Vec::new());
        image::RgbaImage::from_raw(120, 90, px.clone())
            .unwrap()
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        let encoded = from_encoded(png.get_ref(), Options::default()).unwrap();

        // Same pixels via either door must produce the same attachment.
        assert_eq!(raw.hash, encoded.hash);
        assert_eq!(raw.webp, encoded.webp);
    }

    #[test]
    fn options_trade_size_for_detail() {
        let (w, h) = (2400, 1200);
        let px = gradient(w, h);
        let small = from_rgba(w, h, &px, Options { max_edge: 800, ..Options::default() }).unwrap();
        let large = from_rgba(w, h, &px, Options::default()).unwrap();

        assert_eq!(small.width, 800);
        assert!(small.webp.len() < large.webp.len());
        // Scaling must not change identity: it is the source that is hashed.
        assert_eq!(small.hash, large.hash);
    }

    #[test]
    fn rejects_a_buffer_that_does_not_match_its_dimensions() {
        let err = from_rgba(100, 100, &[0u8; 16], Options::default()).unwrap_err();
        assert!(err.to_string().contains("expected"), "{err}");
    }
}
