default:
    @just --list

nightly_toolchain := "nightly-2025-11-30"
web_ui_dir := "crates/arbor-web-ui/app"

setup-macos:
    xcodebuild -downloadComponent MetalToolchain
    brew install --cask font-caskaydia-cove-nerd-font

setup-linux:
    sudo apt-get update
    sudo apt-get install -y libxcb1-dev libxkbcommon-dev libxkbcommon-x11-dev
    #!/usr/bin/env bash
    set -euo pipefail
    FONT_DIR="${HOME}/.local/share/fonts"
    mkdir -p "${FONT_DIR}"
    curl -fLo /tmp/CascadiaCode.tar.xz https://github.com/ryanoasis/nerd-fonts/releases/latest/download/CascadiaCode.tar.xz
    tar -xf /tmp/CascadiaCode.tar.xz -C "${FONT_DIR}"
    rm /tmp/CascadiaCode.tar.xz
    fc-cache -fv

format:
    cargo +{{nightly_toolchain}} fmt --all

format-check:
    cargo +{{nightly_toolchain}} fmt --all -- --check

lockfile-check:
    cargo fetch --locked

lint: lockfile-check
    cargo +{{nightly_toolchain}} clippy --workspace --all-features --all-targets -- -D warnings

test:
    cargo +{{nightly_toolchain}} test --workspace --all-features

ghostty-vt-bridge:
    ./scripts/build-ghostty-vt-bridge.sh

test-ghostty-vt: ghostty-vt-bridge
    RUSTFLAGS="-L native=$(pwd)/target/ghostty-vt-bridge/lib -C link-arg=-Wl,-rpath,$(pwd)/target/ghostty-vt-bridge/lib ${RUSTFLAGS:-}" cargo +{{nightly_toolchain}} test -p arbor-terminal-emulator --features ghostty-vt-experimental

check-ghostty-vt-gui: ghostty-vt-bridge
    RUSTFLAGS="-L native=$(pwd)/target/ghostty-vt-bridge/lib -C link-arg=-Wl,-rpath,$(pwd)/target/ghostty-vt-bridge/lib ${RUSTFLAGS:-}" cargo +{{nightly_toolchain}} check -p arbor-gui --features ghostty-vt-experimental

check-ghostty-vt-httpd: ghostty-vt-bridge
    RUSTFLAGS="-L native=$(pwd)/target/ghostty-vt-bridge/lib -C link-arg=-Wl,-rpath,$(pwd)/target/ghostty-vt-bridge/lib ${RUSTFLAGS:-}" cargo +{{nightly_toolchain}} check -p arbor-httpd --features ghostty-vt-experimental

zizmor:
    zizmor .github/workflows/

ci: format-check lint test

run port="": web-ui-build-if-needed
    #!/usr/bin/env bash
    set -euo pipefail
    if [ -z "{{port}}" ]; then
      DAEMON_PORT=$(python3 -c 'import socket; s=socket.socket(); s.bind(("",0)); print(s.getsockname()[1]); s.close()')
    else
      DAEMON_PORT="{{port}}"
    fi
    echo "daemon port: $DAEMON_PORT"
    export ARBOR_DAEMON_URL="http://127.0.0.1:${DAEMON_PORT}"
    export ARBOR_HTTPD_PORT="${DAEMON_PORT}"
    cargo +{{nightly_toolchain}} run -p arbor-httpd &
    HTTPD_PID=$!
    trap "kill $HTTPD_PID 2>/dev/null" EXIT
    cargo +{{nightly_toolchain}} run -p arbor-gui
    kill $HTTPD_PID 2>/dev/null || true

web-ui-build:
    cd {{web_ui_dir}} && npm install --no-audit --no-fund && npm run build

web-ui-build-if-needed:
    @if [ -f {{web_ui_dir}}/dist/index.html ]; then \
      echo "web-ui assets already built"; \
    else \
      cd {{web_ui_dir}} && npm install --no-audit --no-fund && npm run build; \
    fi

build-release: web-ui-build-if-needed
    cargo +{{nightly_toolchain}} build --release -p arbor-gui -p arbor-httpd

run-httpd: web-ui-build-if-needed
    cargo +{{nightly_toolchain}} run -p arbor-httpd

run-mcp port="": web-ui-build-if-needed
    #!/usr/bin/env bash
    set -euo pipefail
    if [ -z "{{port}}" ]; then
      DAEMON_PORT=$(python3 -c 'import socket; s=socket.socket(); s.bind(("",0)); print(s.getsockname()[1]); s.close()')
    else
      DAEMON_PORT="{{port}}"
    fi
    export ARBOR_DAEMON_URL="http://127.0.0.1:${DAEMON_PORT}"
    export ARBOR_HTTPD_PORT="${DAEMON_PORT}"
    cargo +{{nightly_toolchain}} run -p arbor-httpd &
    HTTPD_PID=$!
    trap "kill $HTTPD_PID 2>/dev/null" EXIT
    cargo +{{nightly_toolchain}} run -p arbor-mcp --features stdio-server

changelog:
    git-cliff --config cliff.toml --output CHANGELOG.md

changelog-unreleased:
    git-cliff --config cliff.toml --unreleased

changelog-release version:
    git-cliff --config cliff.toml --unreleased --tag "v{{version}}" --strip all
