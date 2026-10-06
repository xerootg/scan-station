//! Scanner discovery and acquisition via SANE's `scanimage` CLI.
//!
//! We shell out to `scanimage` rather than binding libsane: it keeps the crate
//! free of C build deps, matches how the container ships the driver stack
//! (ipp-usb + sane-airscan), and makes the exact device behaviour easy to log
//! and reproduce. The HP ScanJet Pro 2000 s2 is driverless over eSCL, surfaced
//! by sane-airscan as an `airscan:*` device.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

/// A scanner as reported by `scanimage -L`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScannerInfo {
    /// SANE device string, e.g. `airscan:e0:HP ScanJet Pro 2000 s2`.
    pub device: String,
    /// Human description, e.g. `eSCL HP ScanJet Pro 2000 s2 sheetfed scanner`.
    pub description: String,
}

/// Which physical input to scan from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    /// Single-sided feed through the document feeder.
    Simplex,
    /// Double-sided feed through the document feeder.
    Duplex,
    /// Flatbed glass (if the device has one).
    Flatbed,
}

impl Side {
    /// SANE `--source` value. Values are device-dependent; these match
    /// sane-airscan / eSCL naming.
    pub fn sane_source(self) -> &'static str {
        match self {
            Side::Simplex => "ADF",
            Side::Duplex => "ADF Duplex",
            Side::Flatbed => "Flatbed",
        }
    }
}

/// Colour depth of the scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ColorMode {
    Color,
    Gray,
    Lineart,
}

impl ColorMode {
    pub fn sane_mode(self) -> &'static str {
        match self {
            ColorMode::Color => "Color",
            ColorMode::Gray => "Gray",
            ColorMode::Lineart => "Lineart",
        }
    }
    /// True when pages scanned in this mode have a single colour component, so
    /// the PDF image XObject must use DeviceGray.
    pub fn is_gray(self) -> bool {
        matches!(self, ColorMode::Gray | ColorMode::Lineart)
    }
}

/// A scan request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanOptions {
    /// SANE device; `None` lets scanimage pick the default device.
    pub device: Option<String>,
    pub source: Side,
    pub mode: ColorMode,
    /// Dots per inch. Also used to size PDF pages (pixels / dpi * 72pt).
    pub resolution: u32,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            device: None,
            source: Side::Simplex,
            mode: ColorMode::Color,
            resolution: 300,
        }
    }
}

/// Build the `scanimage` argument vector for a batch (ADF) acquisition writing
/// JPEG pages to `out_pattern` (a printf-style path like `.../page-%04d.jpg`).
pub fn build_scanimage_args(opts: &ScanOptions, out_pattern: &str) -> Vec<String> {
    let mut args = Vec::new();
    if let Some(dev) = &opts.device {
        args.push("-d".into());
        args.push(dev.clone());
    }
    args.push("--source".into());
    args.push(opts.source.sane_source().into());
    args.push("--mode".into());
    args.push(opts.mode.sane_mode().into());
    args.push("--resolution".into());
    args.push(opts.resolution.to_string());
    args.push("--format=jpeg".into());
    // --batch keeps pulling pages from the feeder until it's empty.
    args.push(format!("--batch={out_pattern}"));
    args
}

/// Parse the output of `scanimage -L` into a device list.
///
/// Each line looks like:
/// ```text
/// device `airscan:e0:HP ScanJet Pro 2000 s2' is a eSCL HP ScanJet Pro 2000 s2 sheetfed scanner
/// ```
pub fn parse_device_list(output: &str) -> Vec<ScannerInfo> {
    let mut devices = Vec::new();
    for line in output.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("device ") else {
            continue;
        };
        // device string sits between a backtick and a single quote.
        let Some(start) = rest.find('`') else {
            continue;
        };
        let after = &rest[start + 1..];
        let Some(end) = after.find('\'') else {
            continue;
        };
        let device = after[..end].to_string();
        let description = after[end + 1..]
            .trim_start()
            .strip_prefix("is a ")
            .unwrap_or("")
            .trim()
            .to_string();
        if !device.is_empty() {
            devices.push(ScannerInfo {
                device,
                description,
            });
        }
    }
    devices
}

