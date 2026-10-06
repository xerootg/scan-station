//! Thumbnail generation for the on-screen page strip.

use std::path::Path;

use anyhow::{Context, Result};
use base64::Engine;

/// Decode a page image, shrink it to fit `max_px` on the long edge, and return
/// a `data:image/png;base64,...` URL the webview can render directly.
pub fn thumbnail_data_url(path: &Path, max_px: u32) -> Result<String> {
    let img = image::ImageReader::open(path)
        .with_context(|| format!("opening {}", path.display()))?
        .with_guessed_format()?
        .decode()
        .with_context(|| format!("decoding {}", path.display()))?;
    // thumbnail() preserves aspect ratio and is cheap (box filter).
    let thumb = img.thumbnail(max_px, max_px);
    let mut buf = std::io::Cursor::new(Vec::new());
    thumb
        .write_to(&mut buf, image::ImageFormat::Png)
        .context("encoding thumbnail")?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(buf.get_ref());
    Ok(format!("data:image/png;base64,{b64}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn makes_data_url_from_jpeg() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("p.jpg");
        let img = image::RgbImage::from_pixel(64, 32, image::Rgb([10, 20, 30]));
        let mut buf = Cursor::new(Vec::new());
        img.write_to(&mut buf, image::ImageFormat::Jpeg).unwrap();
        std::fs::write(&path, buf.into_inner()).unwrap();

        let url = thumbnail_data_url(&path, 16).unwrap();
        assert!(url.starts_with("data:image/png;base64,"));
        assert!(url.len() > 30);
    }
}
