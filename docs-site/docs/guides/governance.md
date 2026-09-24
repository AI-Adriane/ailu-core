---
title: "Governance"
description: "Named approvals, signed decisions, replayable runs, and secrets kept out of prompts and logs."
---

# Governance

Governance in Ailu answers three questions about every run: **who** allowed each sensitive
action, **what** the agent saw and did, and **can we prove it** later. The building blocks:

| Need | Tool |
| --- | --- |
| A person decides before an agent acts | [Tool approval](./tools.md) and [human gates](./human-approval.md) |
| A record of each decision that can't be edited unnoticed | [Signed decisions](#sign-approval-decisions) |
| Proof that a run happened as recorded | [Replay](#replay-a-run) |
| No secrets or personal data sent to a model or written to logs | [Redaction](#redact-secrets-and-personal-data) and [`noLog`](#keep-secrets-out-of-logs) |

## Sign approval decisions

An approval engine keeps the approval requests and who resolved them. An attestor signs each
decision with Ed25519 and links it to the previous one, so removing or editing a record breaks the
chain.

```ts file=packages/graph-sdk/examples/docs/governance-attestation.ts region=example
```

- `approvals.approve(id, user)` refuses a user who is also the requester
  (`ApprovalSelfApprovalError`), and refuses to resolve a request twice.
- `verifyChain(records)` checks the links and every signature. It checks each signature against
  the public key stored in the record, so also compare that key with the one you published.
- `InMemoryApprovalEngine` is for development. In production, implement the `ApprovalEngine`
  interface over your database.

To file approval requests automatically when a run stops, run the graph with the catalog runner
and pass the engine: `runCatalogGraph(app.definition, { approvalEngine, tools })`. Each gated
tool call and each human gate gets a request you can list with `approvalEngine.getPending(runId)`;
their ids are also saved in the run's state.

Pass the same engine to `resumeCatalogGraph`. It checks the engine before anything runs, and
throws `ApprovalNotGrantedError` if a request the run waits on is still pending, a human gate was
rejected, or a tool in `approvedTools` isn't approved by the person the grant names.
[Resume across processes](../examples/resume-across-processes.md) shows it.

:::caution
Without `approvalEngine`, `resumeCatalogGraph` doesn't check anything: it continues past a human
gate.
:::

Audit exports from Ailu Studio can be checked by anyone, offline:
`npx @ailu-ai/verify capsule.json --key <published public key>`.

## Replay a run

With `AILU_LLM_RECORD=1`, the catalog runner records every model call and timestamp of the run.
Store the recording with the run. Later, replay it: the engine re-runs the graph from its first
state, feeding it the recorded model outputs instead of calling a model, and must reach the same
result.

```ts file=packages/graph-sdk/examples/docs/governance-replay.ts region=example
```

To compare the approval decisions of a replay with the signed chain, use
`verifyReplayDecisions(attested, replayed)`: it returns `ok` and the list of mismatches.

## Redact secrets and personal data

Before any text reaches a model, the engine scans it for secrets (API keys, tokens, private keys)
and masks them. This is always on. Set `AILU_SECRETS_POLICY=block` to fail the call instead of
masking.

For personal data (names, emails, account numbers), point the engine at a redaction service with
`AILU_PII_REDACTOR_URL`. The service receives the outgoing texts and returns them redacted. If it
is unreachable, the text is sent unredacted unless you set `AILU_PII_REDACTOR_FAIL_CLOSED=1`,
which is what you want in production.

## Keep secrets out of logs

A channel marked `noLog: true` is masked in every run event, so it never reaches your logs or
traces. It is still checkpointed, so the run can resume.

```ts file=packages/graph-sdk/examples/docs/governance-no-log.ts region=example
```

## Next

- Send traces and costs to your observability stack: [Observability](./observability.md).
- Production settings: [Deploy to production](./production.md).
