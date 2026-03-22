#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
STATE_DIR="${ARBOR_PERF_STATE_DIR:-${TMPDIR:-/tmp}/arbor-perf-harness}"
STATE_FILE="${STATE_DIR}/state.env"
NIGHTLY_TOOLCHAIN="${ARBOR_PERF_TOOLCHAIN:-nightly-2025-11-30}"
RATATUI_TOOLCHAIN="${ARBOR_PERF_RATATUI_TOOLCHAIN:-stable}"
AUTH_TOKEN="arbor-perf-token"

usage() {
  cat <<'EOF'
Usage:
  scripts/dev/arbor-perf-harness.sh build
  scripts/dev/arbor-perf-harness.sh start [dashboard|hidden|redraw|scroll]
  scripts/dev/arbor-perf-harness.sh daemon
  scripts/dev/arbor-perf-harness.sh seed
  scripts/dev/arbor-perf-harness.sh gui
  scripts/dev/arbor-perf-harness.sh status
  scripts/dev/arbor-perf-harness.sh sample
  scripts/dev/arbor-perf-harness.sh stop

Modes:
  dashboard  Runs the upstream ratatui demo dashboard as a realistic TUI workload.
  hidden  Rewrites a single visible line with carriage returns to mimic "thinking".
  redraw  Repaints a full alternate-screen terminal frame to mimic resume/session-list redraws.
  scroll  Continuously prints visible lines to stress scrolling throughput.
EOF
}

find_free_port() {
  python3 - <<'PY'
import socket
s = socket.socket()
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()
PY
}

shell_quote() {
  printf '%q' "$1"
}

ensure_built() {
  (
    cd "$ROOT_DIR"
    cargo +"$NIGHTLY_TOOLCHAIN" build -p arbor-httpd --features agent-chat
    cargo +"$NIGHTLY_TOOLCHAIN" build -p arbor-gui --features agent-chat
    cargo +"$NIGHTLY_TOOLCHAIN" build -p arbor-cli
  )
}

ensure_ratatui_dashboard_built() {
  RATATUI_REPO_DIR="${STATE_DIR}/cache/ratatui"
  RATATUI_DASHBOARD_BIN="${RATATUI_REPO_DIR}/target/release/demo"

  mkdir -p "${STATE_DIR}/cache"

  if [[ ! -d "${RATATUI_REPO_DIR}/.git" ]]; then
    rm -rf "$RATATUI_REPO_DIR"
    git clone --depth=1 https://github.com/ratatui/ratatui.git "$RATATUI_REPO_DIR"
  fi

  (
    cd "$RATATUI_REPO_DIR"
    cargo +"$RATATUI_TOOLCHAIN" build --release --manifest-path examples/apps/demo/Cargo.toml
  )
}

load_state() {
  if [[ ! -f "$STATE_FILE" ]]; then
    echo "harness state not found: $STATE_FILE" >&2
    exit 1
  fi
  # shellcheck disable=SC1090
  source "$STATE_FILE"
}

wait_for_daemon() {
  local daemon_url="$1"
  local home_dir="$2"
  local cli_bin="$ROOT_DIR/target/debug/arbor-cli"

  for _ in $(seq 1 60); do
    if HOME="$home_dir" ARBOR_DAEMON_URL="$daemon_url" ARBOR_DAEMON_AUTH_TOKEN="$AUTH_TOKEN" \
      "$cli_bin" health >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.2
  done

  return 1
}

write_workload_script() {
  local mode="$1"
  local script_path="$2"

  case "$mode" in
    hidden)
      cat >"$script_path" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
i=0
while true; do
  printf '\rthinking %08d ' "$i"
  i=$((i + 1))
  sleep 0.02
done
EOF
      ;;
    redraw)
      cat >"$script_path" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

cleanup() {
  printf '\033[0m\033[?25h\033[?1049l'
}
trap cleanup EXIT INT TERM

printf '\033[?1049h\033[2J\033[?25l'

