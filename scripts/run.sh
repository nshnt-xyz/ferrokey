#!/usr/bin/env bash
# Builds everything, installs and launches the app on every USB-connected
# Android device, then runs the server in the foreground.
#
# Usage: scripts/run.sh [--no-build] [SERVER ARGS...]
#
# Server arguments are passed through, e.g. --pair or --print-events.
# Each phone also gets `adb reverse`, so over USB it can reach the server at
# 127.0.0.1 without being on the same network.
set -euo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/common.sh"

build=1
if [[ ${1:-} == --no-build ]]; then
    build=0
    shift
fi
if ((build)); then
    "$ROOT/scripts/build.sh"
fi

server="$ROOT/target/release/ferrokey-server"
[[ -x $server ]] || die "server not built; run scripts/build.sh"
[[ -f $APK ]] || die "APK not built; run scripts/build.sh"

adb="$(command -v adb || true)"
if [[ -z $adb ]]; then
    sdk="$(find_sdk)" || die "adb not found; install Android platform-tools"
    adb="$sdk/platform-tools/adb"
    [[ -x $adb ]] || die "adb not found; install Android platform-tools"
fi

say "Looking for USB devices"
devices=()
while read -r serial state rest; do
    [[ $rest == *usb:* ]] || continue
    case $state in
        device) devices+=("$serial") ;;
        unauthorized) echo "  $serial: unlock the phone and accept the USB debugging prompt" >&2 ;;
        *) echo "  $serial: skipped ($state)" >&2 ;;
    esac
done < <("$adb" devices -l | tail -n +2)
((${#devices[@]})) || die "no authorized USB device; connect a phone with USB debugging enabled"

for serial in "${devices[@]}"; do
    model="$("$adb" -s "$serial" shell getprop ro.product.model | tr -d '\r')"
    say "Installing on $model ($serial)"
    "$adb" -s "$serial" install -r "$APK"
    "$adb" -s "$serial" reverse "tcp:$SERVER_PORT" "tcp:$SERVER_PORT" >/dev/null
    "$adb" -s "$serial" shell am start -n "$APP_ID/.MainActivity" >/dev/null
done

say "Starting server"
echo "  In the app, use 127.0.0.1 over USB, or this computer's network address:"
if command -v ip >/dev/null; then
    ip -4 -o addr show scope global | awk '{ sub(/\/.*/, "", $4); print "    " $4 }'
fi
exec "$server" "$@"
