//! Trim the trailing blank area from a scanned page.
//!
//! The HP ScanJet (over eSCL/airscan) does not auto-size ADF pages: it returns
//! the full ADF maximum length (~122") with the real page at the top and the
//! rest padded white. Left alone, a Letter page becomes an 8.5"×122" document.
//! We detect where content ends and crop the height to it plus a small margin,
//! keeping full width (the scanner centre-justifies within the page width, so
//! the width already matches the sheet).

use std::path::Path;

use anyhow::{Context, Result};
use image::GenericImageView;

/// A pixel is "content" if its luma is below this (background is ~255 white).
const CONTENT_THRESHOLD: u8 = 235;

/// Crop `path` in place: drop everything below the last content row + margin.
/// `gray` selects the re-encode colourspace so it matches the PDF image
/// XObject (1 component for Gray/Lineart, 3 for Color).
pub fn autocrop_trailing_blank(path: &Path, dpi: u32, gray: bool) -> Result<()> {
    let img = image::open(path).with_context(|| format!("open {}", path.display()))?;
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return Ok(());
    }
    let luma = img.to_luma8();

    // Last row (scanning up from the bottom) that has any content pixel.
    let mut content_bottom: Option<u32> = None;
    'rows: for y in (0..h).rev() {
        let mut x = 0;
        while x < w {
            if luma.get_pixel(x, y)[0] < CONTENT_THRESHOLD {
                content_bottom = Some(y);
                break 'rows;
            }
            x += 4; // sample every 4th column — plenty to detect text/lines
        }
    }

    let margin = ((dpi as f32) * 0.2).round() as u32; // ~0.2in breathing room
    let bottom = match content_bottom {
        Some(y) => y,
        // Blank page: fall back to a Letter-ish height rather than 122".
        None => (dpi * 11).min(h.saturating_sub(1)),
    };
    let new_h = (bottom + margin + 1).min(h);
    if new_h >= h {
        return Ok(()); // nothing meaningful to trim
    }

    let cropped = img.crop_imm(0, 0, w, new_h);
    let file = std::fs::File::create(path).with_context(|| format!("write {}", path.display()))?;
    let mut writer = std::io::BufWriter::new(file);
    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut writer, 90);
    if gray {
        let g = cropped.to_luma8();
        enc.encode(
            g.as_raw(),
            g.width(),
            g.height(),
            image::ExtendedColorType::L8,
        )
    } else {
        let rgb = cropped.to_rgb8();
        enc.encode(
            rgb.as_raw(),
            rgb.width(),
            rgb.height(),
            image::ExtendedColorType::Rgb8,
        )
    }
    .context("re-encoding cropped page")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};
    use std::io::Cursor;

    fn write_padded(path: &Path, w: u32, content_h: u32, total_h: u32) {
        // content_h rows of a dark bar at top, white below to total_h
        let mut img = RgbImage::from_pixel(w, total_h, Rgb([255, 255, 255]));
        for y in 0..content_h {
            for x in 0..w {
                img.put_pixel(x, y, Rgb([10, 10, 10]));
            }
        }
        let mut buf = Cursor::new(Vec::new());
        img.write_to(&mut buf, image::ImageFormat::Jpeg).unwrap();
        std::fs::write(path, buf.into_inner()).unwrap();
    }

    #[test]
    fn trims_trailing_white() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("p.jpg");
        write_padded(&p, 200, 300, 4000); // content 300px, padded to 4000
        autocrop_trailing_blank(&p, 150, false).unwrap();
        let (w, h) = image::ImageReader::open(&p)
            .unwrap()
            .with_guessed_format()
            .unwrap()
            .into_dimensions()
            .unwrap();
        assert_eq!(w, 200);
        // content(300) + margin(30) ≈ 330, well under the original 4000
        assert!(h < 500, "expected trimmed height, got {h}");
        assert!(h >= 300, "must not cut content, got {h}");
    }

    #[test]
    fn keeps_full_page_untrimmed() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("full.jpg");
        write_padded(&p, 200, 1000, 1000); // content fills it
        autocrop_trailing_blank(&p, 150, false).unwrap();
        let (_, h) = image::ImageReader::open(&p)
            .unwrap()
            .with_guessed_format()
            .unwrap()
            .into_dimensions()
            .unwrap();
        assert_eq!(h, 1000);
    }

    #[test]
    fn blank_page_falls_back_not_huge() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("blank.jpg");
        write_padded(&p, 200, 0, 4000); // all white
        autocrop_trailing_blank(&p, 150, false).unwrap();
        let (_, h) = image::ImageReader::open(&p)
            .unwrap()
            .with_guessed_format()
            .unwrap()
            .into_dimensions()
            .unwrap();
        // letter-ish (11in) plus the margin, not the original 4000
        assert!(
            h <= 150 * 12,
            "blank should fall back to letter-ish, got {h}"
        );
        assert!(h < 4000);
    }
}
