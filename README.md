# ferrokey
Cross-platform remote mouse and keyboard control (Linux Wayland + Windows), written in Rust

## Quick start

```sh
scripts/build.sh   # server + client (release) and the Android APK (debug)
scripts/run.sh     # build, install the app on connected phones, start the server
```

`scripts/run.sh` installs and launches the app on every phone connected to
adb, over USB or wireless debugging (emulators are skipped), and sets up
`adb reverse` so the phone can reach the server at `127.0.0.1` through adb.
That works even where the phone can't reach the computer directly, since adb
connects from the computer to the phone. It then runs the server in the
foreground; extra arguments go to the server, e.g. `scripts/run.sh --pair` or
`scripts/run.sh --no-build --print-events`. The forward is for the default
port 47800.

To use wireless debugging (Android 11+), turn it on under Developer options
and connect once with `adb pair <ip:port>` (the "Pair device with pairing
code" dialog) and `adb connect <ip:port>` (the port on the Wireless debugging
screen).

Both scripts need the Android build prerequisites listed under
[Android app](#android-app). They're bash, so on Windows run them from WSL or
Git Bash.

## Running the server

```sh
cargo run --release -p ferrokey-server
```

The server listens on TCP port 47800 (`--listen` to change). On first run, or
with `--pair`, it prints a six-digit pairing code; enter it in the app along
with the computer's address. The code works once, and pairing closes after
three wrong attempts. `--print-events` prints received events instead of
injecting them, which is handy for testing the app.

The server's key and the list of paired devices live in
`$XDG_CONFIG_HOME/ferrokey` (`%APPDATA%\ferrokey` on Windows). Delete a line
from `devices` and restart the server to unpair a phone.

## Security

All traffic is encrypted and authenticated with the
[Noise protocol](https://noiseprotocol.org) (`XX` handshake, X25519,
ChaCha20-Poly1305, BLAKE2s). Each side has a long-term key: the server only
accepts phones it has paired with, and the phone refuses a server whose key
changed since pairing.

Pairing runs [SPAKE2](https://datatracker.ietf.org/doc/rfc9382/) with the
pairing code and feeds the result into the Noise handshake as a pre-shared
key. Someone watching the network can't test guesses offline, and someone
actively attacking gets one guess per attempt (three in total). The protocol
is described in `crates/ferrokey-net/src/lib.rs`.

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

## Android app

`android/` is a Kotlin + Jetpack Compose app. Its core is the
`ferrokey-mobile` Rust crate, exposed to Kotlin through
[UniFFI](https://mozilla.github.io/uniffi-rs/), so the app and the server
share the same protocol types. The Gradle build cross-compiles the crate with
[cargo-ndk](https://github.com/bbqsrc/cargo-ndk) and generates the Kotlin
bindings automatically.

Prerequisites:

```sh
rustup target add aarch64-linux-android x86_64-linux-android
cargo install cargo-ndk
```

plus a JDK (17 or newer, with `javac`; on Debian/Ubuntu `openjdk-21-jdk`, not
just the `-jre`) and the Android SDK (`ANDROID_HOME` or
`android/local.properties`). `scripts/build.sh` installs the NDK version
pinned in `app/build.gradle.kts` if it's missing, using the SDK's
`sdkmanager` (Android Studio: SDK Manager > SDK Tools > Android SDK
Command-line Tools).

```sh
cd android
./gradlew assembleDebug
```

The app pairs with and connects to a server, then shows a touchpad
(drag to move, tap to click, tap then touch again and
hold to drag with the left button down) and left/right buttons. Servers are entered by
address for now; there's no automatic discovery yet.
