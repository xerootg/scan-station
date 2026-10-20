#!/bin/sh
# Install an Xcursor theme whose every cursor is a single transparent pixel, so
# cage never draws a pointer. The Waveshare touch panel enumerates as a *mouse*
# (mouse0), so the compositor would otherwise draw its own cursor on touch and
# before the app maps; the CSS `cursor: none` only covers the app's surface.
set -eu
d=/usr/share/icons/blank/cursors
mkdir -p "$d"
le32() { printf "$(printf '\\%03o\\%03o\\%03o\\%03o' $(($1 & 255)) $((($1 >> 8) & 255)) $((($1 >> 16) & 255)) $((($1 >> 24) & 255)))"; }
{
  printf 'Xcur'
  le32 16; le32 65536; le32 1       # header size, version, ntoc
  le32 4294770690; le32 24; le32 28 # image chunk type 0xfffd0002, subtype (nominal size), offset
  le32 36; le32 4294770690; le32 24; le32 1 # chunk header size, type, subtype, version
  le32 24; le32 24; le32 0; le32 0; le32 0  # width, height, xhot, yhot, delay
  head -c 2304 /dev/zero                     # 24x24 fully transparent ARGB pixels
} > "$d/left_ptr"
for n in default arrow pointer hand hand1 hand2 text xterm ibeam watch wait progress \
         crosshair cross move grab grabbing fleur help question_arrow not-allowed \
         X_cursor top_left_arrow right_ptr context-menu all-scroll; do
  ln -sf left_ptr "$d/$n"
done
printf '[Icon Theme]\nName=blank\nComment=Invisible cursors\n' > /usr/share/icons/blank/index.theme

# The page's GTK/WebKit client loads its OWN cursor from GTK's cursor-theme
# setting (default theme "default" -> Inherits=Adwaita, i.e. a visible arrow);
# XCURSOR_THEME only steers cage itself. Point both at the blank theme.
mkdir -p /usr/share/icons/default /etc/xdg/gtk-3.0
printf '[Icon Theme]\nInherits=blank\n' > /usr/share/icons/default/index.theme
printf '[Settings]\ngtk-cursor-theme-name=blank\ngtk-cursor-theme-size=24\n' > /etc/xdg/gtk-3.0/settings.ini

# wlroots has its own xcursor loader and showed a real arrow (software-composited
# into the primary plane, no cursor plane in use) even with the blank theme in
# place, so also remove every other cursor theme: if nothing else exists it can
# only find, or fall back to, the transparent one.
rm -rf /usr/share/icons/Adwaita/cursors /usr/share/icons/AdwaitaLegacy
