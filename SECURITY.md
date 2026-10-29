# Security Policy

## Reporting a vulnerability

**Please do not open a public issue for security vulnerabilities.**

Report privately through **GitHub's private vulnerability reporting** on this
repository (the _Security_ tab → _Report a vulnerability_). Once a project contact
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

## Sandbox port: design notes

- **One approval covers the retries of a call.** The approval is bound to the call (tenant,
  run, node and the exact requests), not to the attempt number. A retry of the same call
  reuses the grant; repetition is bounded by per-tenant quotas, not by a new decision.
- **A confirmed clock fault can empty the creation-rate window once.** This grants at most
  `max_creations` extra creations and can only be caused by the trusted host's clock.
- The port has no caller yet; output redaction work (#389) and input artifacts (#388) are
  prerequisites to enabling it.

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

## Known audit exceptions

An advisory reported by `pnpm audit` or `cargo audit` that the project accepts for a while is
listed here with its reason, a review date and a tracking issue. Listing it here does not silence the
audit job: the job stays red until the advisory is fixed.

None at present.

## Threat model and assessment context

The versioned [threat model](THREAT-MODEL.md) describes architecture, trust
boundaries, assumptions and review objectives. It is context for assessment, not
a passing security verdict or evidence that every intended control is implemented.

- The embedding host authenticates users, selects tenants and authorizes callback effects; the runtime must preserve its own metadata and execution boundaries.
- Graph inputs, model responses and caller-provided approver names are not authorization.
- Bind approvals to the exact operation and preserve this invariant through nested execution and resume.
- Protect restored state and run/namespace separation; bound resource demand before allocation or fan-out.
- Treat provider/backend responses and proof capsules as untrusted. Trusted provenance requires an independently approved issuer key.
- Logical filesystem policy and native execution do not establish OS sandbox isolation; qualify the concrete backend before enabling it.

Keep actual deployment facts separate from intended guarantees. Review changes to
identity, exposed services, approvals, persistence and data destinations against
these boundaries. Follow the private reporting process above for vulnerabilities;
do not publish confidential findings in documentation or public issues.

SECURITY.md provides repository-local security context. A linked Markdown file is
not evidence that Security Cloud has imported or activated a Cloud-managed model.
No monitoring, scan or Cloud configuration is enabled by this documentation change.
