#!/usr/bin/env bash
# Prepare and optionally publish placeholder packages that reserve Ailu's
# public registry names before the first OSS release.
set -euo pipefail

VERSION="0.0.0"
TMP_ROOT="${TMPDIR:-/tmp}/ailu-name-reservation"
PYPI_NAME="ailu"
NPM_PACKAGES=(
  "@ailu-ai/create"
  "@ailu-ai/graph-sdk"
  "@ailu-ai/cli"
  "@ailu-ai/model-core"
  "@ailu-ai/contracts"
  "@ailu-ai/config"
  "@ailu-ai/verify"
  "@ailu-ai/napi"
  "@ailu-ai/napi-darwin-arm64"
  "@ailu-ai/napi-darwin-x64"
  "@ailu-ai/napi-linux-arm64-gnu"
  "@ailu-ai/napi-linux-x64-gnu"
  "@ailu-ai/napi-win32-x64-msvc"
)

usage() {
  cat >&2 <<'USAGE'
usage: bash scripts/reserve-oss-names.sh [check|prepare|publish-npm|publish-pypi|publish]

check        Verify current registry availability. No writes.
prepare      Generate placeholder npm/PyPI projects under $TMPDIR. No network writes.
publish-npm  Publish npm placeholders. Requires npm login with access to @ailu-ai.
publish-pypi Publish the PyPI placeholder. Requires build + twine + PyPI auth.
publish      Publish both npm and PyPI placeholders.

The placeholders use version 0.0.0 so the first real release can publish 0.0.1+.
USAGE
}

package_dir_name() {
  printf "%s" "$1" | sed 's#^@##; s#/#__#g'
}

check_npm_package() {
  local pkg="$1"
  local output
  if output="$(npm view "$pkg" version 2>&1)"; then
    printf "taken     npm  %s  version=%s\n" "$pkg" "$output"
    return 1
  fi

  if printf "%s" "$output" | grep -q "E404"; then
    printf "available npm  %s\n" "$pkg"
    return 0
  fi

  printf "unknown   npm  %s\n%s\n" "$pkg" "$output" >&2
  return 2
}

check_pypi_project() {
  local output
  if output="$(python3 -m pip index versions "$PYPI_NAME" 2>&1)"; then
    printf "taken     pypi %s\n%s\n" "$PYPI_NAME" "$output"
    return 1
  fi

  if printf "%s" "$output" | grep -q "No matching distribution found"; then
    printf "available pypi %s\n" "$PYPI_NAME"
    return 0
  fi

  printf "unknown   pypi %s\n%s\n" "$PYPI_NAME" "$output" >&2
  return 2
}

check_all() {
  local status=0
  for pkg in "${NPM_PACKAGES[@]}"; do
    check_npm_package "$pkg" || status=$?
  done
  check_pypi_project || status=$?
  return "$status"
}

prepare_npm_package() {
  local pkg="$1"
  local dir="$TMP_ROOT/npm/$(package_dir_name "$pkg")"
  mkdir -p "$dir"

  cat >"$dir/package.json" <<JSON
{
  "name": "$pkg",
  "version": "$VERSION",
  "description": "Reserved package name for Ailu. The first functional release will be published as 0.0.1+.",
  "license": "Apache-2.0",
  "private": false,
  "publishConfig": {
    "access": "public"
  },
  "files": [
    "README.md"
  ]
}
JSON

  cat >"$dir/README.md" <<README
# $pkg

Reserved package name for Ailu.

This 0.0.0 package intentionally contains no runtime code. The first functional
release will be published as 0.0.1 or later.
README
}

prepare_pypi_project() {
  local dir="$TMP_ROOT/pypi/$PYPI_NAME"
  mkdir -p "$dir/src/ailu_placeholder"

  cat >"$dir/pyproject.toml" <<TOML
[build-system]
requires = ["setuptools>=69", "wheel"]
build-backend = "setuptools.build_meta"

[project]
name = "$PYPI_NAME"
version = "$VERSION"
description = "Reserved project name for Ailu. The first functional release will be published as 0.0.1+."
readme = "README.md"
license = { text = "Apache-2.0" }
requires-python = ">=3.9"
classifiers = [
  "License :: OSI Approved :: Apache Software License",
  "Programming Language :: Python :: 3"
]

[tool.setuptools.packages.find]
where = ["src"]
TOML

  cat >"$dir/README.md" <<README
# ailu

Reserved project name for Ailu.

This 0.0.0 package intentionally contains no runtime code. The first functional
release will be published as 0.0.1 or later.
README

  cat >"$dir/src/ailu_placeholder/__init__.py" <<'PY'
"""Reserved placeholder for the Ailu Python package."""

__all__ = []
PY
}

prepare_all() {
  rm -rf "$TMP_ROOT"
  mkdir -p "$TMP_ROOT/npm" "$TMP_ROOT/pypi"
  for pkg in "${NPM_PACKAGES[@]}"; do
    prepare_npm_package "$pkg"
  done
  prepare_pypi_project
  printf "prepared placeholders under %s\n" "$TMP_ROOT"
}

publish_npm() {
  npm whoami >/dev/null
  prepare_all
  for pkg in "${NPM_PACKAGES[@]}"; do
    local dir="$TMP_ROOT/npm/$(package_dir_name "$pkg")"
    npm publish "$dir" --access public
  done
}

publish_pypi() {
  command -v python3 >/dev/null
  python3 -m build --version >/dev/null
  python3 -m twine --version >/dev/null

  prepare_all
  local dir="$TMP_ROOT/pypi/$PYPI_NAME"
  (cd "$dir" && python3 -m build)
  python3 -m twine upload "$dir"/dist/*
}

case "${1:-check}" in
  check)
    check_all
    ;;
  prepare)
    prepare_all
    ;;
  publish-npm)
    publish_npm
    ;;
  publish-pypi)
    publish_pypi
    ;;
  publish)
    publish_npm
    publish_pypi
    ;;
  -h | --help | help)
    usage
    ;;
  *)
    usage
    exit 2
    ;;
esac
