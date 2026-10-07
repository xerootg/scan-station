//! Trim the trailing blank area from a scanned page.
//!
//! The HP ScanJet (over eSCL/airscan) does not auto-size ADF pages: it returns
//! the full ADF scan area (up to ~122") with the real page at the top and the
//! rest padded — sometimes solid white, sometimes solid black (the backing
//! roller). Left alone, a Letter page becomes an 8.5"×122" (or ×14") document.
//! We detect where content ends (the last row with both dark and light pixels)
//! and crop the height to it plus a small margin, keeping full width.

use std::path::Path;

use anyhow::{Context, Result};
use image::GenericImageView;

/// A real content row has both a dark and a light pixel (e.g. ink on paper).
/// Trailing padding the ADF adds is a solid fill — all white *or* all black
/// (the backing roller) — so it has only one, and gets trimmed.
const DARK: u8 = 120;
const LIGHT: u8 = 200;

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

    // Last row (scanning up from the bottom) that holds real content: it must
    // contain both a dark and a light pixel. A solid white or solid black row
    // is padding, not content.
    let mut content_bottom: Option<u32> = None;
    'rows: for y in (0..h).rev() {
        let (mut has_dark, mut has_light) = (false, false);
        let mut x = 0;
        while x < w {
            let v = luma.get_pixel(x, y)[0];
            if v < DARK {
                has_dark = true;
            } else if v > LIGHT {
                has_light = true;
            }
            if has_dark && has_light {
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

    // content_h rows of realistic "ink on paper" (white with a dark left bar,
    // so each content row has both dark and light), then the rest padded with a
    // solid fill — white or black (the ADF backing) per `pad_black`.
    fn write_padded(path: &Path, w: u32, content_h: u32, total_h: u32, pad_black: bool) {
        let pad = if pad_black {
            Rgb([0, 0, 0])
        } else {
            Rgb([255, 255, 255])
        };
        let mut img = RgbImage::from_pixel(w, total_h, Rgb([255, 255, 255]));
        for y in content_h..total_h {
            for x in 0..w {
                img.put_pixel(x, y, pad);
            }
        }
        for y in 0..content_h {
            for x in 0..40.min(w) {
                img.put_pixel(x, y, Rgb([10, 10, 10])); // "ink"
            }
        }
        let mut buf = Cursor::new(Vec::new());
        img.write_to(&mut buf, image::ImageFormat::Jpeg).unwrap();
        std::fs::write(path, buf.into_inner()).unwrap();
    }

    fn height_of(p: &Path) -> u32 {
        image::ImageReader::open(p)
            .unwrap()
            .with_guessed_format()
            .unwrap()
            .into_dimensions()
            .unwrap()
            .1
    }

    #[test]
    fn trims_trailing_white() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("p.jpg");
        write_padded(&p, 200, 300, 4000, false);
        autocrop_trailing_blank(&p, 150, false).unwrap();
        let h = height_of(&p);
        assert!(h < 500, "expected trimmed height, got {h}");
        assert!(h >= 300, "must not cut content, got {h}");
    }

    #[test]
    fn trims_trailing_black_backing() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("b.jpg");
        write_padded(&p, 200, 300, 4000, true); // black ADF backing below content
        autocrop_trailing_blank(&p, 150, false).unwrap();
        let h = height_of(&p);
        assert!(h < 500, "black backing must be trimmed too, got {h}");
        assert!(h >= 300, "must not cut content, got {h}");
    }

    #[test]
    fn keeps_full_page_untrimmed() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("full.jpg");
        write_padded(&p, 200, 1000, 1000, false); // content fills it
        autocrop_trailing_blank(&p, 150, false).unwrap();
        assert_eq!(height_of(&p), 1000);
    }

    #[test]
    fn blank_page_falls_back_not_huge() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("blank.jpg");
        write_padded(&p, 200, 0, 4000, false); // all white
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
