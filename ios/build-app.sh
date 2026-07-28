#!/usr/bin/env bash
# Builds Connect.app for the iOS Simulator: xcodebuild against the bare
# Package.swift (no .xcodeproj needed), then wraps the resulting executable
# into a self-contained .app bundle with its own copy of
# libmessaging_core.dylib (load path rewritten to be bundle-relative).
# Run shared/build-core-apple.sh first if core/ changed.
#
# Usage: ./build-app.sh <simulator-udid>
set -euo pipefail
cd "$(dirname "$0")"  # ios/

UDID="${1:?Usage: ./build-app.sh <simulator-udid>}"

echo "==> Building Connect (debug, iOS Simulator)"
xcodebuild -scheme Connect -destination "platform=iOS Simulator,id=$UDID" build

BINARY=$(find ~/Library/Developer/Xcode/DerivedData/ios-*/Build/Products/Debug-iphonesimulator/Connect \
  -type f -print0 | xargs -0 ls -t | head -1)
DYLIB_SRC=$(otool -L "$BINARY" | awk '/libmessaging_core\.dylib/{print $1}')

APP="Connect.app"
rm -rf "$APP"
mkdir "$APP"

cp "$BINARY" "$APP/Connect"
cp "$DYLIB_SRC" "$APP/libmessaging_core.dylib"

echo "==> Rewriting dylib load paths to be relative to the bundle"
install_name_tool -id "@executable_path/libmessaging_core.dylib" "$APP/libmessaging_core.dylib"
install_name_tool -change "$DYLIB_SRC" "@executable_path/libmessaging_core.dylib" "$APP/Connect"

cat > "$APP/Info.plist" << 'EOF'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key>
    <string>Connect</string>
    <key>CFBundleIdentifier</key>
    <string>com.messagingapp.connect.ios</string>
    <key>CFBundleName</key>
    <string>Connect</string>
    <key>CFBundleDisplayName</key>
    <string>Connect</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>0.1.0</string>
    <key>CFBundleVersion</key>
    <string>1</string>
    <key>DTPlatformName</key>
    <string>iphonesimulator</string>
    <key>MinimumOSVersion</key>
    <string>17.0</string>
    <key>UIDeviceFamily</key>
    <array>
        <integer>1</integer>
        <integer>2</integer>
    </array>
    <key>UILaunchScreen</key>
    <dict/>
    <key>UISupportedInterfaceOrientations</key>
    <array>
        <string>UIInterfaceOrientationPortrait</string>
    </array>
    <key>UIApplicationSceneManifest</key>
    <dict>
        <key>UIApplicationSupportsMultipleScenes</key>
        <false/>
        <key>UISceneConfigurations</key>
        <dict>
            <key>UIWindowSceneSessionRoleApplication</key>
            <array>
                <dict>
                    <key>UISceneConfigurationName</key>
                    <string>Default Configuration</string>
                </dict>
            </array>
        </dict>
    </dict>
</dict>
</plist>
EOF

echo "==> Codesigning"
codesign --force --sign - "$APP"

echo "==> Verifying no absolute build-path dependency remains"
otool -L "$APP/Connect" | grep -i messaging

echo "==> Installing on simulator $UDID"
xcrun simctl uninstall "$UDID" com.messagingapp.connect.ios 2>/dev/null || true
xcrun simctl install "$UDID" "$APP"

echo "==> Done: $APP is self-contained and installed."
