#!/usr/bin/env bash
# Authenticate with a team App Store Connect API key, never an Apple ID password.
set +x
set -euo pipefail
umask 077

: "${APPLE_API_PRIVATE_KEY:?required}"
: "${APPLE_API_KEY_ID:?required}"
: "${APPLE_API_ISSUER_ID:?required}"
ARCHIVE="${1:?usage: notarize-macos.sh ARCHIVE}"

TMP_DIR="$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/arbor-notary.XXXXXX")"
trap 'rm -rf "$TMP_DIR"' EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
printf '%s\n' "$APPLE_API_PRIVATE_KEY" > "$TMP_DIR/AuthKey.p8"
unset APPLE_API_PRIVATE_KEY
AUTH=(--key "$TMP_DIR/AuthKey.p8" --key-id "$APPLE_API_KEY_ID" --issuer "$APPLE_API_ISSUER_ID")

SUBMIT_STATUS=0
xcrun notarytool submit "$ARCHIVE" "${AUTH[@]}" --wait --output-format json \
  > "$TMP_DIR/result.json" || SUBMIT_STATUS=$?
cat "$TMP_DIR/result.json"
if [[ "$SUBMIT_STATUS" -ne 0 ]] || ! jq -e '.status == "Accepted"' "$TMP_DIR/result.json" > /dev/null; then
  echo "Notarization did not succeed; refusing to staple." >&2
  SUBMISSION_ID="$(jq -r '.id // empty' "$TMP_DIR/result.json" 2>/dev/null || true)"
  if [[ -n "$SUBMISSION_ID" ]]; then
    xcrun notarytool log "$SUBMISSION_ID" "${AUTH[@]}" || true
  fi
  exit 1
fi
