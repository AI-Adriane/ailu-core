# Releasing Ailu

This repository publishes **three** open artifacts. Ailu Studio (the
commercial control plane) lives in a separate repository and is **never**
published from here.

| Artifact | Registry | What it is |
| --- | --- | --- |
| `@ailu/graph-sdk` | npm | The self-contained TypeScript SDK bundle (front door) |
| `@ailu/napi` (+ `@ailu/napi-<triple>`) | npm | Prebuilt per-platform native engine addon |
| `ailu` | PyPI | The Python SDK (abi3 wheels) |

> The Rust crates under `crates/*` are **not** published to crates.io — they ship
> only as the napi/PyPI artifacts above. The two binding crates carry
> `publish = false`; the engine crates are not a crates.io publish target.

## ⚠️ Never `pnpm publish -r`

A recursive publish would try to push the deprecated TS engine packages that are
*bundled into* `@ailu/graph-sdk` and are **not** meant to be standalone public
packages. Every `packages/*` except `@ailu/graph-sdk` is now marked
`"private": true`, so `-r` physically cannot publish them — but stay explicit
anyway: publish only the artifacts above, by filter. The tagged CI workflow does
exactly this. `@ailu/graph-sdk` also has a `prepublishOnly` guard that refuses a
non-pnpm publish (npm would not rewrite its `workspace:*` napi dep).

## The automated path (recommended)

Releases run from [`.github/workflows/release.yml`](.github/workflows/release.yml)
on a `v*` tag:

```bash
# from a clean main with the version bumped (see below):
git tag v0.1.0
git push origin v0.1.0
```

The workflow then, in order:

1. **`napi-build`** — builds `@ailu/napi` for each target
   (`napi build --platform --release`) and uploads each `.node`.
2. **`npm-publish`** — assembles the per-platform packages (`napi artifacts` +
   `napi prepublish`), publishes `@ailu/napi-<triple>` and `@ailu/napi`, then
   builds and publishes the `@ailu/graph-sdk` bundle. **napi publishes first** so
   the SDK's `@ailu/napi` optionalDependency resolves on install.
3. **`python-wheels`** + **`pypi-publish`** — builds an abi3 wheel per platform
   (`maturin`, one `cp39-abi3` wheel covers CPython 3.9+) plus an sdist, and uploads
   to PyPI.

### Required repository secrets

- `NPM_TOKEN` — npm automation token with publish rights to the `@ailu` scope.
- `PYPI_API_TOKEN` — PyPI API token for the `ailu` project.

### ⚠️ Validate on a pre-release tag first

The GitHub Actions orchestration (cross-compilation, `napi prepublish`, artifact
plumbing) has **not** run end-to-end yet — only the individual build commands are
validated locally on darwin-arm64. Shake it out with a pre-release tag before a real
release:

```bash
git tag v0.1.0-rc.1 && git push origin v0.1.0-rc.1
```

npm/PyPI ignore pre-release tags for `latest`, so an `-rc.N` is safe to iterate on.

## Manual publish (fallback)

If you publish by hand, keep the **napi-before-sdk** order and never use `-r`:

```bash
# 1. Build the native addon for the platforms you can, then (per napi-rs docs)
#    `napi prepublish` to publish @ailu/napi-<triple> + @ailu/napi.
pnpm --filter @ailu/napi run build:napi
( cd crates/bindings && pnpm exec napi prepublish -t npm )

# 2. The SDK bundle (a clean tsup build, then publish the single artifact):
pnpm --filter @ailu/graph-sdk build
pnpm --filter @ailu/graph-sdk publish --access public

# 3. Python wheels (from a venv with maturin):
cd python && maturin publish     # or `maturin build --release` + `twine upload`
```

`pnpm pack`/`npm pack --dry-run` in `packages/graph-sdk` is a good pre-flight — the
tarball should contain only `dist/`, `README.md`, `LICENSE`, and `package.json`,
with no `workspace:*` specifiers left in `dependencies`.

## Versioning

All artifacts share the workspace version (`0.0.1` today). Before a real release,
bump in lockstep:

- npm: each `packages/*` and `crates/bindings/package.json` `version`.
- Rust/Python: `crates/Cargo.toml` `[workspace.package] version` (the Python wheel
  derives its version from the crate; the pyproject `version` must match).

## Open decisions before the first public release

- **Name availability** — verify `ailu` is free on npm and PyPI (and the
  `ailu.dev` domain). If taken, choose a scoped/namespaced name and update the
  three manifests + READMEs together.
- **Telemetry** — decide whether the SDK/CLI emit any anonymous usage telemetry, and
  if so, document the opt-out *before* publishing.

The `pnpm oss:reserve-names` script (`scripts/reserve-oss-names.sh`) prepares and
optionally publishes 0.0.0 placeholder packages to reserve the registry names
before the first real release.
