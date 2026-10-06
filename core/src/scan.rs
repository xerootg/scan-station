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

/// The value lists a device actually advertises for `--source` and `--mode`
/// (from `scanimage -d DEV -A`). An empty list means the backend reports no
/// enumerated values for that option — passing one anyway can crash the backend
/// (the sane `escl` backend core-dumps on `--source`), so we omit it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeviceCaps {
    pub sources: Vec<String>,
    pub modes: Vec<String>,
}

/// Pull the `a|b|c` value list out of an option line like
/// `    --source ADF|ADF Duplex [ADF]`. Returns an empty vec for
/// `{no_stringlist}` (no enumerated values).
fn option_values(line: &str, opt: &str) -> Option<Vec<String>> {
    let rest = line.trim().strip_prefix(opt)?;
    let list = rest.split(" [").next().unwrap_or("").trim();
    if list.is_empty() || list.contains('{') {
        return Some(Vec::new());
    }
    Some(list.split('|').map(|s| s.trim().to_string()).collect())
}

/// Parse `scanimage -d DEV -A` output into the advertised source/mode lists.
pub fn parse_caps(a_output: &str) -> DeviceCaps {
    let mut caps = DeviceCaps::default();
    for line in a_output.lines() {
        if let Some(v) = option_values(line, "--source ") {
            caps.sources = v;
        } else if let Some(v) = option_values(line, "--mode ") {
            caps.modes = v;
        }
    }
    caps
}

/// Query a device's option lists.
pub fn device_caps(device: &str) -> anyhow::Result<DeviceCaps> {
    let out = Command::new("scanimage")
        .arg("-d")
        .arg(device)
        .arg("-A")
        .output()?;
    Ok(parse_caps(&String::from_utf8_lossy(&out.stdout)))
}

/// Resolve the `--source` value to actually pass, given the device's caps.
/// `None` means omit the flag (unknown-but-no-list backend).
fn clamp_source(requested: &str, caps: Option<&DeviceCaps>) -> Option<String> {
    match caps {
        None => Some(requested.to_string()),
        Some(c) if c.sources.is_empty() => None,
        Some(c) if c.sources.iter().any(|s| s == requested) => Some(requested.to_string()),
        Some(c) => Some(
            c.sources
                .iter()
                .find(|s| s.as_str() == "ADF")
                .cloned()
                .unwrap_or_else(|| c.sources[0].clone()),
        ),
    }
}

/// Resolve the `--mode` value. Lineart isn't offered by the eSCL backends, so
/// fall back to Gray for a B&W request, else Color.
fn clamp_mode(requested: &str, caps: Option<&DeviceCaps>) -> Option<String> {
    match caps {
        None => Some(requested.to_string()),
        Some(c) if c.modes.is_empty() => None,
        Some(c) if c.modes.iter().any(|m| m == requested) => Some(requested.to_string()),
        Some(c) => {
            let pref = if requested == "Lineart" {
                "Gray"
            } else {
                "Color"
            };
            Some(
                c.modes
                    .iter()
                    .find(|m| m.as_str() == pref)
                    .or_else(|| c.modes.iter().find(|m| m.as_str() == "Color"))
                    .cloned()
                    .unwrap_or_else(|| c.modes[0].clone()),
            )
        }
    }
}

