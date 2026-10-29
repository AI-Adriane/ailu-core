#!/usr/bin/env bash
# Single entry point for reproducible Rust checks, used by local pnpm scripts and CI.
set -euo pipefail

ensure_cargo() {
  if command -v cargo >/dev/null 2>&1; then
    return
  fi

  local cargo_env="${CARGO_HOME:-$HOME/.cargo}/env"
  if [[ -f "$cargo_env" ]]; then
    # rustup installs cargo for login shells; npm/CI shells may need this explicitly.
    # shellcheck disable=SC1090
    . "$cargo_env"
  fi

  if ! command -v cargo >/dev/null 2>&1; then
    echo "error: cargo not found. Install Rust with rustup; the repo pins the toolchain in rust-toolchain.toml." >&2
    exit 127
  fi
}

usage() {
  cat >&2 <<'USAGE'
usage: bash scripts/rust-check.sh [all|build|fmt|lint|test]

Runs against crates/Cargo.toml with the locked dependency graph.
Default: all
USAGE
}

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd -- "$script_dir/.." && pwd)"
manifest="$repo_root/crates/Cargo.toml"

run_build() {
  cargo build --locked --manifest-path "$manifest" --workspace
}

run_fmt() {
  cargo fmt --all --manifest-path "$manifest" -- --check
}

# The sandbox crate keeps its simulated backend (`sim`) and its conformance suite (`conformance`)
# behind Cargo features that are off by default, so the workspace-wide runs never compile them.
# Check them as well.
sandbox_features="sim,conformance"

run_lint() {
  cargo clippy --locked --manifest-path "$manifest" --workspace --all-targets -- -D warnings
  cargo clippy --locked --manifest-path "$manifest" -p ailu-sandbox --all-targets \
    --features "$sandbox_features" -- -D warnings
  # The simulator alone, as a crate that depends on it without the suite would build it.
  cargo clippy --locked --manifest-path "$manifest" -p ailu-sandbox --all-targets \
    --features sim -- -D warnings
  # The suite alone, without the simulator it is also tested against.
  cargo clippy --locked --manifest-path "$manifest" -p ailu-sandbox --features conformance \
    -- -D warnings
}

run_test() {
  cargo test --locked --manifest-path "$manifest" --workspace
  cargo test --locked --manifest-path "$manifest" -p ailu-sandbox --features "$sandbox_features"
  cargo test --locked --manifest-path "$manifest" -p ailu-sandbox --features sim
}

ensure_cargo

case "${1:-all}" in
  all)
    run_fmt
    run_lint
    run_test
    ;;
  build)
    run_build
    ;;
  fmt)
    run_fmt
    ;;
  lint | clippy)
    run_lint
    ;;
  test)
    run_test
    ;;
  -h | --help | help)
    usage
    ;;
  *)
    usage
    exit 2
    ;;
esac
