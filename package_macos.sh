#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$ROOT_DIR"

VERSION="$(grep '^version' Cargo.toml | head -n1 | sed -E 's/.*"([^"]+)".*/\1/')"

echo "Building release binaries for macOS..."
cargo build --release

ARCH="$(lipo -archs "$ROOT_DIR/target/release/ssm" 2>/dev/null | tr ' ' '-' || uname -m)"

DIST_DIR="$ROOT_DIR/dist"
APP_NAME="Simple Serial Monitor.app"
APP_DIR="$DIST_DIR/$APP_NAME"
ZIP_PATH="$DIST_DIR/SimpleSerialMonitor-macos-${ARCH}.zip"

rm -rf "$APP_DIR" "$ZIP_PATH"
mkdir -p "$APP_DIR/Contents/MacOS" "$APP_DIR/Contents/Resources"

cp "$ROOT_DIR/target/release/ssm" "$APP_DIR/Contents/MacOS/ssm"
cp "$ROOT_DIR/target/release/ssm-cli" "$APP_DIR/Contents/MacOS/ssm-cli"
chmod +x "$APP_DIR/Contents/MacOS/ssm" "$APP_DIR/Contents/MacOS/ssm-cli"

# Generate AppIcon.icns from assets/app.ico using built-in macOS sips + iconutil
ICON_ENTRY=""
if [ -f "$ROOT_DIR/assets/app.ico" ] && command -v sips >/dev/null && command -v iconutil >/dev/null; then
    ICONSET_DIR="$DIST_DIR/AppIcon.iconset"
    rm -rf "$ICONSET_DIR"
    mkdir -p "$ICONSET_DIR"
    if sips -s format png "$ROOT_DIR/assets/app.ico" --out "$ICONSET_DIR/base.png" >/dev/null 2>&1; then
        for sz in 16 32 128 256 512; do
            sips -z "$sz" "$sz" "$ICONSET_DIR/base.png" --out "$ICONSET_DIR/icon_${sz}x${sz}.png" >/dev/null 2>&1 || true
            dbl=$((sz * 2))
            sips -z "$dbl" "$dbl" "$ICONSET_DIR/base.png" --out "$ICONSET_DIR/icon_${sz}x${sz}@2x.png" >/dev/null 2>&1 || true
        done
        rm -f "$ICONSET_DIR/base.png"
        if iconutil -c icns "$ICONSET_DIR" -o "$APP_DIR/Contents/Resources/AppIcon.icns" >/dev/null 2>&1; then
            ICON_ENTRY="<key>CFBundleIconFile</key><string>AppIcon</string>"
        fi
    fi
    rm -rf "$ICONSET_DIR"
fi

cat > "$APP_DIR/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>Simple Serial Monitor</string>
    <key>CFBundleDisplayName</key>
    <string>Simple Serial Monitor</string>
    <key>CFBundleIdentifier</key>
    <string>com.goukun5160.simple-serial-monitor</string>
    <key>CFBundleVersion</key>
    <string>${VERSION}</string>
    <key>CFBundleShortVersionString</key>
    <string>${VERSION}</string>
    <key>CFBundleExecutable</key>
    <string>ssm</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>LSMinimumSystemVersion</key>
    <string>11.0</string>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>NSPrincipalClass</key>
    <string>NSApplication</string>
    ${ICON_ENTRY}
</dict>
</plist>
EOF

# Ad-hoc sign so Apple Silicon macOS runs the bundle cleanly
if command -v codesign >/dev/null; then
    codesign --force --deep --sign - "$APP_DIR" >/dev/null 2>&1 || true
fi

ditto -c -k --sequesterRsrc --keepParent "$APP_DIR" "$ZIP_PATH"

SSM_KB=$(awk "BEGIN { printf \"%.1f\", $(stat -f%z "$APP_DIR/Contents/MacOS/ssm") / 1024 }")
CLI_KB=$(awk "BEGIN { printf \"%.1f\", $(stat -f%z "$APP_DIR/Contents/MacOS/ssm-cli") / 1024 }")
ZIP_KB=$(awk "BEGIN { printf \"%.1f\", $(stat -f%z "$ZIP_PATH") / 1024 }")
APP_KB=$(du -sk "$APP_DIR" | awk '{print $1}')

echo ""
echo "=== macOS App Bundle Ready ==="
echo "  ssm         : ${SSM_KB} KB"
echo "  ssm-cli     : ${CLI_KB} KB"
echo "  .app bundle : ${APP_DIR} (${APP_KB} KB)"
echo "  ZIP archive : ${ZIP_PATH} (${ZIP_KB} KB)"
