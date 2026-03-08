#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 4 ]]; then
  echo "usage: $0 <tag> <target-triple> <binary-path> <output-dir>" >&2
  exit 1
fi

TAG="$1"
TARGET_TRIPLE="$2"
BINARY_PATH="$3"
OUTPUT_DIR="$4"

ROOT_DIR="$(cd "$(dirname "$0")/../.." && pwd)"
APP_NAME="Arbor"
APP_DIR="${OUTPUT_DIR}/${APP_NAME}.app"
CONTENTS_DIR="${APP_DIR}/Contents"
MACOS_DIR="${CONTENTS_DIR}/MacOS"
RESOURCES_DIR="${CONTENTS_DIR}/Resources"
ICONSET_DIR="${OUTPUT_DIR}/AppIcon.iconset"

mkdir -p "${MACOS_DIR}" "${RESOURCES_DIR}" "${ICONSET_DIR}"
install -m 0755 "${BINARY_PATH}" "${MACOS_DIR}/${APP_NAME}"

# Bundle arbor-httpd alongside the main binary so the GUI can auto-start it.
HTTPD_PATH="$(dirname "${BINARY_PATH}")/arbor-httpd"
if [[ -f "${HTTPD_PATH}" ]]; then
  install -m 0755 "${HTTPD_PATH}" "${MACOS_DIR}/arbor-httpd"
  echo "bundled arbor-httpd from ${HTTPD_PATH}"
else
  echo "note: arbor-httpd not found at ${HTTPD_PATH}, skipping bundle"
fi

# Bundle web UI assets for arbor-httpd.
WEB_UI_DIST="${ROOT_DIR}/crates/arbor-web-ui/app/dist"
if [[ -d "${WEB_UI_DIST}" ]]; then
  cp -R "${WEB_UI_DIST}" "${RESOURCES_DIR}/web-ui"
  echo "bundled web-ui assets from ${WEB_UI_DIST}"
else
  echo "warning: web-ui dist not found at ${WEB_UI_DIST}, skipping bundle"
fi

cp "${ROOT_DIR}/packaging/macos/Info.plist" "${CONTENTS_DIR}/Info.plist"
printf 'APPL????' > "${CONTENTS_DIR}/PkgInfo"

/usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString ${TAG}" "${CONTENTS_DIR}/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleVersion ${TAG}" "${CONTENTS_DIR}/Info.plist"

ICON_SOURCE="${ROOT_DIR}/assets/icons/arbor-icon-1024.png"
for size in 16 32 128 256 512; do
  sips -z "${size}" "${size}" "${ICON_SOURCE}" --out "${ICONSET_DIR}/icon_${size}x${size}.png" >/dev/null
  double_size=$((size * 2))
  sips -z "${double_size}" "${double_size}" "${ICON_SOURCE}" --out "${ICONSET_DIR}/icon_${size}x${size}@2x.png" >/dev/null
done

iconutil -c icns "${ICONSET_DIR}" -o "${RESOURCES_DIR}/AppIcon.icns"
rm -rf "${ICONSET_DIR}"

ARCHIVE_NAME="${APP_NAME}-${TAG}-${TARGET_TRIPLE}.app.zip"
/usr/bin/ditto -c -k --keepParent --norsrc "${APP_DIR}" "${OUTPUT_DIR}/${ARCHIVE_NAME}"

echo "${OUTPUT_DIR}/${ARCHIVE_NAME}"
