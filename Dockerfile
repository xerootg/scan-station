# Scan Station kiosk image (arm64) for the Raspberry Pi 4 + Waveshare panel.
#
# Two stages: compile the Tauri app, then a slim runtime that runs it full
# screen under cage (a one-window Wayland compositor) with the SANE eSCL driver
# stack for the HP ScanJet Pro 2000 s2:
#   ipp-usb      bridges the USB-only eSCL scanner onto a local HTTP endpoint
#   avahi-daemon lets ipp-usb advertise and sane-airscan discover it
#   sane-airscan the driverless (eSCL/WSD) SANE backend
#   sane-utils   provides `scanimage`, which the app shells out to
#
# Fallback driver: if airscan/ipp-usb misbehaves on this unit, HPLIP's `hpaio`
# backend (package `hplip`, https://sourceforge.net/projects/hplip/) can be
# added to the runtime stage — it is not installed by default to keep the image
# lean and firmware-free.
#
# Build MUST run on arm64 (native Tauri binary). CI uses an arm64 runner.

# ---------- build ----------
FROM debian:trixie-slim AS build
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
      build-essential curl ca-certificates pkg-config file \
      libwebkit2gtk-4.1-dev libsoup-3.0-dev libjavascriptcoregtk-4.1-dev \
      libgtk-3-dev librsvg2-dev libssl-dev libxdo-dev \
 && rm -rf /var/lib/apt/lists/*
ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
      | sh -s -- -y --profile minimal --default-toolchain stable
WORKDIR /src
COPY . .
RUN cargo build --release -p scanstation-app

# ---------- runtime ----------
FROM debian:trixie-slim
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
      cage seatd \
      libwebkit2gtk-4.1-0 libgtk-3-0 libsoup-3.0-0 librsvg2-2 \
      libgl1-mesa-dri mesa-vulkan-drivers libgles2 libegl1 libgbm1 \
      sane-utils sane-airscan ipp-usb avahi-daemon \
      dbus dbus-x11 fonts-noto-core ca-certificates tzdata \
 && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/scanstation-app /usr/local/bin/scanstation-app
COPY entrypoint.sh /usr/local/bin/entrypoint.sh
RUN chmod +x /usr/local/bin/entrypoint.sh
ENV XDG_RUNTIME_DIR=/run/user/0 \
    GDK_BACKEND=wayland \
    WEBKIT_DISABLE_COMPOSITING_MODE=1 \
    WEBKIT_DISABLE_DMABUF_RENDERER=1 \
    LANG=C.UTF-8 LC_ALL=C.UTF-8
ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