/// Build the `scanimage` argument vector for a batch (ADF) acquisition writing
/// JPEG pages to `out_pattern` (a printf-style path like `.../page-%04d.jpg`).
/// When `caps` is provided, the `--source`/`--mode` values are clamped to what
/// the device advertises (and omitted when it advertises no list).
pub fn build_scanimage_args(
    opts: &ScanOptions,
    out_pattern: &str,
    caps: Option<&DeviceCaps>,
) -> Vec<String> {
    let mut args = Vec::new();
    if let Some(dev) = &opts.device {
        args.push("-d".into());
        args.push(dev.clone());
    }
    if let Some(source) = clamp_source(opts.source.sane_source(), caps) {
        args.push("--source".into());
        args.push(source);
    }
    if let Some(mode) = clamp_mode(opts.mode.sane_mode(), caps) {
        args.push("--mode".into());
        args.push(mode);
    }
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
    let mut devices = parse_device_list(&text);
    // A USB eSCL scanner shows up under two backends: airscan and escl. Prefer
    // airscan — it advertises proper source/mode lists (incl. ADF Duplex),
    // whereas the escl backend core-dumps when given a --source. Rank
    // airscan first, escl last, everything else in between (stable).
    devices.sort_by_key(|d| {
        if d.device.starts_with("airscan:") {
            0
        } else if d.device.starts_with("escl:") {
            2
        } else {
            1
        }
    });
    Ok(devices)
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
    // Clamp source/mode to what this device actually advertises (best-effort).
    let caps = opts.device.as_deref().and_then(|d| device_caps(d).ok());
    let args = build_scanimage_args(opts, &pattern, caps.as_ref());

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
        let args = build_scanimage_args(&opts, "/tmp/out/page-%04d.jpg", None);
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
        let args = build_scanimage_args(&ScanOptions::default(), "p-%04d.jpg", None);
        assert!(!args.contains(&"-d".to_string()));
        assert_eq!(args[0], "--source");
    }

    #[test]
    fn parse_caps_reads_airscan_lists() {
        let a = "    --source ADF|ADF Duplex [ADF]\n    --mode Color|Gray [Color]\n    --resolution 75|300dpi [75]\n";
        let caps = parse_caps(a);
        assert_eq!(caps.sources, vec!["ADF", "ADF Duplex"]);
        assert_eq!(caps.modes, vec!["Color", "Gray"]);
    }

    #[test]
    fn parse_caps_escl_no_source_list() {
        let a = "    --mode Gray|Color [Gray]\n    --source {no_stringlist} [ADF]\n";
        let caps = parse_caps(a);
        assert!(caps.sources.is_empty()); // signals: omit --source (escl crashes otherwise)
        assert_eq!(caps.modes, vec!["Gray", "Color"]);
    }

    #[test]
    fn omits_source_when_backend_has_no_list() {
        // escl-style: empty source list -> no --source emitted (prevents crash)
        let caps = DeviceCaps {
            sources: vec![],
            modes: vec!["Gray".into(), "Color".into()],
        };
        let args = build_scanimage_args(&ScanOptions::default(), "p-%04d.jpg", Some(&caps));
        assert!(!args.iter().any(|a| a == "--source"));
        assert!(args.windows(2).any(|w| w[0] == "--mode" && w[1] == "Color"));
    }

    #[test]
    fn clamps_unsupported_source_and_lineart() {
        let caps = DeviceCaps {
            sources: vec!["ADF".into(), "ADF Duplex".into()],
            modes: vec!["Color".into(), "Gray".into()],
        };
        // Flatbed + Lineart aren't supported -> ADF + Gray
        let opts = ScanOptions {
            device: None,
            source: Side::Flatbed,
            mode: ColorMode::Lineart,
            resolution: 300,
        };
        let args = build_scanimage_args(&opts, "p-%04d.jpg", Some(&caps));
        assert!(args.windows(2).any(|w| w[0] == "--source" && w[1] == "ADF"));
        assert!(args.windows(2).any(|w| w[0] == "--mode" && w[1] == "Gray"));
    }

    #[test]
    fn supported_duplex_passes_through() {
        let caps = DeviceCaps {
            sources: vec!["ADF".into(), "ADF Duplex".into()],
            modes: vec!["Color".into(), "Gray".into()],
        };
        let opts = ScanOptions {
            device: None,
            source: Side::Duplex,
            mode: ColorMode::Color,
            resolution: 300,
        };
        let args = build_scanimage_args(&opts, "p-%04d.jpg", Some(&caps));
        assert!(args
            .windows(2)
            .any(|w| w[0] == "--source" && w[1] == "ADF Duplex"));
    }

    #[test]
    fn gray_modes_flag_gray_colorspace() {
        assert!(ColorMode::Gray.is_gray());
        assert!(ColorMode::Lineart.is_gray());
        assert!(!ColorMode::Color.is_gray());
    }
}
