#!/usr/bin/env bash
# Builds Connect.app for macOS as a self-contained bundle: the Swift
# executable plus its own copy of libmessaging_core.dylib, with the
# executable's load path rewritten to find it relative to the bundle
# instead of the absolute build path Swift links by default. Run
# shared/build-core-apple.sh first if core/ changed.
set -euo pipefail
cd "$(dirname "$0")"  # macos/

echo "==> Building Connect (debug)"
swift build

BUILD_DIR=".build/arm64-apple-macosx/debug"
DYLIB_SRC=$(otool -L "$BUILD_DIR/Connect" | awk '/libmessaging_core\.dylib/{print $1}')

APP="Connect.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Frameworks"

cp "$BUILD_DIR/Connect" "$APP/Contents/MacOS/Connect"
cp "$DYLIB_SRC" "$APP/Contents/Frameworks/libmessaging_core.dylib"

echo "==> Rewriting dylib load paths to be relative to the bundle"
install_name_tool -id "@executable_path/../Frameworks/libmessaging_core.dylib" \
  "$APP/Contents/Frameworks/libmessaging_core.dylib"
install_name_tool -change "$DYLIB_SRC" "@executable_path/../Frameworks/libmessaging_core.dylib" \
  "$APP/Contents/MacOS/Connect"

cat > "$APP/Contents/Info.plist" << 'EOF'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key>
    <string>Connect</string>
    <key>CFBundleIdentifier</key>
    <string>com.messagingapp.connect</string>
    <key>CFBundleName</key>
    <string>Connect</string>
    <key>CFBundleDisplayName</key>
    <string>Connect</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>0.1.0</string>
    <key>LSMinimumSystemVersion</key>
    <string>13.0</string>
    <key>NSPrincipalClass</key>
    <string>NSApplication</string>
    <key>NSHighResolutionCapable</key>
    <true/>
</dict>
</plist>
EOF

echo "==> Codesigning"
codesign --force --sign - "$APP/Contents/Frameworks/libmessaging_core.dylib"
codesign --force --sign - "$APP"

echo "==> Verifying no absolute build-path dependency remains"
otool -L "$APP/Contents/MacOS/Connect" | grep -i messaging

echo "==> Done: $APP is self-contained."
