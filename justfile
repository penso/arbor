default:
    @just --list

nightly_toolchain := "nightly-2025-11-30"
web_ui_dir := "crates/arbor-web-ui/app"

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

ci: format-check lint test

run:
    cargo +{{nightly_toolchain}} run -p arbor-gui

web-ui-build-if-needed:
    @if [ -f {{web_ui_dir}}/dist/index.html ]; then \
      echo "web-ui assets already built"; \
    else \
      cd {{web_ui_dir}} && npm install --no-audit --no-fund && npm run build; \
    fi

run-httpd: web-ui-build-if-needed
    cargo +{{nightly_toolchain}} run -p arbor-httpd

changelog:
    git-cliff --config cliff.toml --output CHANGELOG.md

changelog-unreleased:
    git-cliff --config cliff.toml --unreleased

changelog-release version:
    git-cliff --config cliff.toml --unreleased --tag "v{{version}}" --strip all
