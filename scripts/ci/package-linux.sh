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

mkdir -p "${STAGING_DIR}"
install -m 0755 "${BINARY_PATH}" "${STAGING_DIR}/${APP_NAME}"
cp README.md "${STAGING_DIR}/README.md"

tar -C "${OUTPUT_DIR}" -czf "${ARCHIVE_PATH}" "$(basename "${STAGING_DIR}")"

echo "${ARCHIVE_PATH}"
