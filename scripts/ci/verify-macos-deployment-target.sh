#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 2 ]]; then
  echo "usage: $0 <max-macos-version> <binary> [<binary>...]" >&2
  exit 1
fi

MAX_MACOS_VERSION="$1"
shift

resolve_vtool() {
  if command -v vtool >/dev/null 2>&1; then
    command -v vtool
    return 0
  fi

  if command -v xcrun >/dev/null 2>&1; then
    xcrun --find vtool 2>/dev/null || true
    return 0
  fi

  return 1
}

extract_minos() {
  local bin="$1"
  local vtool_path="$2"
  local vtool_output

  if ! vtool_output="$("$vtool_path" -show-build "$bin" 2>/dev/null)"; then
    return 1
  fi

  printf '%s\n' "$vtool_output" | awk '
    $1 == "minos" { print $2; exit }
    $1 == "cmd" && $2 == "LC_VERSION_MIN_MACOSX" { in_legacy_macos_block = 1; next }
    in_legacy_macos_block && $1 == "version" { print $2; exit }
  '
}

compare_versions() {
  local actual="$1"
  local maximum="$2"

  python3 - "$actual" "$maximum" <<'PY'
import sys

def parse(version: str) -> tuple[int, ...]:
    return tuple(int(part) for part in version.split('.'))

sys.exit(0 if parse(sys.argv[1]) <= parse(sys.argv[2]) else 1)
PY
}

VTOOL_PATH="$(resolve_vtool)"

if [[ -z "${VTOOL_PATH:-}" ]]; then
  echo "::error::Could not find vtool on PATH or via xcrun"
  exit 1
fi

for bin in "$@"; do
  if [[ ! -f "$bin" ]]; then
    echo "note: $bin not found, skipping deployment target check"
    continue
  fi

  if ! minos="$(extract_minos "$bin" "$VTOOL_PATH")"; then
    echo "::error::Failed to inspect build metadata for $bin with $VTOOL_PATH"
    exit 1
  fi

  echo "$bin: minos=$minos"

  if [[ -z "$minos" ]]; then
    echo "::error::Could not determine minos for $bin"
    exit 1
  fi

  if ! compare_versions "$minos" "$MAX_MACOS_VERSION"; then
    echo "::error::$bin requires macOS $minos, expected <= $MAX_MACOS_VERSION"
    exit 1
  fi
done