frame=0
while true; do
  frame=$((frame + 1))
  mode=$((frame % 3))
  printf '\033[H'
  printf '+----------------------------------------------------------------------------------------------------------------------+\n'
  printf '| codex resume test harness                                                          frame %08d |\n' "$frame"
  printf '+----------------------------------------------------------------------------------------------------------------------+\n'
  if [[ "$mode" -eq 0 ]]; then
    printf '| Recent sessions                                                                                                      |\n'
    printf '+----------------------------------------------------------------------------------------------------------------------+\n'
    row=1
    while [[ "$row" -le 28 ]]; do
      state_index=$(((frame + row) % 4))
      case "$state_index" in
        0) state='running ' ;;
        1) state='waiting ' ;;
        2) state='done    ' ;;
        *) state='failed  ' ;;
      esac
      printf '| [%02d] %-8s session-%03d  model=gpt-5.4  cwd=/Users/penso/code/arbor  tokens=%06d  eta=%02ds                  |\n' \
        "$row" "$state" "$row" $(((frame * 37 + row * 13) % 900000)) $(((frame + row) % 59))
      row=$((row + 1))
    done
  elif [[ "$mode" -eq 1 ]]; then
    printf '| Resume details                                                                                                       |\n'
    printf '+----------------------------------------------------------------------------------------------------------------------+\n'
    printf '| restoring transcript...                                                                                              |\n'
    printf '| loading checkpoints...                                                                                               |\n'
    printf '| reconnecting tools...                                                                                                |\n'
    printf '| rebuilding context window...                                                                                         |\n'
    printf '|                                                                                                                      |\n'
    row=1
    while [[ "$row" -le 23 ]]; do
      width=$(((frame + row * 7) % 92 + 18))
      bar="$(printf '%*s' "$width" '' | tr ' ' '=')"
      printf '| task-%02d  %-96s |\n' "$row" "$bar"
      row=$((row + 1))
    done
  else
    printf '| Agent activity                                                                                                       |\n'
    printf '+----------------------------------------------------------------------------------------------------------------------+\n'
    row=1
    while [[ "$row" -le 28 ]]; do
      spark=''
      col=1
      while [[ "$col" -le 28 ]]; do
        value=$(((frame + row + col) % 8))
        case "$value" in
          0) spark="${spark}." ;;
          1) spark="${spark}:" ;;
          2) spark="${spark}-" ;;
          3) spark="${spark}=" ;;
          4) spark="${spark}+" ;;
          5) spark="${spark}*" ;;
          6) spark="${spark}#" ;;
          *) spark="${spark}@" ;;
        esac
        col=$((col + 1))
      done
      printf '| pane-%02d  %-28s  mem=%05dMiB  cpu=%03d%%  state=rendering  branch=perf/redraw                     |\n' \
        "$row" "$spark" $(((frame * 11 + row * 17) % 64000)) $(((frame * 3 + row * 5) % 100))
      row=$((row + 1))
    done
  fi
  printf '+----------------------------------------------------------------------------------------------------------------------+\n'
  printf '| q to quit harness                                                                                                    |\n'
  printf '+----------------------------------------------------------------------------------------------------------------------+\n'
  sleep 0.04
done
EOF
      ;;
    dashboard)
      if [[ -z "${RATATUI_DASHBOARD_BIN:-}" ]]; then
        echo "ratatui dashboard binary path is not initialized" >&2
        exit 1
      fi
      cat >"$script_path" <<EOF
#!/usr/bin/env bash
set -euo pipefail
exec $(shell_quote "$RATATUI_DASHBOARD_BIN") --tick-rate 33
EOF
      ;;
    scroll)
      cat >"$script_path" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
i=0
while true; do
  printf 'scroll %08d %s\n' "$i" "abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ"
  i=$((i + 1))
  sleep 0.005
done
EOF
      ;;
    *)
      echo "unknown mode: $mode" >&2
      exit 2
      ;;
  esac

  chmod +x "$script_path"
}

