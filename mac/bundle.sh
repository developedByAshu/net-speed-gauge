#!/bin/sh
# Builds target/mac/NetSpeedGauge.app for Apple Silicon. Run from anywhere.
set -e
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
# The release workflow runs its own tests, because some here need a Mac with real network traffic.
[ -n "$SKIP_TESTS" ] || cargo test --locked -p netspeedgauge-shared -p netspeedgauge-mac
cargo build --locked --release --target aarch64-apple-darwin -p netspeedgauge-mac --bin NetSpeedGauge
version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
app="target/mac/NetSpeedGauge.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp target/aarch64-apple-darwin/release/NetSpeedGauge "$app/Contents/MacOS/NetSpeedGauge"
sed "s/VERSION_PLACEHOLDER/$version/g" mac/Info.plist > "$app/Contents/Info.plist"
icons="$(mktemp -d)/NetSpeedGauge.iconset"
mkdir -p "$icons"
for size in 16 32 128; do
    sips -z "$size" "$size" shared/assets/netspeedgauge.png --out "$icons/icon_${size}x${size}.png" > /dev/null
    double=$((size * 2))
    sips -z "$double" "$double" shared/assets/netspeedgauge.png --out "$icons/icon_${size}x${size}@2x.png" > /dev/null
done
iconutil -c icns "$icons" -o "$app/Contents/Resources/NetSpeedGauge.icns"
# Sign with the developedByAshu certificate when this Mac has it, so every release carries the same
# designated requirement and the updater only accepts a new copy signed by that certificate. Without
# it the app is signed ad hoc, which still runs here but cannot update itself.
identity="${MAC_SIGN_IDENTITY:-developedByAshu Code Signing}"
bundle_id="$(/usr/libexec/PlistBuddy -c 'Print CFBundleIdentifier' "$app/Contents/Info.plist")"
if security find-identity -p codesigning | grep -q "\"$identity\""; then
    cert_sha1="$(security find-certificate -c "$identity" -Z | awk '/SHA-1 hash:/ { print $3; exit }')"
    requirement="designated => identifier \"$bundle_id\" and certificate leaf = H\"$cert_sha1\""
    codesign --force --options runtime --identifier "$bundle_id" --requirements "=$requirement" --sign "$identity" "$app"
else
    echo "WARNING: signing identity \"$identity\" not found; signing ad hoc. This build cannot update itself." >&2
    codesign --force --options runtime --sign - "$app"
fi
codesign --verify --deep --strict "$app"
echo "Built $root/$app"
