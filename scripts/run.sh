#!/usr/bin/env bash
# Builds everything, installs and launches the app on every Android phone
# connected to adb (over USB or wireless debugging; emulators are skipped),
# then runs the server in the foreground.
#
# Usage: scripts/run.sh [--no-build] [SERVER ARGS...]
#
# Server arguments are passed through, e.g. --pair or --print-events.
# Each phone also gets `adb reverse`, so it can reach the server at 127.0.0.1
# through adb, over the cable or wireless debugging.
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

say "Looking for devices"
# A freshly started adb daemon takes a moment to find phones, wireless ones
# especially, so give it a few seconds before giving up.
"$adb" start-server >/dev/null 2>&1
devices=()
for _ in 1 2 3 4 5; do
    seen=" "
    notes=()
    while read -r serial state _; do
        [[ $serial == emulator-* ]] && continue
        case $state in
            device)
                # A phone on both USB and wireless debugging is listed twice.
                id="$("$adb" -s "$serial" shell getprop ro.serialno </dev/null | tr -d '\r')"
                [[ $seen == *" $id "* ]] && continue
                seen+="$id "
                devices+=("$serial")
                ;;
            unauthorized) notes+=("$serial: unlock the phone and accept the debugging prompt") ;;
            *) notes+=("$serial: skipped ($state)") ;;
        esac
    done < <("$adb" devices | tail -n +2)
    ((${#devices[@]})) && break
    sleep 1
done
for note in "${notes[@]}"; do echo "  $note" >&2; done
((${#devices[@]})) || die "no authorized device; connect a phone over USB or wireless debugging"

for serial in "${devices[@]}"; do
    model="$("$adb" -s "$serial" shell getprop ro.product.model | tr -d '\r')"
    say "Installing on $model ($serial)"
    "$adb" -s "$serial" install -r "$APK"
    "$adb" -s "$serial" reverse "tcp:$SERVER_PORT" "tcp:$SERVER_PORT" >/dev/null
    "$adb" -s "$serial" shell am start -n "$APP_ID/.MainActivity" >/dev/null
done

say "Starting server"
echo "  In the app, use 127.0.0.1 (through adb), or this computer's network address:"
if command -v ip >/dev/null; then
    ip -4 -o addr show scope global | awk '{ sub(/\/.*/, "", $4); print "    " $4 }'
fi
exec "$server" "$@"
