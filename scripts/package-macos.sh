#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -ne 4 ]; then
  echo "Usage: package-macos.sh <x86_64-binary> <arm64-binary> <version> <dist-directory>"
  exit 2
fi

x86_binary="$(realpath "$1")"
arm_binary="$(realpath "$2")"
version="$3"
dist_directory="$(mkdir -p "$4" && realpath "$4")"
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
signing_identity="${MNEMOS_MACOS_SIGN_IDENTITY:--}"
notarize="${MNEMOS_MACOS_NOTARIZE:-0}"

for binary in "$x86_binary" "$arm_binary"; do
  if [ ! -x "$binary" ]; then
    echo "Collector binary is missing or not executable: $binary"
    exit 1
  fi
done

for command_name in lipo codesign hdiutil iconutil sips rsvg-convert; do
  if ! command -v "$command_name" > /dev/null 2>&1; then
    echo "Required macOS packaging command is unavailable: $command_name"
    exit 1
  fi
done

if [ "$notarize" = "1" ]; then
  if [ "$signing_identity" = "-" ]; then
    echo "Notarized macOS packages require a Developer ID Application signing identity."
    exit 1
  fi

  for variable_name in APPLE_ID APPLE_TEAM_ID APPLE_APP_SPECIFIC_PASSWORD; do
    if [ -z "${!variable_name:-}" ]; then
      echo "$variable_name is required for macOS notarization."
      exit 1
    fi
  done
fi

work_directory="$(mktemp -d)"
trap 'rm -rf "$work_directory"' EXIT

sign_binary() {
  target="$1"

  if [ "$signing_identity" = "-" ]; then
    codesign --force --sign - "$target"
  else
    codesign \
      --force \
      --options runtime \
      --timestamp \
      --sign "$signing_identity" \
      "$target"
  fi
}

notarize_file() {
  target="$1"

  xcrun notarytool submit \
    "$target" \
    --apple-id "$APPLE_ID" \
    --team-id "$APPLE_TEAM_ID" \
    --password "$APPLE_APP_SPECIFIC_PASSWORD" \
    --wait
}

update_binary="$dist_directory/mnemos-collector-update-macos-universal"
lipo -create \
  "$x86_binary" \
  "$arm_binary" \
  -output "$update_binary"
chmod 0755 "$update_binary"

architectures="$(lipo -archs "$update_binary")"

if [[ "$architectures" != *"x86_64"* || "$architectures" != *"arm64"* ]]; then
  echo "Universal Collector binary must contain x86_64 and arm64: $architectures"
  exit 1
fi

sign_binary "$update_binary"

app="$work_directory/Mnemos Collector.app"
contents="$app/Contents"
mkdir -p "$contents/MacOS" "$contents/Resources"
install -m 0755 "$update_binary" "$contents/MacOS/mnemos-collector"

cat > "$contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key>
  <string>en</string>
  <key>CFBundleDisplayName</key>
  <string>Mnemos Collector</string>
  <key>CFBundleExecutable</key>
  <string>mnemos-collector</string>
  <key>CFBundleIconFile</key>
  <string>MnemosCollector</string>
  <key>CFBundleIdentifier</key>
  <string>rest.knalis.mnemos-collector</string>
  <key>CFBundleInfoDictionaryVersion</key>
  <string>6.0</string>
  <key>CFBundleName</key>
  <string>Mnemos Collector</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>$version</string>
  <key>CFBundleVersion</key>
  <string>$version</string>
  <key>LSMinimumSystemVersion</key>
  <string>12.0</string>
  <key>NSHighResolutionCapable</key>
  <true/>
</dict>
</plist>
PLIST

icon_source="$work_directory/icon-1024.png"
rsvg-convert \
  --format png \
  --width 1024 \
  --height 1024 \
  "$repo_root/assets/mnemos-mascot-cat.svg" \
  > "$icon_source"

iconset="$work_directory/MnemosCollector.iconset"
mkdir -p "$iconset"

create_icon() {
  size="$1"
  scale="$2"
  pixels=$((size * scale))
  suffix=""

  if [ "$scale" -eq 2 ]; then
    suffix="@2x"
  fi

  sips \
    -z "$pixels" "$pixels" \
    "$icon_source" \
    --out "$iconset/icon_${size}x${size}${suffix}.png" \
    > /dev/null
}

for size in 16 32 128 256 512; do
  create_icon "$size" 1
  create_icon "$size" 2
done

iconutil \
  -c icns \
  "$iconset" \
  -o "$contents/Resources/MnemosCollector.icns"

if [ "$signing_identity" = "-" ]; then
  codesign --force --deep --sign - "$app"
else
  codesign \
    --force \
    --deep \
    --options runtime \
    --timestamp \
    --sign "$signing_identity" \
    "$app"
fi

codesign --verify --deep --strict "$app"

if [ "$notarize" = "1" ]; then
  app_archive="$work_directory/Mnemos-Collector-app.zip"
  ditto -c -k --keepParent "$app" "$app_archive"
  notarize_file "$app_archive"
  xcrun stapler staple "$app"
  xcrun stapler validate "$app"
  spctl --assess --type execute --verbose=2 "$app"
fi

staging="$work_directory/dmg"
mkdir -p "$staging"
cp -R "$app" "$staging/Mnemos Collector.app"
ln -s /Applications "$staging/Applications"

dmg="$dist_directory/Mnemos-Collector-macOS-universal.dmg"
hdiutil create \
  -volname "Mnemos Collector" \
  -srcfolder "$staging" \
  -format UDZO \
  -ov \
  "$dmg" \
  > /dev/null

if [ "$signing_identity" != "-" ]; then
  codesign \
    --force \
    --timestamp \
    --sign "$signing_identity" \
    "$dmg"
fi

if [ "$notarize" = "1" ]; then
  notarize_file "$dmg"
  xcrun stapler staple "$dmg"
  xcrun stapler validate "$dmg"
fi

for artifact in "$dmg" "$update_binary"; do
  if [ ! -s "$artifact" ]; then
    echo "macOS distribution artifact was not produced: $artifact"
    exit 1
  fi
done

printf 'Created macOS distribution artifacts:\n  %s\n  %s\n' \
  "$dmg" \
  "$update_binary"
