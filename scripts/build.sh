#!/usr/bin/env bash
# Builds the server and client (release) and the Android app (debug APK).
#
# Usage: scripts/build.sh
set -euo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/common.sh"

command -v cargo >/dev/null || die "cargo not found; install Rust from https://rustup.rs"
command -v cargo-ndk >/dev/null || die "cargo-ndk not found; run: cargo install cargo-ndk"
installed_targets="$(rustup target list --installed 2>/dev/null || true)"
for target in aarch64-linux-android x86_64-linux-android; do
    grep -qx "$target" <<<"$installed_targets" \
        || die "Rust target $target missing; run: rustup target add aarch64-linux-android x86_64-linux-android"
done
sdk="$(find_sdk)" || die "Android SDK not found; set ANDROID_HOME or sdk.dir in android/local.properties"
export ANDROID_HOME="$sdk"

say "Building server and client"
cargo build --release --manifest-path "$ROOT/Cargo.toml" -p ferrokey-server -p ferrokey-client

say "Building Android app"
(cd "$ROOT/android" && ./gradlew assembleDebug)

say "Done"
echo "  server: $ROOT/target/release/ferrokey-server"
echo "  app:    $APK"
