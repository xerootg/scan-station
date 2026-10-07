//! The current scan job: an ordered set of page images in a scratch directory,
//! with the operations the UI drives (append, delete, reorder, clear, build).

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::Serialize;

use crate::pdf;
use crate::preview;

/// One page in the job. Fields the UI needs are serialized; on-disk details are
/// skipped.
#[derive(Debug, Clone, Serialize)]
pub struct Page {
    pub id: String,
    /// `data:image/png;base64,...` preview.
    pub thumbnail: String,
    pub width_px: u32,
    pub height_px: u32,
    #[serde(skip)]
    pub path: PathBuf,
    #[serde(skip)]
    pub gray: bool,
    #[serde(skip)]
    pub dpi: u32,
}

/// A scan job. Pages live in a [`tempfile::TempDir`] that is removed when the
/// job is dropped or cleared.
pub struct Job {
    dir: tempfile::TempDir,
    pages: Vec<Page>,
    counter: usize,
    thumb_px: u32,
}

impl Job {
    pub fn new() -> Result<Self> {
        Ok(Self {
            dir: tempfile::tempdir()?,
            pages: Vec::new(),
            counter: 0,
            thumb_px: 240,
        })
    }

    pub fn dir(&self) -> &Path {
        self.dir.path()
    }
    pub fn pages(&self) -> &[Page] {
        &self.pages
    }
    pub fn len(&self) -> usize {
        self.pages.len()
    }
    pub fn is_empty(&self) -> bool {
        self.pages.is_empty()
    }

    /// Import freshly scanned JPEG files into the job (copied into the job's
    /// scratch dir so the caller's temp area can be cleaned up), building a
    /// thumbnail for each. Returns the pages added, in order.
    pub fn append_scanned(&mut self, files: &[PathBuf], gray: bool, dpi: u32) -> Result<Vec<Page>> {
        let mut added = Vec::new();
        for src in files {
            self.counter += 1;
            let id = uuid::Uuid::new_v4().to_string();
            let dest = self
                .dir
                .path()
                .join(format!("page-{:06}-{}.jpg", self.counter, &id[..8]));
            std::fs::copy(src, &dest)?;
            // ADF pages come back padded to the scanner's max length; trim the
            // trailing blank so a Letter page isn't an 8.5"x122" document.
            if let Err(e) = crate::crop::autocrop_trailing_blank(&dest, dpi, gray) {
                log::warn!("autocrop failed for {}: {e}", dest.display());
            }
            let thumbnail = preview::thumbnail_data_url(&dest, self.thumb_px)?;
            let (width_px, height_px) = image::ImageReader::open(&dest)?
                .with_guessed_format()?
                .into_dimensions()?;
            let page = Page {
                id,
                thumbnail,
                width_px,
                height_px,
                path: dest,
                gray,
                dpi,
            };
            self.pages.push(page.clone());
            added.push(page);
        }
        Ok(added)
    }

    /// Remove a page by id; true if it existed.
    pub fn remove(&mut self, id: &str) -> bool {
        if let Some(pos) = self.pages.iter().position(|p| p.id == id) {
            let p = self.pages.remove(pos);
            let _ = std::fs::remove_file(&p.path);
            true
        } else {
            false
        }
    }

    /// Reorder pages to match `ids`. Ids not present are ignored; pages not
    /// listed keep their relative order and trail the listed ones.
    pub fn reorder(&mut self, ids: &[String]) {
        let mut remaining = std::mem::take(&mut self.pages);
        let mut ordered = Vec::with_capacity(remaining.len());
        for id in ids {
            if let Some(pos) = remaining.iter().position(|p| &p.id == id) {
                ordered.push(remaining.remove(pos));
            }
        }
        ordered.extend(remaining);
        self.pages = ordered;
    }

    /// Discard all pages (and their files).
    pub fn clear(&mut self) {
        for p in self.pages.drain(..) {
            let _ = std::fs::remove_file(&p.path);
        }
        self.counter = 0;
    }

    /// Assemble the current pages into a single PDF.
    pub fn build_pdf(&self) -> Result<Vec<u8>> {
        anyhow::ensure!(!self.pages.is_empty(), "no pages to build");
        let mut imgs = Vec::with_capacity(self.pages.len());
        for p in &self.pages {
            imgs.push(pdf::page_from_jpeg(&p.path, p.gray, p.dpi)?);
        }
        pdf::build_pdf(&imgs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn write_jpeg(path: &Path, w: u32, h: u32) {
        let img = image::RgbImage::from_pixel(w, h, image::Rgb([200, 100, 50]));
        let mut buf = Cursor::new(Vec::new());
        img.write_to(&mut buf, image::ImageFormat::Jpeg).unwrap();
        std::fs::write(path, buf.into_inner()).unwrap();
    }

    fn fixture(n: usize) -> (tempfile::TempDir, Vec<PathBuf>) {
        let dir = tempfile::tempdir().unwrap();
        let mut files = Vec::new();
        for i in 0..n {
            let p = dir.path().join(format!("src-{i}.jpg"));
            write_jpeg(&p, 32 + i as u32, 48);
            files.push(p);
        }
        (dir, files)
    }

    #[test]
    fn append_builds_pages_with_thumbnails() {
        let (_src, files) = fixture(2);
        let mut job = Job::new().unwrap();
        let added = job.append_scanned(&files, false, 300).unwrap();
        assert_eq!(added.len(), 2);
        assert_eq!(job.len(), 2);
        assert!(job.pages()[0]
            .thumbnail
            .starts_with("data:image/png;base64,"));
    }

    #[test]
    fn remove_and_clear() {
        let (_src, files) = fixture(3);
        let mut job = Job::new().unwrap();
        job.append_scanned(&files, false, 300).unwrap();
        let id = job.pages()[1].id.clone();
        assert!(job.remove(&id));
        assert!(!job.remove(&id));
        assert_eq!(job.len(), 2);
        job.clear();
        assert!(job.is_empty());
    }

    #[test]
    fn reorder_puts_listed_first_then_leftovers() {
        let (_src, files) = fixture(3);
        let mut job = Job::new().unwrap();
        job.append_scanned(&files, false, 300).unwrap();
        let ids: Vec<String> = job.pages().iter().map(|p| p.id.clone()).collect();
        // Ask for [third, first]; second should trail.
        job.reorder(&[ids[2].clone(), ids[0].clone()]);
        let now: Vec<String> = job.pages().iter().map(|p| p.id.clone()).collect();
        assert_eq!(now, vec![ids[2].clone(), ids[0].clone(), ids[1].clone()]);
    }

    #[test]
    fn build_pdf_from_pages() {
        let (_src, files) = fixture(2);
        let mut job = Job::new().unwrap();
        job.append_scanned(&files, true, 150).unwrap();
        let pdf = job.build_pdf().unwrap();
        assert!(pdf.starts_with(b"%PDF-"));
    }

    #[test]
    fn build_pdf_empty_errors() {
        let job = Job::new().unwrap();
        assert!(job.build_pdf().is_err());
    }
}
