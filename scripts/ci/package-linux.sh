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

APP_NAME="Arbor"
STAGING_DIR="${OUTPUT_DIR}/${APP_NAME}-${TAG}-${TARGET_TRIPLE}"
ARCHIVE_PATH="${OUTPUT_DIR}/${APP_NAME}-${TAG}-${TARGET_TRIPLE}.tar.gz"

ROOT_DIR="$(cd "$(dirname "$0")/../.." && pwd)"

mkdir -p "${STAGING_DIR}/bin" "${STAGING_DIR}/share/arbor"
install -m 0755 "${BINARY_PATH}" "${STAGING_DIR}/bin/${APP_NAME}"
cp README.md "${STAGING_DIR}/README.md"

# Bundle arbor-httpd alongside the main binary.
HTTPD_PATH="$(dirname "${BINARY_PATH}")/arbor-httpd"
if [[ -f "${HTTPD_PATH}" ]]; then
  install -m 0755 "${HTTPD_PATH}" "${STAGING_DIR}/bin/arbor-httpd"
  echo "bundled arbor-httpd from ${HTTPD_PATH}"
else
  echo "note: arbor-httpd not found at ${HTTPD_PATH}, skipping bundle"
fi

# Bundle web UI assets for arbor-httpd.
WEB_UI_DIST="${ROOT_DIR}/crates/arbor-web-ui/app/dist"
if [[ -d "${WEB_UI_DIST}" ]]; then
  cp -R "${WEB_UI_DIST}" "${STAGING_DIR}/share/arbor/web-ui"
  echo "bundled web-ui assets from ${WEB_UI_DIST}"
else
  echo "warning: web-ui dist not found at ${WEB_UI_DIST}, skipping bundle"
fi

tar -C "${OUTPUT_DIR}" -czf "${ARCHIVE_PATH}" "$(basename "${STAGING_DIR}")"

echo "${ARCHIVE_PATH}"
