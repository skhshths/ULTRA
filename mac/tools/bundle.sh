#!/bin/sh
# Build the app bundle (ULTRA.app, or "ULTRA Experimental.app" for the experimental build).
#
#   tools/bundle.sh             build for this Mac's architecture
#   tools/bundle.sh universal   build one app that runs on both Apple silicon and Intel
#
# The app lands in target/<name>.app (name and bundle id come from src/edition.rs). It is
# ad-hoc signed, which is enough to run it on the Mac that built it and lets macOS remember permissions (camera, audio visuals) between runs.
set -eu
cd "$(dirname "$0")/.."

edition() { sed -n "s/^pub const $1: &str = \"\(.*\)\";/\1/p" src/edition.rs; }
edition_bool() { sed -n "s/^pub const $1: bool = \(.*\);/\1/p" src/edition.rs; }
NAME=$(edition NAME)
BUNDLE_ID=$(edition BUNDLE_ID)
APP="target/$NAME.app"
if [ "${1:-}" = "universal" ]; then
    rustup target add aarch64-apple-darwin x86_64-apple-darwin
    cargo build --release --target aarch64-apple-darwin
    cargo build --release --target x86_64-apple-darwin
    BIN=target/ultra-universal
    lipo -create -output "$BIN" \
        target/aarch64-apple-darwin/release/ultra \
        target/x86_64-apple-darwin/release/ultra
else
    cargo build --release
    BIN=target/release/ultra
fi

rm -rf target/*.app
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/ultra"
cp bundle/Info.plist "$APP/Contents/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleName \"$NAME\"" -c "Set :CFBundleDisplayName \"$NAME\"" \
    -c "Set :CFBundleIdentifier $BUNDLE_ID" "$APP/Contents/Info.plist"
# The inverted-colors edition (experimental) wears the inverted icon too.
ICON=assets/ultra.icns
[ "$(edition_bool INVERT)" = "true" ] && ICON=assets/ultra-inv.icns
cp "$ICON" "$APP/Contents/Resources/ultra.icns"
codesign --force --sign - "$APP"

echo "built $APP"
echo "run it with:  open \"$APP\""
echo "install it:   cp -R \"$APP\" /Applications/"
