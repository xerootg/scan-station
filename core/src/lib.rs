//! scan-station core: scanner control, PDF assembly, and upload dispatch.
//!
//! This crate deliberately has no GUI/webkit dependencies so it compiles and
//! unit-tests on any host (CI, a dev laptop) without the Tauri toolchain. The
//! Tauri shell in `../src-tauri` is a thin layer that holds job state and wires
//! these functions to the frontend.

pub mod config;
pub mod job;
pub mod pdf;
pub mod preview;
pub mod scan;
pub mod upload;

pub use config::Config;
pub use job::{Job, Page};
pub use scan::{ColorMode, ScanOptions, ScannerInfo, Side};
