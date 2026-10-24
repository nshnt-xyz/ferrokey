# ferrokey
Cross-platform remote mouse and keyboard control (Linux Wayland + Windows), written in Rust

## Linux

Input is injected through the xdg-desktop-portal
[RemoteDesktop](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.RemoteDesktop.html)
interface, which is supported on GNOME and KDE Plasma. The first run shows a
permission dialog; the restore token it returns is saved to
`$XDG_STATE_HOME/ferrokey/portal-restore-token` so later runs skip it.

### Not yet supported: wlroots compositors

Sway, Hyprland and other wlroots-based compositors don't implement the
RemoteDesktop portal, so the backend above won't work there. Ideas for a
fallback backend, used when the portal is unavailable:

- **wlroots virtual input protocols**: `zwlr_virtual_pointer_v1` and
  `zwp_virtual_keyboard_v1`. No extra permissions, but compositor-specific.
- **`/dev/uinput`**: creates a virtual kernel input device, so it works on any
  compositor (and X11/TTY), but needs write access to `/dev/uinput` (udev rule
  or `input` group).
