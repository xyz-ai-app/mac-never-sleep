#!/usr/bin/env bash
set -euo pipefail

# Package the menu-bar app on macOS. Linux CI does not run this script.

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "Run this on a Mac: ./scripts/package-macos.sh"
  exit 1
fi

cargo build -p never-sleep --release

APP_NAME="Never Sleep"
DIST="$ROOT/dist/${APP_NAME}.app"
MACOS_DIR="$DIST/Contents/MacOS"
RES_DIR="$DIST/Contents/Resources"
BIN="$ROOT/target/release/never-sleep"

rm -rf "$DIST"
mkdir -p "$MACOS_DIR" "$RES_DIR"
cp "$BIN" "$MACOS_DIR/never-sleep"
cp "$ROOT/packaging/Info.plist" "$DIST/Contents/Info.plist"
"$ROOT/scripts/bump-version.sh" --stamp-plist "$DIST/Contents/Info.plist"
cp -R "$ROOT/packaging/en.lproj" "$RES_DIR/en.lproj"
cp -R "$ROOT/packaging/zh-Hans.lproj" "$RES_DIR/zh-Hans.lproj"

# Optional icns
if [[ -f "$ROOT/packaging/AppIcon.icns" ]]; then
  cp "$ROOT/packaging/AppIcon.icns" "$RES_DIR/AppIcon.icns"
fi

chmod +x "$MACOS_DIR/never-sleep"
echo "Built $DIST"
echo "Open with: open \"$DIST\""
echo "Or copy the binary onto PATH: cp $BIN /usr/local/bin/never-sleep"
