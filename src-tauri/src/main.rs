// Prevent a second console window on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Tauri shell for scan-station.
//!
//! Holds the current [`Job`] and loaded [`Config`] as managed state and exposes
//! a handful of commands the copier-style frontend calls. All the real work
//! (driving scanimage, assembling the PDF, uploading) lives in
//! `scanstation-core`; blocking work is pushed onto the blocking pool so the UI
//! stays responsive.

use std::sync::Mutex;

use serde::Serialize;
use tauri::State;

use scanstation_core::config::Destination;
use scanstation_core::job::{Job, Page};
use scanstation_core::scan::{self, ScanOptions, ScannerInfo};
use scanstation_core::status::{self, ScannerStatus};
use scanstation_core::upload::{self, DocMeta};
use scanstation_core::Config;

struct AppState {
    job: Mutex<Job>,
    cfg: Config,
}

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

#[derive(Serialize)]
struct UiConfig {
    destinations: Vec<Destination>,
    resolution: u32,
    mode: scan::ColorMode,
    source: scan::Side,
}

#[derive(Serialize)]
struct ScanResult {
    added: Vec<Page>,
    total: usize,
}

#[derive(Serialize)]
struct UploadStatus {
    target: String,
    ok: bool,
    detail: String,
}

#[tauri::command]
fn config_info(state: State<'_, AppState>) -> UiConfig {
    UiConfig {
        destinations: state.cfg.destinations(),
        resolution: state.cfg.scan.resolution,
        mode: state.cfg.scan.mode,
        source: state.cfg.scan.source,
    }
}

#[tauri::command]
async fn list_scanners() -> Result<Vec<ScannerInfo>, String> {
    tauri::async_runtime::spawn_blocking(scan::list_devices)
        .await
        .map_err(err)?
        .map_err(err)
}

/// Live scanner status for the copier-style status pill. Reads the eSCL
/// ScannerStatus endpoint (ipp-usb exposes it on localhost); override with
/// SCANNER_ESCL_URL for a network scanner.
#[tauri::command]
async fn scanner_status() -> Result<ScannerStatus, String> {
    let base =
        std::env::var("SCANNER_ESCL_URL").unwrap_or_else(|_| "http://localhost:60000".to_string());
    tauri::async_runtime::spawn_blocking(move || status::fetch(&base))
        .await
        .map_err(err)
}

#[tauri::command]
fn pages(state: State<'_, AppState>) -> Result<Vec<Page>, String> {
    Ok(state.job.lock().map_err(err)?.pages().to_vec())
}

#[tauri::command]
async fn scan(state: State<'_, AppState>, opts: ScanOptions) -> Result<ScanResult, String> {
    // Scan into a throwaway dir on the blocking pool; the pages are copied into
    // the job's own scratch dir by `append_scanned`, so this dir can vanish.
    let tmp = tempfile::tempdir().map_err(err)?;
    let dir = tmp.path().to_path_buf();
    let opts_for_scan = opts.clone();
    let files =
        tauri::async_runtime::spawn_blocking(move || scan::scan_batch(&opts_for_scan, &dir))
            .await
            .map_err(err)?
            .map_err(err)?;

    if files.is_empty() {
        return Err("No pages scanned — is the feeder loaded?".into());
    }

    let gray = opts.mode.is_gray();
    let mut job = state.job.lock().map_err(err)?;
    let added = job
        .append_scanned(&files, gray, opts.resolution)
        .map_err(err)?;
    let total = job.len();
    Ok(ScanResult { added, total })
}

#[tauri::command]
fn delete_page(state: State<'_, AppState>, id: String) -> Result<usize, String> {
    let mut job = state.job.lock().map_err(err)?;
    job.remove(&id);
    Ok(job.len())
}

#[tauri::command]
fn reorder(state: State<'_, AppState>, ids: Vec<String>) -> Result<(), String> {
    state.job.lock().map_err(err)?.reorder(&ids);
    Ok(())
}

#[tauri::command]
fn clear(state: State<'_, AppState>) -> Result<(), String> {
    state.job.lock().map_err(err)?.clear();
    Ok(())
}

#[tauri::command]
async fn upload(
    state: State<'_, AppState>,
    targets: Vec<String>,
    title: String,
    recipient: Option<String>,
) -> Result<Vec<UploadStatus>, String> {
    if targets.is_empty() {
        return Err("Pick at least one destination".into());
    }
    // Build the PDF under the lock (fast), then release it before the network
    // calls so the UI can keep interacting with the job.
    let pdf = {
        let job = state.job.lock().map_err(err)?;
        job.build_pdf().map_err(err)?
    };

    let title = if title.trim().is_empty() {
        "Scanned document".to_string()
    } else {
        title.trim().to_string()
    };
    let meta = DocMeta {
        filename: format!("{}.pdf", sanitize_filename(&title)),
        title,
        recipient,
    };
    let cfg = state.cfg.clone();

    let results =
        tauri::async_runtime::spawn_blocking(move || upload::dispatch(&cfg, &targets, &pdf, &meta))
            .await
            .map_err(err)?;

    Ok(results
        .into_iter()
        .map(|(target, r)| match r {
            Ok(detail) => UploadStatus {
                target,
                ok: true,
                detail,
            },
            Err(detail) => UploadStatus {
                target,
                ok: false,
                detail,
            },
        })
        .collect())
}

/// Turn a title into a safe filename stem.
fn sanitize_filename(title: &str) -> String {
    let s: String = title
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let s = s.trim_matches('_').to_string();
    if s.is_empty() {
        "scan".to_string()
    } else {
        s
    }
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let job = Job::new().expect("failed to create scratch job directory");
    let cfg = Config::load();
    log::info!(
        "scan-station starting; destinations available: {:?}",
        cfg.destinations()
            .iter()
            .filter(|d| d.available)
            .map(|d| d.id.clone())
            .collect::<Vec<_>>()
    );

    tauri::Builder::default()
        .manage(AppState {
            job: Mutex::new(job),
            cfg,
        })
        .invoke_handler(tauri::generate_handler![
            config_info,
            list_scanners,
            scanner_status,
            pages,
            scan,
            delete_page,
            reorder,
            clear,
            upload,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Scan Station");
}

#[cfg(test)]
mod tests {
    use super::sanitize_filename;

    #[test]
    fn sanitizes_titles() {
        assert_eq!(sanitize_filename("Invoice 2026/01"), "Invoice_2026_01");
        assert_eq!(sanitize_filename("  "), "scan");
        assert_eq!(sanitize_filename("a.b.c"), "a_b_c");
    }
}
