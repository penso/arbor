#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
TEST_DIR="$(mktemp -d)"
trap 'rm -rf "$TEST_DIR"' EXIT
export RUNNER_TEMP="$TEST_DIR"
export APPLE_API_PRIVATE_KEY="test-private-key"
export APPLE_API_KEY_ID="TESTKEY"
export APPLE_API_ISSUER_ID="test-issuer"

xcrun() {
  [[ "$1" == notarytool ]] || return 90
  local operation="$2"
  shift 2
  if [[ "$operation" == submit ]]; then
    [[ "$1" == 'app with spaces.zip' ]] || return 91
  else
    [[ "$operation" == log && "$1" == test-submission ]] || return 92
    touch "$RUNNER_TEMP/log-called"
  fi
  shift
  [[ "$1" == --key && -f "$2" ]] || return 93
  [[ "$(< "$2")" == test-private-key ]] || return 94
  [[ -z "${APPLE_API_PRIVATE_KEY:-}" ]] || return 95
  [[ "$(find "$2" -perm 0600 -print)" == "$2" ]] || return 96
  shift 2
  [[ "$1" == --key-id && "$2" == TESTKEY && "$3" == --issuer && "$4" == test-issuer ]] || return 97
  shift 4
  if [[ "$operation" == log ]]; then
    [[ "$#" -eq 0 ]] || return 98
    return "${LOG_EXIT:-0}"
  fi
  [[ "$*" == '--wait --output-format json' ]] || return 99
  touch "$RUNNER_TEMP/submit-called"
  printf '%s\n' "$RESPONSE"
  return "$SUBMIT_EXIT"
}
export -f xcrun

run_case() {
  local name="$1" expected="$2" expect_log="$3" result=0
  export RESPONSE="$4" SUBMIT_EXIT="$5" LOG_EXIT="${6:-0}"
  rm -f "$TEST_DIR/log-called" "$TEST_DIR/submit-called"
  bash "$SCRIPT_DIR/notarize-macos.sh" 'app with spaces.zip' > "$TEST_DIR/output" 2>&1 || result=$?
  [[ "$result" -eq "$expected" ]] || { cat "$TEST_DIR/output"; exit 1; }
  [[ -f "$TEST_DIR/submit-called" ]]
  if [[ "$expect_log" == yes ]]; then
    [[ -f "$TEST_DIR/log-called" ]]
  else
    [[ ! -f "$TEST_DIR/log-called" ]]
  fi
  [[ -z "$(find "$TEST_DIR" -name 'arbor-notary.*' -print)" ]]
  if grep -q 'test-private-key' "$TEST_DIR/output"; then
    exit 1
  fi
  printf 'PASS: %s\n' "$name"
}

run_case accepted 0 no '{"id":"test-submission","status":"Accepted"}' 0
run_case rejected 1 yes '{"id":"test-submission","status":"Invalid"}' 0
run_case pending 1 yes '{"id":"test-submission","status":"In Progress"}' 0
run_case command-failure 1 yes '{"id":"test-submission","status":"Accepted"}' 1
run_case log-failure 1 yes '{"id":"test-submission","status":"Invalid"}' 1 1
run_case malformed 1 no 'not JSON' 0
run_case empty 1 no '' 1
run_case missing-status 1 no '{}' 0
unset APPLE_API_PRIVATE_KEY
if bash "$SCRIPT_DIR/notarize-macos.sh" 'app with spaces.zip' > "$TEST_DIR/output" 2>&1; then
  exit 1
fi
printf 'PASS: missing credentials\n'
