# Security Policy

## Reporting a vulnerability

**Please do not open a public issue for security vulnerabilities.**

Report privately through **GitHub's private vulnerability reporting** on this
repository (the *Security* tab → *Report a vulnerability*). Once a project contact
address is published, email will also be accepted. Include:

- a description of the issue and its impact,
- steps to reproduce (a minimal graph/run or request if applicable),
- affected component (`crates/*` engine, `packages/*` SDK, `plugin/*`).

We aim to acknowledge reports within a few business days. As a pre-release,
solo-maintained project there is no formal SLA yet, but security reports are
triaged ahead of feature work.

## Supported versions

Ailu is pre-1.0 (`0.0.x`). Only the latest published version is supported;
fixes land on `main` and ship in the next release. There are no backports yet.

## Scope notes

The engine is designed with several hard security invariants — useful context when
assessing a report:

- **No `eval` / `new Function` / dynamic `import()` of user strings** anywhere.
  Graph conditions are named registry predicates, never executed code.
- **Agents cannot approve their own outputs**; sensitive actions route through an
  approval gate.
- **Secrets only via environment** (`.env` is gitignored). The SDK and engine never
  hardcode credentials.

A bypass of any of these is in scope and valued.
