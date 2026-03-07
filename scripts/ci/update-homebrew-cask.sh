#!/usr/bin/env bash
set -euo pipefail

# Updates the Homebrew cask in the tap repository.
# Usage: update-homebrew-cask.sh <tag> <sha256>

if [[ $# -ne 2 ]]; then
  echo "usage: $0 <tag> <sha256>" >&2
  exit 1
fi

TAG="$1"
SHA256="$2"

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
CASK_TEMPLATE="${SCRIPT_DIR}/../../homebrew/Casks/arbor.rb"

if [[ ! -f "${CASK_TEMPLATE}" ]]; then
  echo "error: cask template not found at ${CASK_TEMPLATE}" >&2
  exit 1
fi

sed -e "s/version \"PLACEHOLDER\"/version \"${TAG}\"/" \
    -e "s/sha256 \"PLACEHOLDER\"/sha256 \"${SHA256}\"/" \
    "${CASK_TEMPLATE}"
