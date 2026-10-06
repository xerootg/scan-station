//! Assemble scanned JPEG pages into a single PDF.
//!
//! Scanned pages are embedded with the DCTDecode filter, i.e. the original
//! JPEG bytes are stored verbatim — no decode/re-encode, so there is no quality
//! loss and assembly is fast even on the Pi. Page geometry comes from the pixel
//! dimensions and the scan DPI (points = pixels / dpi * 72).

use std::io::Cursor;
use std::path::Path;

use anyhow::Context;
use pdf_writer::{Content, Filter, Finish, Name, Pdf, Rect, Ref};

const IMAGE_NAME: Name = Name(b"Im0");

/// One page ready to embed: the raw JPEG plus the facts needed to place it.
#[derive(Debug, Clone)]
pub struct PageImage {
    pub jpeg: Vec<u8>,
    pub width_px: u32,
    pub height_px: u32,
    /// DeviceGray when true, DeviceRGB otherwise. Must match the JPEG's
    /// component count or viewers render garbage.
    pub gray: bool,
    pub dpi: u32,
}

impl PageImage {
    fn width_pt(&self) -> f32 {
        self.width_px as f32 / self.dpi as f32 * 72.0
    }
    fn height_pt(&self) -> f32 {
        self.height_px as f32 / self.dpi as f32 * 72.0
    }
}

/// Load a JPEG page file into a [`PageImage`]. `gray` and `dpi` come from the
/// scan settings (so we needn't fully decode the image just to classify it);
/// only the dimensions are read from the file header.
pub fn page_from_jpeg(path: &Path, gray: bool, dpi: u32) -> anyhow::Result<PageImage> {
    let jpeg = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let (width_px, height_px) = image::ImageReader::new(Cursor::new(&jpeg))
        .with_guessed_format()?
        .into_dimensions()
        .with_context(|| format!("reading dimensions of {}", path.display()))?;
    Ok(PageImage {
        jpeg,
        width_px,
        height_px,
        gray,
        dpi: dpi.max(1),
    })
}

/// Build a PDF document from the given pages, returning the file bytes.
pub fn build_pdf(pages: &[PageImage]) -> anyhow::Result<Vec<u8>> {
    anyhow::ensure!(!pages.is_empty(), "cannot build a PDF with no pages");

    let mut pdf = Pdf::new();
    let mut next = 1i32;
    let mut alloc = || {
        let r = Ref::new(next);
        next += 1;
        r
    };

    let catalog_id = alloc();
    let page_tree_id = alloc();

    // Allocate (page, content, image) refs up front so we can list page kids.
    struct PageRefs {
        page: Ref,
        content: Ref,
        image: Ref,
    }
    let refs: Vec<PageRefs> = pages
        .iter()
        .map(|_| PageRefs {
            page: alloc(),
            content: alloc(),
            image: alloc(),
        })
        .collect();

    pdf.catalog(catalog_id).pages(page_tree_id);
    pdf.pages(page_tree_id)
        .kids(refs.iter().map(|r| r.page))
        .count(pages.len() as i32);

    for (page, r) in pages.iter().zip(&refs) {
        let w = page.width_pt();
        let h = page.height_pt();

        {
            let mut p = pdf.page(r.page);
            p.media_box(Rect::new(0.0, 0.0, w, h));
            p.parent(page_tree_id);
            p.contents(r.content);
            p.resources().x_objects().pair(IMAGE_NAME, r.image);
            p.finish();
        }

        {
            let mut img = pdf.image_xobject(r.image, &page.jpeg);
            img.width(page.width_px as i32);
            img.height(page.height_px as i32);
            if page.gray {
                img.color_space().device_gray();
            } else {
                img.color_space().device_rgb();
            }
            img.bits_per_component(8);
            img.filter(Filter::DctDecode);
            img.finish();
        }

        let mut content = Content::new();
        content.save_state();
        // Image space is the unit square; scale it to fill the page.
        content.transform([w, 0.0, 0.0, h, 0.0, 0.0]);
        content.x_object(IMAGE_NAME);
        content.restore_state();
        pdf.stream(r.content, &content.finish());
    }

    Ok(pdf.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    // A 2x2 red JPEG, generated once and inlined so the test needs no encoder.
    fn tiny_jpeg() -> Vec<u8> {
        use image::{ImageFormat, RgbImage};
        let img = RgbImage::from_pixel(2, 2, image::Rgb([255, 0, 0]));
        let mut buf = Cursor::new(Vec::new());
        img.write_to(&mut buf, ImageFormat::Jpeg).unwrap();
        buf.into_inner()
    }

    #[test]
    fn page_dimensions_in_points() {
        let p = PageImage {
            jpeg: vec![],
            width_px: 300,
            height_px: 600,
            gray: false,
            dpi: 300,
        };
        assert!((p.width_pt() - 72.0).abs() < 0.001);
        assert!((p.height_pt() - 144.0).abs() < 0.001);
    }

    #[test]
    fn builds_nonempty_pdf_with_header() {
        let page = PageImage {
            jpeg: tiny_jpeg(),
            width_px: 2,
            height_px: 2,
            gray: false,
            dpi: 72,
        };
        let bytes = build_pdf(&[page.clone(), page]).unwrap();
        assert!(bytes.starts_with(b"%PDF-"));
        assert!(bytes.len() > 100);
    }

    #[test]
    fn empty_pages_is_error() {
        assert!(build_pdf(&[]).is_err());
    }

    #[test]
    fn reads_jpeg_dimensions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("p.jpg");
        std::fs::write(&path, tiny_jpeg()).unwrap();
        let page = page_from_jpeg(&path, true, 200).unwrap();
        assert_eq!((page.width_px, page.height_px), (2, 2));
        assert!(page.gray);
        assert_eq!(page.dpi, 200);
    }
}
