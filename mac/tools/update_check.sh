#!/bin/sh
# The checks around a Check now update test that need no clicks. Run `backup` before the test, click
# Check now in the app (Settings closes to install), then run `verify` with the version you expect.
#
#   update_check.sh backup APP DIR             copies the app's settings folder into DIR
#   update_check.sh verify APP VERSION DIR     checks the installed app and compares settings with DIR
#
# APP is the bundle name without .app (MiniBar, JuiceBar or NetSpeedGauge). Settings are never
# deleted; a difference is printed for you to restore by hand. last_update_check always differs,
# because it records the real check.
set -u

APP_SUPPORT="$HOME/Library/Application Support"

case "${1:-}" in
backup)
    app="$2" dir="$3"
    mkdir -p "$dir" && cp -pR "$APP_SUPPORT/$app/." "$dir/" && echo "backed up $APP_SUPPORT/$app to $dir"
    ;;
verify)
    app="$2" want="$3" dir="$4" bundle="/Applications/$2.app" failed=0
    have=$(/usr/libexec/PlistBuddy -c "Print CFBundleShortVersionString" "$bundle/Contents/Info.plist" 2>/dev/null)
    if [ "$have" = "$want" ]; then echo "ok   version $have"; else echo "FAIL version $have, expected $want"; failed=1; fi
    if pgrep -x "$app" >/dev/null; then echo "ok   running again"; else echo "FAIL not running"; failed=1; fi
    if codesign --verify --deep "$bundle" 2>/dev/null; then echo "ok   signature valid"; else echo "FAIL signature"; failed=1; fi
    for leftover in "$bundle.old" "${TMPDIR:-/tmp}/$app-update"; do
        if [ -e "$leftover" ]; then echo "FAIL leftover $leftover"; failed=1; fi
    done
    for file in "$dir"/settings.*; do
        name=$(basename "$file")
        changes=$(diff "$file" "$APP_SUPPORT/$app/$name" | grep '^[<>]' | grep -v 'last_update_check=')
        if [ -z "$changes" ]; then echo "ok   $name matches the backup (last_update_check aside)"; else echo "FAIL $name differs:"; echo "$changes"; failed=1; fi
    done
    exit "$failed"
    ;;
*)
    sed -n '2,10p' "$0"
    exit 1
    ;;
esac
