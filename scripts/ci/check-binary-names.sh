#!/usr/bin/env bash
# Detect case-insensitive filename collisions.
#
# Usage:
#   check-binary-names.sh <directory>    — check files in a directory
#   check-binary-names.sh --cargo        — check [[bin]] names across workspace Cargo.toml files
#
# Exits 0 if no collisions, 1 if duplicates are found.
set -euo pipefail

check_directory() {
  local dir="$1"
  if [[ ! -d "$dir" ]]; then
    echo "error: directory not found: $dir" >&2
    exit 1
  fi

  local -A seen
  local collisions=0

  for path in "$dir"/*; do
    [[ -f "$path" ]] || continue
    local name
    name="$(basename "$path")"
    local lower
    lower="$(echo "$name" | tr '[:upper:]' '[:lower:]')"

    if [[ -n "${seen[$lower]:-}" ]]; then
      echo "error: case-insensitive collision: '$name' vs '${seen[$lower]}'" >&2
      collisions=$((collisions + 1))
    else
      seen[$lower]="$name"
    fi
  done

  if [[ $collisions -gt 0 ]]; then
    echo "error: found $collisions case-insensitive collision(s) in $dir" >&2
    exit 1
  fi
  echo "ok: no case-insensitive collisions in $dir"
}

check_cargo() {
  local root_dir
  root_dir="$(cd "$(dirname "$0")/../.." && pwd)"

  local -A seen
  local collisions=0

  while IFS= read -r toml; do
    while IFS= read -r name; do
      [[ -n "$name" ]] || continue
      local lower
      lower="$(echo "$name" | tr '[:upper:]' '[:lower:]')"

      if [[ -n "${seen[$lower]:-}" ]]; then
        echo "error: case-insensitive binary name collision: '$name' (in $toml) vs '${seen[$lower]}'" >&2
        collisions=$((collisions + 1))
      else
        seen[$lower]="$name ($toml)"
      fi
    done < <(awk '/^\[\[bin\]\]/{b=1;next} /^\[/{b=0} b && /^name[ \t]*=/{gsub(/.*= *"/,""); gsub(/".*/,""); print}' "$toml")
  done < <(find "$root_dir/crates" -name Cargo.toml)

  if [[ $collisions -gt 0 ]]; then
    echo "error: found $collisions case-insensitive binary name collision(s)" >&2
    exit 1
  fi
  echo "ok: no case-insensitive binary name collisions in workspace"
}

if [[ $# -lt 1 ]]; then
  echo "usage: $0 <directory> | --cargo" >&2
  exit 1
fi

case "$1" in
  --cargo) check_cargo ;;
  *)       check_directory "$1" ;;
esac
