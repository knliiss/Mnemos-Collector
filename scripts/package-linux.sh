#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -ne 4 ]; then
  echo "Usage: package-linux.sh <collector-binary> <version> <dist-directory> <linuxdeploy>"
  exit 2
fi

binary="$(realpath "$1")"
version="$2"
dist_directory="$(mkdir -p "$3" && realpath "$3")"
linuxdeploy="$(realpath "$4")"
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

if [ ! -x "$binary" ]; then
  echo "Collector binary is missing or not executable: $binary"
  exit 1
fi

if [ ! -f "$linuxdeploy" ]; then
  echo "linuxdeploy is missing: $linuxdeploy"
  exit 1
fi

work_directory="$(mktemp -d)"
trap 'rm -rf "$work_directory"' EXIT

app_dir="$work_directory/MnemosCollector.AppDir"
mkdir -p \
  "$app_dir/usr/bin" \
  "$app_dir/usr/share/applications" \
  "$app_dir/usr/share/icons/hicolor/scalable/apps"

install -m 0755 "$binary" "$app_dir/usr/bin/mnemos-collector"
install -m 0644 \
  "$repo_root/assets/mnemos-mascot-cat.svg" \
  "$app_dir/usr/share/icons/hicolor/scalable/apps/mnemos-collector.svg"

cat > "$app_dir/usr/share/applications/mnemos-collector.desktop" <<'DESKTOP'
[Desktop Entry]
Type=Application
Name=Mnemos Collector
Comment=Cristalix Master Sword telemetry collector
Exec=mnemos-collector
Icon=mnemos-collector
Terminal=false
Categories=Utility;
StartupNotify=true
DESKTOP

appimage="$dist_directory/Mnemos-Collector-Linux-x86_64.AppImage"

(
  cd "$work_directory"

  APPIMAGE_EXTRACT_AND_RUN=1 \
  LDAI_OUTPUT="$appimage" \
  LINUXDEPLOY_OUTPUT_VERSION="$version" \
    "$linuxdeploy" \
      --appimage-extract-and-run \
      --appdir "$app_dir" \
      --executable "$app_dir/usr/bin/mnemos-collector" \
      --desktop-file "$app_dir/usr/share/applications/mnemos-collector.desktop" \
      --icon-file "$app_dir/usr/share/icons/hicolor/scalable/apps/mnemos-collector.svg" \
      --output appimage
)

chmod 0755 "$appimage"

update_binary="$dist_directory/mnemos-collector-update-linux-x86_64"
install -m 0755 "$binary" "$update_binary"

deb_root="$work_directory/deb"
mkdir -p \
  "$deb_root/DEBIAN" \
  "$deb_root/usr/bin" \
  "$deb_root/usr/share/applications" \
  "$deb_root/usr/share/icons/hicolor/scalable/apps"

install -m 0755 "$binary" "$deb_root/usr/bin/mnemos-collector"
install -m 0644 \
  "$app_dir/usr/share/applications/mnemos-collector.desktop" \
  "$deb_root/usr/share/applications/mnemos-collector.desktop"
install -m 0644 \
  "$repo_root/assets/mnemos-mascot-cat.svg" \
  "$deb_root/usr/share/icons/hicolor/scalable/apps/mnemos-collector.svg"

dependencies="libc6"

if command -v dpkg-shlibdeps > /dev/null 2>&1; then
  mkdir -p "$work_directory/debian"
  cat > "$work_directory/debian/control" <<'CONTROL'
Source: mnemos-collector
Section: utils
Priority: optional
Maintainer: Mnemos <noreply@mnemos.invalid>
Standards-Version: 4.6.2

Package: mnemos-collector
Architecture: amd64
Description: Mnemos Collector
CONTROL

  detected_dependencies="$(
    (
      cd "$work_directory"
      dpkg-shlibdeps -O -e"$binary" 2>/dev/null || true
    ) | sed -n 's/^shlibs:Depends=//p'
  )"

  if [ -n "$detected_dependencies" ]; then
    dependencies="$detected_dependencies"
  fi
fi

cat > "$deb_root/DEBIAN/control" <<CONTROL
Package: mnemos-collector
Version: $version
Section: utils
Priority: optional
Architecture: amd64
Maintainer: Mnemos <noreply@mnemos.invalid>
Depends: $dependencies
Description: Mnemos Collector
 Cristalix Master Sword telemetry collector for Mnemos.
CONTROL

deb="$dist_directory/Mnemos-Collector-Linux-amd64.deb"
dpkg-deb --build --root-owner-group "$deb_root" "$deb" > /dev/null

for artifact in "$appimage" "$deb" "$update_binary"; do
  if [ ! -s "$artifact" ]; then
    echo "Linux distribution artifact was not produced: $artifact"
    exit 1
  fi
done

printf 'Created Linux distribution artifacts:\n  %s\n  %s\n  %s\n' \
  "$appimage" \
  "$deb" \
  "$update_binary"
