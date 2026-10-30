# Shared helpers for the scripts in this directory. Source, don't run.

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APK="$ROOT/android/app/build/outputs/apk/debug/app-debug.apk"
APP_ID="xyz.nshnt.ferrokey"
SERVER_PORT=47800

say() { printf '\033[1m==> %s\033[0m\n' "$*"; }
die() { printf '\033[31merror:\033[0m %s\n' "$*" >&2; exit 1; }

# Prints the Android SDK directory, looking where Gradle and Android Studio do.
find_sdk() {
    local props="$ROOT/android/local.properties" dir
    for dir in "${ANDROID_HOME:-}" "${ANDROID_SDK_ROOT:-}" \
        "$( [[ -f $props ]] && sed -n 's/^sdk\.dir=//p' "$props" )" \
        "$HOME/Android/Sdk" "$HOME/Library/Android/sdk"; do
        if [[ -n $dir && -d $dir ]]; then
            echo "$dir"
            return
        fi
    done
    return 1
}
