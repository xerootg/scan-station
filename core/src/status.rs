//! Live scanner status via the eSCL `ScannerStatus` endpoint, so the UI can
//! show device state like a network copier (Ready / Document loaded / Scanning
//! / Jam / Cover open / Offline).

use std::time::Duration;

use serde::Serialize;

/// A friendly, UI-ready status snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ScannerStatus {
    pub reachable: bool,
    /// Raw `pwg:State` (Idle/Processing/Stopped), for logging/debug.
    pub state: String,
    /// Raw `scan:AdfState` (ScannerAdfLoaded/Empty/Jam/...).
    pub adf: String,
    /// Short human label for the status pill.
    pub label: String,
    /// One of: ready | loaded | busy | jam | open | offline — drives pill colour.
    pub kind: String,
}

impl ScannerStatus {
    fn offline() -> Self {
        Self {
            reachable: false,
            state: String::new(),
            adf: String::new(),
            label: "Scanner offline — is it powered on?".into(),
            kind: "offline".into(),
        }
    }
}

/// Extract the text content of the first `<name>...</name>` element.
fn tag<'a>(xml: &'a str, name: &str) -> Option<&'a str> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&close)? + start;
    Some(xml[start..end].trim())
}

/// Map raw eSCL state + ADF state to a friendly label and kind.
pub fn classify(state: &str, adf: &str) -> ScannerStatus {
    let (kind, label): (&str, &str) = if adf.contains("Jam") {
        ("jam", "Paper jam — clear the feeder")
    } else if adf.contains("Mispick") {
        ("jam", "Misfeed — reload the paper")
    } else if adf.contains("HatchOpen") {
        ("open", "Feeder cover open")
    } else if state.eq_ignore_ascii_case("Processing") || adf.contains("Processing") {
        ("busy", "Scanning…")
    } else if state.eq_ignore_ascii_case("Stopped") {
        ("jam", "Scanner stopped — check it")
    } else if adf == "ScannerAdfLoaded" {
        ("loaded", "Document loaded — ready to scan")
    } else {
        // Idle / ScannerAdfEmpty / anything unremarkable
        ("ready", "Ready")
    };
    ScannerStatus {
        reachable: true,
        state: state.to_string(),
        adf: adf.to_string(),
        label: label.to_string(),
        kind: kind.to_string(),
    }
}

/// Parse a `ScannerStatus` XML document.
pub fn parse_status(xml: &str) -> ScannerStatus {
    let state = tag(xml, "pwg:State").unwrap_or("");
    let adf = tag(xml, "scan:AdfState").unwrap_or("");
    classify(state, adf)
}

/// Fetch and classify the eSCL status from `base_url` (e.g.
/// `http://localhost:60000`). Any error (unreachable, timeout) yields an
/// `offline` status rather than an error — the UI just shows "offline".
pub fn fetch(base_url: &str) -> ScannerStatus {
    let url = format!("{}/eSCL/ScannerStatus", base_url.trim_end_matches('/'));
    let client = match reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
    {
        Ok(c) => c,
        Err(_) => return ScannerStatus::offline(),
    };
    match client.get(&url).send().and_then(|r| r.text()) {
        Ok(body) => parse_status(&body),
        Err(_) => ScannerStatus::offline(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"<?xml version="1.0"?>
<scan:ScannerStatus>
  <pwg:Version>2.9</pwg:Version>
  <pwg:State>Idle</pwg:State>
  <scan:AdfState>ScannerAdfLoaded</scan:AdfState>
</scan:ScannerStatus>"#;

    #[test]
    fn parses_loaded() {
        let s = parse_status(SAMPLE);
        assert_eq!(s.state, "Idle");
        assert_eq!(s.adf, "ScannerAdfLoaded");
        assert_eq!(s.kind, "loaded");
        assert!(s.reachable);
    }

    #[test]
    fn classifies_states() {
        assert_eq!(classify("Idle", "ScannerAdfEmpty").kind, "ready");
        assert_eq!(classify("Processing", "ScannerAdfProcessing").kind, "busy");
        assert_eq!(classify("Idle", "ScannerAdfJam").kind, "jam");
        assert_eq!(classify("Idle", "ScannerAdfMispick").kind, "jam");
        assert_eq!(classify("Idle", "ScannerAdfHatchOpen").kind, "open");
        assert_eq!(classify("Idle", "ScannerAdfLoaded").kind, "loaded");
    }

    #[test]
    fn missing_tags_default_ready() {
        let s = parse_status("<scan:ScannerStatus></scan:ScannerStatus>");
        assert_eq!(s.kind, "ready");
    }
}