write_state_file() {
  mkdir -p "$STATE_DIR"
  cat >"$STATE_FILE" <<EOF
RUN_DIR=$(shell_quote "$RUN_DIR")
HOME_DIR=$(shell_quote "$HOME_DIR")
LOG_DIR=$(shell_quote "$LOG_DIR")
WORKLOAD_SCRIPT=$(shell_quote "$WORKLOAD_SCRIPT")
MODE=$(shell_quote "$MODE")
PORT=$(shell_quote "$PORT")
DAEMON_URL=$(shell_quote "$DAEMON_URL")
SESSION_ID=$(shell_quote "$SESSION_ID")
AUTH_TOKEN=$(shell_quote "$AUTH_TOKEN")
APP_ID=$(shell_quote "$APP_ID")
RATATUI_REPO_DIR=$(shell_quote "${RATATUI_REPO_DIR:-}")
RATATUI_DASHBOARD_BIN=$(shell_quote "${RATATUI_DASHBOARD_BIN:-}")
EOF
}

write_home_config() {
  mkdir -p "$HOME_DIR/.config/arbor"
  cat >"$HOME_DIR/.config/arbor/config.toml" <<EOF
daemon_url = "$DAEMON_URL"

[daemon]
auth_token = "$AUTH_TOKEN"
bind = "localhost"
EOF

  cat >"$HOME_DIR/.config/arbor/daemon_auth_tokens.json" <<EOF
{
  "$DAEMON_URL": "$AUTH_TOKEN"
}
EOF
}

start_harness() {
  local mode="${1:-dashboard}"

  ensure_built
  mkdir -p "$STATE_DIR"
  if [[ -f "$STATE_FILE" ]]; then
    echo "existing harness state found, stopping it first" >&2
    stop_harness || true
  fi

  RUN_DIR="$(mktemp -d "${STATE_DIR}/run.XXXXXX")"
  HOME_DIR="${RUN_DIR}/home"
  LOG_DIR="${RUN_DIR}/logs"
  WORKLOAD_SCRIPT="${RUN_DIR}/${mode}.sh"
  MODE="$mode"
  PORT="$(find_free_port)"
  DAEMON_URL="http://127.0.0.1:${PORT}"
  SESSION_ID="perf-${MODE}-$(date +%s)"
  APP_ID="so.pen.arbor.perf.${SESSION_ID}"

  mkdir -p "$HOME_DIR" "$LOG_DIR"
  RATATUI_REPO_DIR=""
  RATATUI_DASHBOARD_BIN=""
  if [[ "$MODE" == "dashboard" ]]; then
    ensure_ratatui_dashboard_built
  fi
  write_home_config
  write_workload_script "$MODE" "$WORKLOAD_SCRIPT"

  write_state_file

  echo "Arbor perf harness started"
  echo "mode:       $MODE"
  echo "run dir:    $RUN_DIR"
  echo "daemon url: $DAEMON_URL"
  echo "session id: $SESSION_ID"
  echo "app id:     $APP_ID"
  echo
  echo "Next steps:"
  echo "  just perf-harness daemon"
  echo "  just perf-harness seed"
  echo "  just perf-harness gui"
  echo "  just perf-harness status"
  echo "  just perf-harness sample"
  echo "  just perf-harness stop"
}

status_harness() {
  load_state
  local daemon_pid
  local gui_pid
  daemon_pid="$(resolve_daemon_pid)"
  gui_pid="$(resolve_gui_pid)"

  echo "run dir:    $RUN_DIR"
  echo "mode:       $MODE"
  echo "daemon url: $DAEMON_URL"
  echo "session id: $SESSION_ID"
  echo "app id:     $APP_ID"
  echo

  if [[ -n "$daemon_pid" ]]; then
    ps -p "$daemon_pid" -o pid=,%cpu=,%mem=,etime=,command=
  else
    echo "daemon: not running"
  fi
  if [[ -n "$gui_pid" ]]; then
    ps -p "$gui_pid" -o pid=,%cpu=,%mem=,etime=,command=
  else
    echo "gui: not running"
  fi
  echo
  if [[ -n "$daemon_pid" ]]; then
    HOME="$HOME_DIR" ARBOR_DAEMON_URL="$DAEMON_URL" \
      ARBOR_DAEMON_AUTH_TOKEN="$AUTH_TOKEN" \
      "$ROOT_DIR/target/debug/arbor-cli" --json terminals read "$SESSION_ID" --max-lines 8
  else
    echo "terminal snapshot unavailable: daemon is not running"
  fi
}

