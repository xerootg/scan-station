//! Deliver the assembled PDF to one or more destinations.
//!
//! All transports are blocking (reqwest::blocking, lettre's SmtpTransport); the
//! Tauri shell calls [`dispatch`] from a blocking task. Each destination
//! reports its own success/failure so one failing target never blocks another.

use anyhow::{Context, Result};

use crate::config::{Config, EmailConfig, NextcloudConfig, PaperlessConfig};

/// Metadata for an upload.
#[derive(Debug, Clone)]
pub struct DocMeta {
    pub title: String,
    pub filename: String,
    /// Overrides the configured default email recipient when set.
    pub recipient: Option<String>,
}

/// Outcome per destination: `Ok(detail)` or `Err(message)`.
pub type DispatchResult = Vec<(String, std::result::Result<String, String>)>;

fn paperless_post_url(base: &str) -> String {
    format!(
        "{}/api/documents/post_document/",
        base.trim_end_matches('/')
    )
}

fn nextcloud_dav_base(cfg: &NextcloudConfig) -> String {
    format!(
        "{}/remote.php/dav/files/{}",
        cfg.base_url.trim_end_matches('/'),
        cfg.username
    )
}

/// POST the PDF to Paperless-ngx. Returns the consumption task UUID it echoes.
pub fn to_paperless(cfg: &PaperlessConfig, pdf: &[u8], meta: &DocMeta) -> Result<String> {
    let part = reqwest::blocking::multipart::Part::bytes(pdf.to_vec())
        .file_name(meta.filename.clone())
        .mime_str("application/pdf")?;
    let form = reqwest::blocking::multipart::Form::new()
        .text("title", meta.title.clone())
        .part("document", part);

    let resp = reqwest::blocking::Client::new()
        .post(paperless_post_url(&cfg.base_url))
        .header("Authorization", format!("Token {}", cfg.token))
        .multipart(form)
        .send()
        .context("posting to paperless")?;

    let status = resp.status();
    let body = resp.text().unwrap_or_default();
    anyhow::ensure!(status.is_success(), "paperless {status}: {}", body.trim());
    Ok(body.trim().trim_matches('"').to_string())
}

/// PUT the PDF into a Nextcloud folder over WebDAV (creating the folder first).
pub fn to_nextcloud(cfg: &NextcloudConfig, pdf: &[u8], filename: &str) -> Result<()> {
    let client = reqwest::blocking::Client::new();
    let dav = nextcloud_dav_base(cfg);
    let folder = cfg.folder.trim_matches('/');

    if !folder.is_empty() {
        // MKCOL is idempotent enough for us: 201 created, 405 already-exists.
        let resp = client
            .request(
                reqwest::Method::from_bytes(b"MKCOL")?,
                format!("{dav}/{folder}"),
            )
            .basic_auth(&cfg.username, Some(&cfg.password))
            .send()
            .context("nextcloud MKCOL")?;
        let code = resp.status().as_u16();
        anyhow::ensure!(
            resp.status().is_success() || code == 405,
            "nextcloud MKCOL failed: {}",
            resp.status()
        );
    }

    let url = if folder.is_empty() {
        format!("{dav}/{filename}")
    } else {
        format!("{dav}/{folder}/{filename}")
    };
    let resp = client
        .put(url)
        .basic_auth(&cfg.username, Some(&cfg.password))
        .header("Content-Type", "application/pdf")
        .body(pdf.to_vec())
        .send()
        .context("nextcloud PUT")?;
    anyhow::ensure!(
        resp.status().is_success(),
        "nextcloud PUT failed: {}",
        resp.status()
    );
    Ok(())
}

/// Email the PDF as an attachment via SMTP.
pub fn to_email(
    cfg: &EmailConfig,
    pdf: &[u8],
    filename: &str,
    recipient: Option<&str>,
) -> Result<()> {
    use lettre::message::{header, Attachment, MultiPart, SinglePart};
    use lettre::transport::smtp::authentication::Credentials;
    use lettre::{Message, SmtpTransport, Transport};

    let to = recipient.unwrap_or(&cfg.to);
    let attachment = Attachment::new(filename.to_string())
        .body(pdf.to_vec(), header::ContentType::parse("application/pdf")?);

    let email = Message::builder()
        .from(cfg.from.parse().context("parsing From address")?)
        .to(to.parse().context("parsing To address")?)
        .subject(format!("Scanned document: {filename}"))
        .multipart(
            MultiPart::mixed()
                .singlepart(SinglePart::plain("Scanned with Scan Station.".to_string()))
                .singlepart(attachment),
        )
        .context("building email")?;

    let creds = Credentials::new(cfg.username.clone(), cfg.password.clone());
    let builder = if cfg.starttls {
        SmtpTransport::starttls_relay(&cfg.smtp_host)?
    } else {
        SmtpTransport::relay(&cfg.smtp_host)?
    };
    let mailer = builder.port(cfg.smtp_port).credentials(creds).build();
    mailer.send(&email).context("sending email")?;
    Ok(())
}

/// Send `pdf` to each requested destination id. Never returns early on failure;
/// every target gets an entry in the result.
pub fn dispatch(cfg: &Config, targets: &[String], pdf: &[u8], meta: &DocMeta) -> DispatchResult {
    let mut out = DispatchResult::new();
    for t in targets {
        let r: Result<String> = match t.as_str() {
            "paperless" => match &cfg.paperless {
                Some(c) => to_paperless(c, pdf, meta),
                None => Err(anyhow::anyhow!("paperless not configured")),
            },
            "nextcloud" => match &cfg.nextcloud {
                Some(c) => to_nextcloud(c, pdf, &meta.filename).map(|_| "uploaded".into()),
                None => Err(anyhow::anyhow!("nextcloud not configured")),
            },
            "email" => match &cfg.email {
                Some(c) => to_email(c, pdf, &meta.filename, meta.recipient.as_deref())
                    .map(|_| "sent".into()),
                None => Err(anyhow::anyhow!("email not configured")),
            },
            other => Err(anyhow::anyhow!("unknown destination {other}")),
        };
        out.push((t.clone(), r.map_err(|e| e.to_string())));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paperless_url_normalizes_trailing_slash() {
        assert_eq!(
            paperless_post_url("http://p/"),
            "http://p/api/documents/post_document/"
        );
        assert_eq!(
            paperless_post_url("http://p"),
            "http://p/api/documents/post_document/"
        );
    }

    #[test]
    fn dav_base_uses_username() {
        let cfg = NextcloudConfig {
            base_url: "https://nc/".into(),
            username: "scanner".into(),
            password: "pw".into(),
            folder: "Scans".into(),
        };
        assert_eq!(
            nextcloud_dav_base(&cfg),
            "https://nc/remote.php/dav/files/scanner"
        );
    }

    #[test]
    fn dispatch_reports_unconfigured_and_unknown() {
        let cfg = Config::default();
        let meta = DocMeta {
            title: "t".into(),
            filename: "t.pdf".into(),
            recipient: None,
        };
        let res = dispatch(&cfg, &["paperless".into(), "bogus".into()], b"x", &meta);
        assert_eq!(res.len(), 2);
        assert!(res[0].1.is_err());
        assert!(res[1]
            .1
            .as_ref()
            .unwrap_err()
            .contains("unknown destination"));
    }

    #[test]
    fn dispatch_empty_targets_is_empty() {
        let cfg = Config::default();
        let meta = DocMeta {
            title: "t".into(),
            filename: "t.pdf".into(),
            recipient: None,
        };
        assert!(dispatch(&cfg, &[], b"x", &meta).is_empty());
    }
}
