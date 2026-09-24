---
title: "Tools and approval"
description: "Give an agent tools, and make it stop for a named human before it calls the sensitive ones."
---

# Tools and approval

A tool is a function an agent can call. You describe it (name, description, input schema) and
register a handler. Mark it `requiresApproval: true` and the agent can't call it until a human
says yes.

```ts file=packages/graph-sdk/examples/docs/tools-approval.ts region=example
```

## Describe a tool

`InMemoryToolRegistry.register(definition, handler)` takes:

| Field | Purpose |
| --- | --- |
| `id`, `name` | The tool's identity. The model calls it by `name`. |
| `description` | When to use it. The model reads this. |
| `jsonSchema` | The JSON Schema of the input. The model reads this to build the call. |
| `inputSchema`, `outputSchema` | Objects with a `parse(value)` method that validate the input and output in your code. A Zod schema fits. |
| `permissions` | Labels for your own audit, such as `"payments:write"`. |
| `requiresApproval` | `true` makes every call wait for a human. |

The handler receives the parsed input and returns a JSON value, which the agent sees as the
tool's result.

Tools without `requiresApproval` run as soon as the agent calls them.

## Approve a call

With `suspendForApproval: true`, an agent that wants a gated tool stops the run:

1. `run()` returns `status: "suspended"`. The agent's result lists what it wants in
   `approvalRequests`, for example `{ subject: "tool:refund", reason: "..." }`.
   `app.explain(runId).summary` says the same in one sentence.
2. Show the request to a person in your app.
3. When they approve, call
   `approveAndResume(runId, { approvedTools: ["refund"], resolvedBy: "<their user id>" })`.
   The agent runs again and can now call `refund`.

If they refuse, don't resume. The run stays suspended; you can discard it or keep it for the
record.

:::caution Who approves
Always pass `resolvedBy`, from your authenticated session. It is recorded with the approval. The
engine refuses an empty value and refuses a grant where the approver is the agent that asked.
:::

## Keep the handler honest

The model chooses the tool's input. Treat it like user input:

- validate it in `inputSchema.parse`;
- check permissions in the handler (does this user own this order?);
- keep handlers idempotent where you can: a resumed run may call a tool again.

## Record approvals as evidence

To sign each decision and let an auditor check it later, see
[Governance](./governance.md#sign-approval-decisions). To resume an approval in another process
(after a deploy, or from a queue worker), see [Long-running runs](./long-running.md).

## Next

- Stop the whole run, not just a tool: [Human approval](./human-approval.md).
- Full app: [Governed refund agent](../examples/refund-agent.md).