sample_harness() {
  load_state
  local gui_pid
  gui_pid="$(resolve_gui_pid)"

  if [[ -z "$gui_pid" ]]; then
    echo "gui is not running for app id $APP_ID" >&2
    exit 1
  fi

  echo "cpu samples for gui pid $gui_pid"
  for _ in $(seq 1 5); do
    date '+%H:%M:%S'
    ps -p "$gui_pid" -o pid=,%cpu=,%mem=,etime=,command=
    sleep 1
  done

  local sample_file="$RUN_DIR/gui-sample.txt"
  sample "$gui_pid" 3 -file "$sample_file" >/dev/null 2>&1
  echo
  echo "sample file: $sample_file"
  rg -n "Window::draw|draw_roots|compute_layout|shape_line" "$sample_file" || true
}

stop_harness() {
  load_state
  local daemon_pid
  local gui_pid
  daemon_pid="$(resolve_daemon_pid)"
  gui_pid="$(resolve_gui_pid)"

  if [[ -n "$daemon_pid" ]] && ps -p "$daemon_pid" >/dev/null 2>&1; then
    kill "$daemon_pid" >/dev/null 2>&1 || true
  fi
  if [[ -n "$gui_pid" ]] && ps -p "$gui_pid" >/dev/null 2>&1; then
    kill "$gui_pid" >/dev/null 2>&1 || true
  fi

  rm -f "$STATE_FILE"
  echo "stopped harness from $RUN_DIR"
}

resolve_daemon_pid() {
  pgrep -f "${APP_ID}.httpd" | tail -n 1 || true
}

resolve_gui_pid() {
  pgrep -f "${APP_ID}.gui" | tail -n 1 || true
}

run_daemon_foreground() {
  load_state
  cd "$ROOT_DIR"
  HOME="$HOME_DIR" \
  ARBOR_HTTPD_PORT="$PORT" \
  ARBOR_DAEMON_URL="$DAEMON_URL" \
  exec -a "${APP_ID}.httpd" "$ROOT_DIR/target/debug/arbor-httpd"
}

seed_workload_terminal() {
  load_state

  if ! wait_for_daemon "$DAEMON_URL" "$HOME_DIR"; then
    echo "daemon is not responding at $DAEMON_URL" >&2
    exit 1
  fi

  local quoted_workload
  quoted_workload="$(shell_quote "$WORKLOAD_SCRIPT")"
  (
    cd "$ROOT_DIR"
    HOME="$HOME_DIR" \
    ARBOR_DAEMON_URL="$DAEMON_URL" \
    ARBOR_DAEMON_AUTH_TOKEN="$AUTH_TOKEN" \
    target/debug/arbor-cli --json terminals create \
      --cwd "$ROOT_DIR" \
      --session-id "$SESSION_ID" \
      --title "perf-${MODE}" \
      --command "exec /bin/bash ${quoted_workload}" \
      >"$RUN_DIR/create-terminal.json"
  )
  cat "$RUN_DIR/create-terminal.json"
}

run_gui_foreground() {
  load_state
  cd "$ROOT_DIR"
  HOME="$HOME_DIR" \
  ARBOR_HTTPD_PORT="$PORT" \
  ARBOR_DAEMON_URL="$DAEMON_URL" \
  ARBOR_DAEMON_AUTH_TOKEN="$AUTH_TOKEN" \
  ARBOR_APP_ID="$APP_ID" \
  ARBOR_BUILD_BRANCH="${ARBOR_BUILD_BRANCH:-$(git branch --show-current 2>/dev/null || true)}" \
  exec -a "${APP_ID}.gui" "$ROOT_DIR/target/debug/Arbor"
}

main() {
  local action="${1:-start}"
  local mode="${2:-dashboard}"

  case "$action" in
    build)
      ensure_built
      ;;
    start)
      start_harness "$mode"
      ;;
    daemon)
      run_daemon_foreground
      ;;
    seed)
      seed_workload_terminal
      ;;
    gui)
      run_gui_foreground
      ;;
    status)
      status_harness
      ;;
    sample)
      sample_harness
      ;;
    stop)
      stop_harness
      ;;
    -h|--help|help)
      usage
      ;;
    *)
      usage >&2
      exit 2
      ;;
  esac
}

main "$@"
