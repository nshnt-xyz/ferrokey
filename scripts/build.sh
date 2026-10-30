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
# Gradle compiles with the JDK it runs on, so a runtime-only install fails
# deep inside the build. Catch it here instead.
if [[ -n ${JAVA_HOME:-} ]]; then
    java_home="$JAVA_HOME"
else
    java="$(command -v java)" || die "Java not found; install a JDK, e.g.: sudo apt install openjdk-21-jdk"
    java_home="$(dirname "$(dirname "$(readlink -f "$java")")")"
fi
if [[ ! -x $java_home/bin/javac ]]; then
    version="$(basename "$java_home" | grep -oE '[0-9]+' | head -1)"
    die "$java_home is a Java runtime without a compiler; install the JDK, e.g.: sudo apt install openjdk-${version:-21}-jdk"
fi
sdk="$(find_sdk)" || die "Android SDK not found; set ANDROID_HOME or sdk.dir in android/local.properties"
export ANDROID_HOME="$sdk"

# Gradle doesn't download the NDK for us, since the Rust build (not Gradle)
# is what uses it. Install the version the app pins if it's missing.
ndk_version="$(sed -n 's/^ *ndkVersion = "\(.*\)"/\1/p' "$ROOT/android/app/build.gradle.kts")"
[[ -n $ndk_version ]] || die "could not read ndkVersion from android/app/build.gradle.kts"
if [[ ! -d $sdk/ndk/$ndk_version ]]; then
    sdkmanager="$(command -v sdkmanager || true)"
    [[ -n $sdkmanager ]] || sdkmanager="$(ls "$sdk"/cmdline-tools/*/bin/sdkmanager 2>/dev/null | sort -V | tail -1 || true)"
    [[ -x $sdkmanager ]] \
        || die "NDK $ndk_version is not installed and sdkmanager wasn't found. Install it from Android Studio (SDK Manager > SDK Tools > NDK (Side by side), tick \"Show Package Details\"), or install the \"Android SDK Command-line Tools\" there and rerun"
    say "Installing NDK $ndk_version"
    "$sdkmanager" --sdk_root="$sdk" "ndk;$ndk_version"
fi

say "Building server and client"
cargo build --release --manifest-path "$ROOT/Cargo.toml" -p ferrokey-server -p ferrokey-client

say "Building Android app"
(cd "$ROOT/android" && ./gradlew assembleDebug)

say "Done"
echo "  server: $ROOT/target/release/ferrokey-server"
echo "  app:    $APK"
