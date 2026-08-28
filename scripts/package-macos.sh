#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -ne 4 ]; then
  echo "Usage: package-macos.sh <collector-binary> <version> <architecture> <output-directory>" >&2
  exit 1
fi

binary="$1"
version="$2"
architecture="$3"
output_directory="$4"

case "$architecture" in
  aarch64|x86_64)
    ;;
  *)
    echo "Unsupported macOS architecture: $architecture" >&2
    exit 1
    ;;
esac

if [ ! -x "$binary" ]; then
  echo "Collector binary is missing or not executable: $binary" >&2
  exit 1
fi

for command_name in codesign hdiutil iconutil rsvg-convert; do
  if ! command -v "$command_name" >/dev/null 2>&1; then
    echo "Required macOS packaging command is unavailable: $command_name" >&2
    exit 1
  fi
done

app_name="Mnemos Collector"
bundle_identifier="rest.knalis.mnemos-collector"
bundle_directory="$output_directory/$app_name.app"
contents_directory="$bundle_directory/Contents"
macos_directory="$contents_directory/MacOS"
resources_directory="$contents_directory/Resources"
executable_name="mnemos-collector"
update_name="mnemos-collector-update-macos-$architecture"
dmg_name="Mnemos-Collector-macOS-$architecture.dmg"
work_directory="$(mktemp -d)"
iconset_directory="$work_directory/MnemosCollector.iconset"
dmg_directory="$work_directory/dmg"

cleanup() {
  rm -rf "$work_directory"
}
trap cleanup EXIT

mkdir -p \
  "$output_directory" \
  "$macos_directory" \
  "$resources_directory" \
  "$iconset_directory" \
  "$dmg_directory"

cp "$binary" "$macos_directory/$executable_name"
chmod 0755 "$macos_directory/$executable_name"

cat > "$contents_directory/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "https://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDevelopmentRegion</key>
    <string>en</string>
    <key>CFBundleDisplayName</key>
    <string>Mnemos Collector</string>
    <key>CFBundleExecutable</key>
    <string>$executable_name</string>
    <key>CFBundleIconFile</key>
    <string>MnemosCollector</string>
    <key>CFBundleIdentifier</key>
    <string>$bundle_identifier</string>
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
    <key>LSApplicationCategoryType</key>
    <string>public.app-category.utilities</string>
    <key>LSMinimumSystemVersion</key>
    <string>12.0</string>
    <key>NSHighResolutionCapable</key>
    <true/>
</dict>
</plist>
EOF

render_icon() {
  size="$1"
  output="$2"

  rsvg-convert \
    --width "$size" \
    --height "$size" \
    assets/mnemos-mascot-cat.svg \
    --output "$output"
}

render_icon 16 "$iconset_directory/icon_16x16.png"
render_icon 32 "$iconset_directory/icon_16x16@2x.png"
render_icon 32 "$iconset_directory/icon_32x32.png"
render_icon 64 "$iconset_directory/icon_32x32@2x.png"
render_icon 128 "$iconset_directory/icon_128x128.png"
render_icon 256 "$iconset_directory/icon_128x128@2x.png"
render_icon 256 "$iconset_directory/icon_256x256.png"
render_icon 512 "$iconset_directory/icon_256x256@2x.png"
render_icon 512 "$iconset_directory/icon_512x512.png"
render_icon 1024 "$iconset_directory/icon_512x512@2x.png"

iconutil \
  --convert icns \
  --output "$resources_directory/MnemosCollector.icns" \
  "$iconset_directory"

codesign \
  --force \
  --deep \
  --sign - \
  --identifier "$bundle_identifier" \
  "$bundle_directory"

codesign \
  --verify \
  --deep \
  --strict \
  "$bundle_directory"

cp "$binary" "$output_directory/$update_name"
chmod 0755 "$output_directory/$update_name"
codesign \
  --force \
  --sign - \
  --identifier "$bundle_identifier.updater.$architecture" \
  "$output_directory/$update_name"
codesign \
  --verify \
  --strict \
  "$output_directory/$update_name"

cp -R "$bundle_directory" "$dmg_directory/$app_name.app"
ln -s /Applications "$dmg_directory/Applications"

hdiutil create \
  -volname "$app_name" \
  -srcfolder "$dmg_directory" \
  -ov \
  -format UDZO \
  "$output_directory/$dmg_name" \
  >/dev/null

test -s "$output_directory/$dmg_name"
test -x "$output_directory/$update_name"
