#!/usr/bin/env bash
# Bring up the driver stack, then launch the app full-screen under cage.
# Not `set -e`: the service bring-up is best-effort and must never block the
# final `exec cage`, which is the only thing that must succeed.
set -uxo pipefail

: "${XDG_RUNTIME_DIR:=/run/user/0}"
export XDG_RUNTIME_DIR
mkdir -p "$XDG_RUNTIME_DIR" && chmod 700 "$XDG_RUNTIME_DIR" || true

# --- D-Bus system bus (Avahi needs it) ---
mkdir -p /run/dbus
if [ ! -S /run/dbus/system_bus_socket ]; then
  dbus-daemon --system --fork || true
fi

# --- Avahi: ipp-usb advertises the scanner over mDNS; sane-airscan discovers it ---
avahi-daemon --no-drop-root --daemonize --no-chroot || true

# --- ipp-usb: expose the USB-only eSCL scanner on a local HTTP endpoint ---
# Needs /dev/bus/usb (hostPath) and privileged. Runs in the background; it keeps
# serving as devices come and go.
if command -v ipp-usb >/dev/null 2>&1; then
  ipp-usb standalone >/var/log/ipp-usb.log 2>&1 &
fi

# Give discovery a moment, then log what SANE sees (non-fatal).
sleep 3
scanimage -L 2>&1 | sed -n '1,10p' || echo "scanimage -L found nothing yet (will retry at scan time)"

# --- udev: wlroots/libinput enumerate input devices (touchscreen, keyboard)
# through udev. There is no udevd in a bare container, so start one and populate
# the device db; otherwise cage aborts with "libinput: no input devices".
for udevd in /usr/lib/systemd/systemd-udevd /lib/systemd/systemd-udevd; do
  [ -x "$udevd" ] && { "$udevd" --daemon 2>/dev/null; break; }
done
udevadm trigger --action=add 2>/dev/null || true
udevadm settle --timeout=5 2>/dev/null || true
# Safety: let cage start even if input enumeration lags; udev hotplug still
# attaches the touchscreen once it appears.
export WLR_LIBINPUT_NO_DEVICES=1

# --- pick the KMS display device for cage ---
# The Pi 4 exposes two DRM nodes: vc4 (the HDMI display, has connectors) and
# v3d (render-only, no connectors). wlroots must scan out on the vc4 node or its
# backend aborts ("Unable to start the wlroots backend"). Card numbering can
# swap across boots, so detect the connector-bearing node rather than hardcode
# it, and point WLR_DRM_DEVICES at it. GL rendering still uses the render node
# (renderD128) automatically.
if [ -z "${WLR_DRM_DEVICES:-}" ]; then
  for card in /sys/class/drm/card[0-9]*; do
    cd=$(basename "$card")
    for conn in "$card/$cd"-*; do
      if [ -e "$conn" ]; then
        export WLR_DRM_DEVICES="/dev/dri/$cd"
        break 2
      fi
    done
  done
fi
echo "WLR_DRM_DEVICES=${WLR_DRM_DEVICES:-<unset: no KMS connector found>}"

# --- launch: cage full-screens its single client and exits when it exits ---
exec cage -- /usr/local/bin/scanstation-app
