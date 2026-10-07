//! Runtime configuration: scan defaults and upload destinations.
//!
//! In the container, everything arrives as environment variables (secrets come
//! from a Kubernetes Secret). For native/desktop use a TOML file is also
//! supported; env vars overlay the file. A destination is "available" to the UI
//! only when all its required fields are present.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::scan::{ColorMode, ScanOptions, Side};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub scan: ScanDefaults,
    #[serde(default)]
    pub paperless: Option<PaperlessConfig>,
    #[serde(default)]
    pub nextcloud: Option<NextcloudConfig>,
    #[serde(default)]
    pub email: Option<EmailConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanDefaults {
    pub resolution: u32,
    pub mode: ColorMode,
    pub source: Side,
}

impl Default for ScanDefaults {
    fn default() -> Self {
        Self {
            resolution: 300,
            mode: ColorMode::Color,
            source: Side::Duplex,
        }
    }
}

impl ScanDefaults {
    pub fn to_options(&self, device: Option<String>) -> ScanOptions {
        ScanOptions {
            device,
            source: self.source,
            mode: self.mode,
            resolution: self.resolution,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaperlessConfig {
    /// Base URL, e.g. `http://paperless.themissing.xyz` (no trailing slash needed).
    pub base_url: String,
    /// API token (`Authorization: Token <token>`).
    pub token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NextcloudConfig {
    /// Base URL of the Nextcloud instance.
    pub base_url: String,
    pub username: String,
    /// App password (not the login password).
    pub password: String,
    /// Destination folder under the user's files, e.g. `Scans`.
    #[serde(default = "default_folder")]
    pub folder: String,
}

fn default_folder() -> String {
    "Scans".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmailConfig {
    pub smtp_host: String,
    #[serde(default = "default_smtp_port")]
    pub smtp_port: u16,
    pub username: String,
    pub password: String,
    /// From header, e.g. `Scan Station <scans@example.com>`.
    pub from: String,
    /// Default recipient; empty means the operator must type one in the UI.
    #[serde(default)]
    pub to: String,
    /// STARTTLS when true (port 587), implicit TLS otherwise (port 465).
    #[serde(default = "default_true")]
    pub starttls: bool,
    /// Skip TLS certificate verification. Needed when sending to an internal
    /// mail server by its cluster/service name while its cert is issued for the
    /// public mail hostname (name mismatch). LAN-only; off by default.
    #[serde(default)]
    pub insecure_tls: bool,
}

fn default_smtp_port() -> u16 {
    587
}
fn default_true() -> bool {
    true
}

/// A destination surfaced to the UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Destination {
    pub id: String,
    pub label: String,
    pub available: bool,
}

impl Config {
    /// Load TOML (if a path is set/exists) then overlay environment variables.
    pub fn load() -> Self {
        let mut cfg = std::env::var("SCAN_STATION_CONFIG")
            .ok()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .or_else(|| std::fs::read_to_string("/etc/scan-station/config.toml").ok())
            .and_then(|s| Self::from_toml_str(&s).ok())
            .unwrap_or_default();
        let env: HashMap<String, String> = std::env::vars().collect();
        cfg.overlay_env(&env);
        cfg
    }

    pub fn from_toml_str(s: &str) -> anyhow::Result<Self> {
        Ok(toml::from_str(s)?)
    }

    /// Apply env-var overrides. Only touches a destination when its key fields
    /// are present, so a partial env won't half-configure something.
    pub fn overlay_env(&mut self, env: &HashMap<String, String>) {
        let get = |k: &str| {
            env.get(k)
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        };

        if let (Some(base_url), Some(token)) = (get("PAPERLESS_URL"), get("PAPERLESS_TOKEN")) {
            self.paperless = Some(PaperlessConfig { base_url, token });
        }

        if let (Some(base_url), Some(username), Some(password)) = (
            get("NEXTCLOUD_URL"),
            get("NEXTCLOUD_USER"),
            get("NEXTCLOUD_PASS"),
        ) {
            self.nextcloud = Some(NextcloudConfig {
                base_url,
                username,
                password,
                folder: get("NEXTCLOUD_FOLDER").unwrap_or_else(default_folder),
            });
        }

        // Email needs host + credentials; From defaults to the username and the
        // default recipient is optional (the UI can supply one per job).
        if let (Some(smtp_host), Some(username), Some(password)) =
            (get("SMTP_HOST"), get("SMTP_USER"), get("SMTP_PASS"))
        {
            let from = get("SMTP_FROM").unwrap_or_else(|| username.clone());
            self.email = Some(EmailConfig {
                smtp_host,
                smtp_port: get("SMTP_PORT").and_then(|p| p.parse().ok()).unwrap_or(587),
                username,
                password,
                from,
                to: get("SMTP_TO").unwrap_or_default(),
                starttls: get("SMTP_STARTTLS")
                    .map(|v| v != "0" && !v.eq_ignore_ascii_case("false"))
                    .unwrap_or(true),
                insecure_tls: get("SMTP_INSECURE_TLS")
                    .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                    .unwrap_or(false),
            });
        }

        if let Some(res) = get("SCAN_RESOLUTION").and_then(|v| v.parse().ok()) {
            self.scan.resolution = res;
        }
    }

    /// Destinations for the UI, each flagged available or not.
    pub fn destinations(&self) -> Vec<Destination> {
        vec![
            Destination {
                id: "paperless".into(),
                label: "Paperless".into(),
                available: self.paperless.is_some(),
            },
            Destination {
                id: "nextcloud".into(),
                label: "Nextcloud".into(),
                available: self.nextcloud.is_some(),
            },
            Destination {
                id: "email".into(),
                label: "Email".into(),
                available: self.email.is_some(),
            },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn defaults_have_no_destinations_available() {
        let cfg = Config::default();
        assert!(cfg.destinations().iter().all(|d| !d.available));
        assert_eq!(cfg.scan.resolution, 300);
    }

    #[test]
    fn env_overlay_enables_paperless_only_when_both_fields_present() {
        let mut cfg = Config::default();
        cfg.overlay_env(&env(&[("PAPERLESS_URL", "http://p")]));
        assert!(cfg.paperless.is_none(), "url without token must not enable");

        cfg.overlay_env(&env(&[
            ("PAPERLESS_URL", "http://p"),
            ("PAPERLESS_TOKEN", "abc"),
        ]));
        {
            let p = cfg.paperless.as_ref().expect("should enable");
            assert_eq!(p.base_url, "http://p");
            assert_eq!(p.token, "abc");
        }
        assert!(cfg
            .destinations()
            .iter()
            .any(|d| d.id == "paperless" && d.available));
    }

    #[test]
    fn env_overlay_email_parses_port_and_tls() {
        let mut cfg = Config::default();
        cfg.overlay_env(&env(&[
            ("SMTP_HOST", "mail"),
            ("SMTP_USER", "u"),
            ("SMTP_PASS", "p"),
            ("SMTP_FROM", "a@b"),
            ("SMTP_TO", "c@d"),
            ("SMTP_PORT", "465"),
            ("SMTP_STARTTLS", "false"),
        ]));
        let e = cfg.email.expect("email configured");
        assert_eq!(e.smtp_port, 465);
        assert!(!e.starttls);
    }

    #[test]
    fn email_from_defaults_to_user_and_to_optional() {
        let mut cfg = Config::default();
        cfg.overlay_env(&env(&[
            ("SMTP_HOST", "mailu-front.mailu.svc.cluster.local"),
            ("SMTP_USER", "scans@themissing.xyz"),
            ("SMTP_PASS", "p"),
            ("SMTP_INSECURE_TLS", "true"),
        ]));
        let e = cfg.email.expect("email configured");
        assert_eq!(e.from, "scans@themissing.xyz"); // defaults to the username
        assert_eq!(e.to, ""); // optional; UI supplies a recipient
        assert!(e.insecure_tls);
        assert!(e.starttls); // default
    }

    #[test]
    fn blank_env_values_are_ignored() {
        let mut cfg = Config::default();
        cfg.overlay_env(&env(&[("PAPERLESS_URL", "  "), ("PAPERLESS_TOKEN", "x")]));
        assert!(cfg.paperless.is_none());
    }

    #[test]
    fn toml_parses_nextcloud_with_default_folder() {
        let cfg = Config::from_toml_str(
            r#"
            [nextcloud]
            base_url = "https://nc.example"
            username = "scanner"
            password = "app-pw"
        "#,
        )
        .unwrap();
        let nc = cfg.nextcloud.unwrap();
        assert_eq!(nc.folder, "Scans");
    }
}
