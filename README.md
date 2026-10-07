# scan-station

A touch-screen scanning station for the kiosk Raspberry Pi. Feed a document into
the scanner, watch the pages appear, then send the whole thing as one PDF to
Paperless, Nextcloud, or email — with a UI modelled on a network copier so it's
familiar to anyone who's used an office MFP.

Built as a [Tauri](https://tauri.app) app (Rust backend + web frontend) so it
can run as a native desktop binary, but it ships as a container that runs full
screen under `cage` on the cluster node `kiosk-1` (Raspberry Pi 4 + Waveshare 5"
HDMI AMOLED, 960×544, USB-HID touch).

## Hardware & driver

The target scanner is an **HP ScanJet Pro 2000 s2** (USB sheet-feed, ADF
duplex). It is driverless over **eSCL**, so the image uses the firmware-free
SANE stack:

- **ipp-usb** — bridges the USB-only eSCL interface onto a local HTTP endpoint.
- **avahi-daemon** — lets ipp-usb advertise it and sane-airscan discover it.
- **sane-airscan** — the eSCL/WSD SANE backend; surfaces the device as `airscan:*`.
- **sane-utils** — provides `scanimage`, which the app drives.

If airscan/ipp-usb ever misbehaves on a particular unit, HPLIP's `hpaio` backend
([hplip](https://sourceforge.net/projects/hplip/)) is the documented fallback;
add `hplip` to the runtime stage of the `Dockerfile`.

## Layout

```
core/       scanstation-core — scanner, PDF assembly, upload. No GUI deps, so
            it compiles and unit-tests anywhere (this is where the logic lives).
src-tauri/  the Tauri shell: holds job state, exposes commands to the frontend.
ui/         static copier-style frontend (no bundler) — HTML/CSS/vanilla JS.
Dockerfile  arm64 kiosk image (compile → cage + webkit + SANE stack).
```

`core` has no dependency on Tauri or webkit; `src-tauri` is a thin wrapper. The
split keeps the testable logic fast to build and verify.

## Scanning flow

1. **Scan** runs an ADF batch (`scanimage --batch`) in the chosen source
   (1-sided / 2-sided / glass), colour mode, and resolution.
2. Pages append to the current job; each shows as a thumbnail. **Scan More**
   adds to the same document; the ✕ on a page removes it.
3. **Send** assembles the pages into one PDF (JPEGs embedded via DCTDecode — no
   re-encode) and delivers it to every selected destination.
4. **Clear** discards the job.

## Configuration

Everything is environment variables (secrets come from a Kubernetes Secret). A
destination only appears as available in the UI when all its fields are set.

| Destination | Variables |
|-------------|-----------|
| Paperless   | `PAPERLESS_URL`, `PAPERLESS_TOKEN` |
| Nextcloud   | `NEXTCLOUD_URL`, `NEXTCLOUD_USER`, `NEXTCLOUD_PASS`, `NEXTCLOUD_FOLDER` (default `Scans`) |
| Email       | `SMTP_HOST`, `SMTP_USER`, `SMTP_PASS` (required); `SMTP_PORT` (587), `SMTP_FROM` (defaults to user), `SMTP_TO` (optional default recipient), `SMTP_STARTTLS` (true), `SMTP_INSECURE_TLS` (false) |
| Scan default | `SCAN_RESOLUTION` (300) |

For native/desktop use a TOML file at `$SCAN_STATION_CONFIG` (or
`/etc/scan-station/config.toml`) is read first; env vars overlay it.

### Paperless service account (minimal permissions)

Paperless-ngx issues one API token per user, so give the station its **own**
dedicated user rather than a person's account (a shared token breaks whatever
else uses it). That user needs only enough to push documents in via
`POST /api/documents/post_document/`:

- **Not** staff, **not** superuser.
- Global permission **`Documents: Add document`** only
  (Django `documents.add_document`). That is sufficient for the upload endpoint;
  the uploaded file is queued to the consumer and owned by this user.
- Optionally also grant `Documents: View document` if you want the account to
  see what it submitted in the UI — not required for scanning.

Then generate that user's API token (Paperless **Settings → My Profile → API
Token**, or via the admin) and put it in `PAPERLESS_TOKEN`. Create the user and
grant the single permission from **Settings → Users & Groups** (or a Group with
just that permission, with the user as a member).

## Build & run

```sh
# logic only — fast, no GUI toolkit needed
cargo test -p scanstation-core

# the full app (needs the Tauri system deps: libwebkit2gtk-4.1-dev, etc.)
cargo run -p scanstation-app

# the kiosk container (arm64)
docker build -t scan-station .
```

## CI / deployment

GitHub Actions (`ci`) runs fmt + clippy + tests on every PR — this is the merge
gate and must be green. On a push to `main` it builds the arm64 image and
pushes it to `ghcr.io/xerootg/scan-station` (`:main` + `:sha-…`).

Deployment is GitOps via `argo-things`: a kiosk Deployment pinned to `kiosk-1`
runs the image, and **argocd-image-updater** follows the `:main` digest so each
green build rolls out automatically.
