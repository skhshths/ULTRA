#!/bin/sh
# Type-check the Mac app from Windows (nothing is linked or run). Needs the Rust target once:
#   rustup target add aarch64-apple-darwin
# The two C dependencies (SQLite, objc2's exception helper) can't be compiled for macOS here;
# cc.bat / ar.bat stand in for the compiler and archiver and just write empty files, which is
# enough for `cargo check`.
here=$(cd "$(dirname "$0")" && pwd -W 2>/dev/null || pwd)
cd "$(dirname "$0")/../../mac" || exit 1
CC_aarch64_apple_darwin="$here/cc.bat" AR_aarch64_apple_darwin="$here/ar.bat" \
  cargo check --release --target aarch64-apple-darwin "$@"
