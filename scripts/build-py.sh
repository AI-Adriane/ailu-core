#!/usr/bin/env bash
# Reproducible dev build for the ailu-py (pyo3) extension module.
# Builds the cdylib with cargo and copies it to python/ailu/ailu.abi3.so
# (abi3-py39), which `import ailu` loads. macOS linking is handled by
# crates/py-bindings/build.rs (-undefined dynamic_lookup).
set -euo pipefail

if ! command -v cargo >/dev/null 2>&1; then
  # rustup installs cargo for login shells; non-login shells may need the env sourced.
  # shellcheck disable=SC1091
  . "$HOME/.cargo/env"
fi

cd "$(dirname "$0")/../crates"

cargo build --locked -p ailu-py

case "$(uname -s)" in
  Darwin) LIB_NAME="libailu.dylib" ;;
  Linux) LIB_NAME="libailu.so" ;;
  *) LIB_NAME="ailu.dll" ;;
esac

DEST="../python/ailu/ailu.abi3.so"
cp "target/debug/${LIB_NAME}" "$DEST"

echo "ailu-py dev build OK -> $(cd ../python/ailu && pwd)/ailu.abi3.so"
