#!/usr/bin/env bash
set -euo pipefail

if ! command -v zig >/dev/null 2>&1; then
  echo "error: zig is required on PATH" >&2
  exit 1
fi

if [ -z "${ARBOR_GHOSTTY_SRC:-}" ]; then
  echo "error: ARBOR_GHOSTTY_SRC must point at a Ghostty source checkout" >&2
  exit 1
fi

if [ ! -d "${ARBOR_GHOSTTY_SRC}" ]; then
  echo "error: ARBOR_GHOSTTY_SRC does not exist: ${ARBOR_GHOSTTY_SRC}" >&2
  exit 1
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
OUT_DIR="${ARBOR_GHOSTTY_BRIDGE_OUT_DIR:-${REPO_ROOT}/target/ghostty-vt-bridge}"
LIB_DIR="${OUT_DIR}/lib"
BUILD_DIR="$(mktemp -d "${TMPDIR:-/tmp}/arbor-ghostty-vt-XXXXXX")"
STAGED_GHOSTTY_DIR="${BUILD_DIR}/ghostty"
trap 'rm -rf "${BUILD_DIR}"' EXIT

mkdir -p "${STAGED_GHOSTTY_DIR}" "${LIB_DIR}"
rm -f "${LIB_DIR}/libarbor_ghostty_vt_bridge".*

if command -v rsync >/dev/null 2>&1; then
  rsync -a --delete --exclude '.git' "${ARBOR_GHOSTTY_SRC}/" "${STAGED_GHOSTTY_DIR}/"
else
  cp -R "${ARBOR_GHOSTTY_SRC}/." "${STAGED_GHOSTTY_DIR}/"
  rm -rf "${STAGED_GHOSTTY_DIR}/.git"
fi

cp "${REPO_ROOT}/scripts/ghostty-vt/arbor_build.zig" "${STAGED_GHOSTTY_DIR}/arbor_build.zig"
cp "${REPO_ROOT}/scripts/ghostty-vt/arbor_bridge.zig" "${STAGED_GHOSTTY_DIR}/arbor_bridge.zig"

(
  cd "${STAGED_GHOSTTY_DIR}"
  zig build --build-file arbor_build.zig -Doptimize=ReleaseFast
)

cp "${STAGED_GHOSTTY_DIR}/zig-out/lib/"libarbor_ghostty_vt_bridge.* "${LIB_DIR}/"

echo "built Ghostty VT bridge in ${LIB_DIR}"