/// List connected scanners (`scanimage -L`).
pub fn list_devices() -> anyhow::Result<Vec<ScannerInfo>> {
    let out = Command::new("scanimage").arg("-L").output()?;
    let text = String::from_utf8_lossy(&out.stdout);
    Ok(parse_device_list(&text))
}

/// Run one batch acquisition. Returns the JPEG page files produced, in order.
///
/// An empty feeder is not an error: scanimage exits with status 7
/// (`SANE_STATUS_NO_DOCS`) once the last page has been pulled, which is how a
/// normal run ends. We surface a real error only when no pages were produced
/// *and* the exit status indicates a genuine failure.
pub fn scan_batch(opts: &ScanOptions, out_dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    std::fs::create_dir_all(out_dir)?;
    let pattern = out_dir.join("page-%04d.jpg");
    let pattern = pattern.to_string_lossy().to_string();
    let args = build_scanimage_args(opts, &pattern);

    let out = Command::new("scanimage").args(&args).output()?;
    let pages = collect_pages(out_dir)?;

    if pages.is_empty() && !out.status.success() {
        let code = out.status.code().unwrap_or(-1);
        // 7 == SANE_STATUS_NO_DOCS: feeder was empty, not a fault.
        if code != 7 {
            let stderr = String::from_utf8_lossy(&out.stderr);
            anyhow::bail!("scanimage failed (exit {code}): {}", stderr.trim());
        }
    }
    Ok(pages)
}

/// Gather `page-*.jpg` files from a directory, sorted by name (acquisition
/// order, zero-padded).
pub fn collect_pages(dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut pages: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("page-") && n.ends_with(".jpg"))
                .unwrap_or(false)
        })
        .collect();
    pages.sort();
    Ok(pages)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_airscan_device_line() {
        let out = "device `airscan:e0:HP ScanJet Pro 2000 s2' is a eSCL HP ScanJet Pro 2000 s2 sheetfed scanner\n";
        let devs = parse_device_list(out);
        assert_eq!(devs.len(), 1);
        assert_eq!(devs[0].device, "airscan:e0:HP ScanJet Pro 2000 s2");
        assert_eq!(
            devs[0].description,
            "eSCL HP ScanJet Pro 2000 s2 sheetfed scanner"
        );
    }

    #[test]
    fn parses_multiple_and_ignores_noise() {
        let out = "\
searching for devices\n\
device `airscan:e0:HP ScanJet Pro 2000 s2' is a eSCL HP ScanJet Pro 2000 s2 sheetfed scanner\n\
device `test:0' is a Noname frontend-tester virtual device\n";
        let devs = parse_device_list(out);
        assert_eq!(devs.len(), 2);
        assert_eq!(devs[1].device, "test:0");
    }

    #[test]
    fn empty_output_yields_no_devices() {
        assert!(parse_device_list("No scanners were identified.").is_empty());
    }

    #[test]
    fn builds_duplex_color_args() {
        let opts = ScanOptions {
            device: Some("airscan:e0:HP".into()),
            source: Side::Duplex,
            mode: ColorMode::Color,
            resolution: 300,
        };
        let args = build_scanimage_args(&opts, "/tmp/out/page-%04d.jpg");
        assert_eq!(
            args,
            vec![
                "-d",
                "airscan:e0:HP",
                "--source",
                "ADF Duplex",
                "--mode",
                "Color",
                "--resolution",
                "300",
                "--format=jpeg",
                "--batch=/tmp/out/page-%04d.jpg",
            ]
        );
    }

    #[test]
    fn default_device_omits_dash_d() {
        let args = build_scanimage_args(&ScanOptions::default(), "p-%04d.jpg");
        assert!(!args.contains(&"-d".to_string()));
        assert_eq!(args[0], "--source");
    }

    #[test]
    fn gray_modes_flag_gray_colorspace() {
        assert!(ColorMode::Gray.is_gray());
        assert!(ColorMode::Lineart.is_gray());
        assert!(!ColorMode::Color.is_gray());
    }
}
