# Contributing to Ailu

Thanks for your interest in Ailu — the governed agentic graph framework. This guide covers the
ways to contribute, how the repo is laid out, how to set up the project (code **and** docs), the
standards we hold contributions to, and the sign-off we require on every commit.

## Ways to contribute

- **Docs** — fix a typo, clarify a guide, add a cookbook recipe, or document an integration. The
  fastest way in; see [Contributing to the documentation](#contributing-to-the-documentation).
- **Code** — engine (Rust), the TypeScript / Python SDKs, or supporting packages.
- **Issues** — a clear, reproducible bug report is a real contribution. (Security issues go to
  [`SECURITY.md`](./SECURITY.md), **never** a public issue.)

Good first contributions are labelled `good first issue`. When in doubt, open a draft PR or a
discussion before a large change — especially anything structural (see
[Mandatory review](#mandatory-review)).

## Repository layout

```
crates/          The Rust engine — the single source of truth (graph model, runtime, validator,
                 DSL compilers, agents, gateway, fs, approval). napi + pyo3 bindings live here.
packages/        The TypeScript SDK + supporting packages (graph-sdk, contracts, llm-gateway,
                 rag-pipeline, …). Thin shims over the Rust engine.
python/          The Python SDK packaging (pyo3 wheel).
docs-site/       The Docusaurus documentation site (this is what docs PRs touch).
docs/adr/        Architecture Decision Records — structural decisions are recorded here.
plugin/          The Claude Code plugin.
```

One engine, two first-class SDKs: the graph model / validator / compiler live once in Rust, and
the SDKs are thin surfaces over it. A change to engine behaviour usually touches the crate **and**
the SDK that exposes it.

## Development setup

Prerequisites: **Node ≥ 22**, **pnpm 10**, and a **stable Rust toolchain** (pinned in
`rust-toolchain.toml`). The framework is storage-agnostic — no database is required to build, test,
or run it.

```bash
pnpm install

# TypeScript / JS gates (run across all workspaces via Turbo):
pnpm build
pnpm typecheck
pnpm test
pnpm lint

# Rust engine:
pnpm rust:fmt    # cargo fmt --check
pnpm rust:lint   # cargo clippy --all-targets -D warnings
pnpm rust:test   # cargo test

# Native bindings (optional — enables the Rust fast-path locally):
pnpm napi:build  # builds crates/bindings/ailu_napi.node
pnpm py:build    # builds the pyo3 extension for local `import ailu`
```

## Contributing to the documentation

The docs are a [Docusaurus](https://docusaurus.io) site under [`docs-site/`](./docs-site). They are
the public face of the project — clear docs are as valuable as code.

### Run the site locally

```bash
cd docs-site
pnpm install
pnpm start          # live-reload dev server at http://localhost:3000
pnpm build          # production build — also the CI gate (fails on broken links)
```

Always run `pnpm build` before opening a docs PR: it validates every internal link and all MDX. A
broken link fails the build.

### Where pages live

Pages are Markdown/MDX under `docs-site/docs/`, grouped by feature folder. The site's navigation is
a **manual sidebar** in [`docs-site/sidebars.js`](./docs-site/sidebars.js) — files on disk are
grouped there into the top-level sections (**Get started · Build · Cookbook · Govern · Monitor ·
Deploy · API Reference**), so **a new page must be added to `sidebars.js`** to appear. The
information architecture mirrors a mature docs site (e.g. LangChain): *Build* is split into Concepts,
Graphs, Agents, Deep agents, Integrations, DSL, and more.

Each page starts with frontmatter:

```markdown
---
sidebar_position: 3
title: Agent nodes & ReAct
description: One-line summary used for SEO and the category index.
---
```

### Adding an integration

Integrations (models, middleware, backends, checkpointers, retrievers, text splitters, vector
stores) are split one page per integration under `docs-site/docs/integrations/`. To add one, copy
the shape of an existing page (e.g. `integrations/models/anthropic.md`), document **only what the
code actually supports** — mark anything not yet implemented as *Planned* or an *external seam* —
and add it to the relevant category in `sidebars.js`.

### Style

- Terse, technical, **code-first**. Show a runnable TypeScript snippet early.
- Use tables for config / options. Cross-link related pages with relative links.
- Don't document an API that doesn't exist. If unsure, read the source under
  `packages/graph-sdk/src/` or `crates/`.

## Standards (code)

- **TypeScript strict everywhere** — no `as any`, no `@ts-ignore`, no skipped tests. Validate
  external data with Zod. Errors are typed classes, not bare `throw new Error(...)`. Public API
  only through each package's `src/index.ts`.
- **No `eval` / `new Function` / dynamic `import()` of user strings** anywhere. Graph conditions are
  named registry predicates, never eval'd code.
- **Tests live next to source** (`foo.ts` → `foo.test.ts`), behavior-focused; mock only I/O
  boundaries (DB / HTTP / LLM).
- **Conventional Commits** for messages. Prettier: double quotes, semicolons, no trailing commas,
  width 100.

The authoritative per-layer rules live in `.cursor/rules/*.mdc` — worth reading before structural
work.

## Mandatory review

Structural changes are recorded as an **ADR** under [`docs/adr/`](./docs/adr) and reviewed before
implementation: runtime invariants (determinism, checkpointing, events, human gates), public engine
APIs, security-relevant changes, and large rewrites. Propose the change (an ADR if structural), then
wait for review — don't land it unilaterally. Small fixes and docs don't need an ADR.

## Pull requests

1. Branch off `main`.
2. Keep PRs focused; explain the *why* in the description.
3. Ensure the gates pass: `pnpm typecheck && pnpm test && pnpm lint`, the Rust gates, and (for docs)
   `cd docs-site && pnpm build`.
4. **Sign off every commit** (`-s`) — see the DCO below.
5. CI (`.github/workflows/*`) must be green.

## Developer Certificate of Origin (DCO)

We use the [Developer Certificate of Origin](https://developercertificate.org/) — not a CLA. By
signing off a commit you certify that you wrote the patch or otherwise have the right to submit it
under the project's license.

**Every commit must be signed off.** Add a `Signed-off-by` trailer with your real name and email:

```bash
git commit -s -m "feat(runtime): add fan-out budget guard"
```

This appends `Signed-off-by: Your Name <you@example.com>`. PRs whose commits are not signed off
cannot be merged. To sign off a branch you already wrote: `git rebase --signoff main`.

<details>
<summary>The DCO text you are certifying</summary>

```
By making a contribution to this project, I certify that:

(a) The contribution was created in whole or in part by me and I have the right to
    submit it under the open source license indicated in the file; or
(b) The contribution is based upon previous work that, to the best of my knowledge,
    is covered under an appropriate open source license and I have the right under
    that license to submit that work with modifications; or
(c) The contribution was provided directly to me by some other person who certified
    (a), (b) or (c) and I have not modified it.
(d) I understand and agree that this project and the contribution are public and
    that a record of the contribution (including all personal information I submit
    with it, including my sign-off) is maintained indefinitely.
```
</details>

## License

This repository is the **open Ailu framework** — the Rust engine (`crates/*`), the TypeScript and
Python SDKs and supporting packages (`packages/*`), and the Claude Code plugin (`plugin/*`), all
under Apache-2.0. Contributions are made under Apache-2.0 (inbound = outbound). See
[`LICENSE`](./LICENSE).

## Security

Do **not** open public issues for vulnerabilities — see [`SECURITY.md`](./SECURITY.md).
