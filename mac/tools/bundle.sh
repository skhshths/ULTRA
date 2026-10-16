#!/bin/sh
# Build ULTRA.app.
#
#   tools/bundle.sh             build for this Mac's architecture
#   tools/bundle.sh universal   build one app that runs on both Apple silicon and Intel
#
# The app lands in target/ULTRA.app. It is ad-hoc signed, which is enough to run it on the
# Mac that built it and lets macOS remember permissions (camera, audio visuals) between runs.
set -eu
cd "$(dirname "$0")/.."

APP=target/ULTRA.app
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

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/ultra"
cp bundle/Info.plist "$APP/Contents/Info.plist"
cp assets/ultra.icns "$APP/Contents/Resources/ultra.icns"
codesign --force --sign - "$APP"

echo "built $APP"
echo "run it with:  open $APP"
echo "install it:   cp -R $APP /Applications/"
