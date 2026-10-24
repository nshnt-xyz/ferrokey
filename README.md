# ferrokey
Cross-platform remote mouse and keyboard control (Linux Wayland + Windows), written in Rust

## Linux

Input is injected through the xdg-desktop-portal
[RemoteDesktop](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.RemoteDesktop.html)
interface, which is supported on GNOME and KDE Plasma. The first run shows a
permission dialog; the restore token it returns is saved to
`$XDG_STATE_HOME/ferrokey/portal-restore-token` so later runs skip it.
