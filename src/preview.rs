//! Image-preview decoding. A previewable file, double-clicked, spawns an
//! ephemeral worker (via `Tasks::spawn_preview`) that runs `decode` and sends
//! the result back for attachment as a preview node — the same plain-data,
//! main-thread-applies pattern as directory scans.

use std::path::Path;

/// Don't inherit the C app's decompression-bomb click: cap input size.
const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_PIXELS: u64 = 64_000_000;

pub struct DecodedImage {
    pub width: u32,
    pub height: u32,
    /// RGBA8, premultiplied alpha.
    pub rgba: Vec<u8>,
}

pub fn previewable(path: &Path) -> bool {
    let ext = path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase());
    matches!(ext.as_deref(), Some("png" | "jpg" | "jpeg" | "jfif" | "webp"))
}

pub fn decode(path: &Path) -> Option<DecodedImage> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() > MAX_FILE_BYTES {
        return None;
    }
    let reader = image::ImageReader::open(path).ok()?;
    let (w, h) = reader.into_dimensions().ok()?;
    if w as u64 * h as u64 > MAX_PIXELS {
        return None;
    }
    let img = image::open(path).ok()?;
    // The preview draws at 400 logical px wide; keep the texture (and the
    // staging traffic) bounded for huge photos.
    let img = if img.width() > 2048 || img.height() > 2048 {
        img.thumbnail(2048, 2048)
    } else {
        img
    };
    let mut rgba = img.into_rgba8();
    for px in rgba.pixels_mut() {
        let a = px.0[3] as u16;
        px.0[0] = ((px.0[0] as u16 * a) / 255) as u8;
        px.0[1] = ((px.0[1] as u16 * a) / 255) as u8;
        px.0[2] = ((px.0[2] as u16 * a) / 255) as u8;
    }
    let (width, height) = rgba.dimensions();
    Some(DecodedImage { width, height, rgba: rgba.into_raw() })
}
