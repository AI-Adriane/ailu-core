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
| `approvalWhen` | With `requiresApproval`, only the calls that cross a threshold wait — see below. |

The handler receives the parsed input and returns a JSON value, which the agent sees as the
tool's result.

Tools without `requiresApproval` run as soon as the agent calls them.

## Approve a call

With `suspendForApproval: true`, an agent that wants a gated tool stops the run:

1. `run()` returns `status: "suspended"`. The agent's result lists what it wants in
   `approvalRequests`, for example
   `{ subject: "tool:refund", reason: "...", input: { order: "A-7", amount: 40 }, callKey: "refund#3f…" }`.
   `input` is the call's arguments; `callKey` is `<name>#<sha256>` of those arguments with their
   keys sorted — the same call always has the same key, so you can sign it and check that a
   replay asks for it again. `app.explain(runId).summary` says the same in one sentence.
2. Show the request to a person in your app — its arguments too, masked as your policy requires.
3. When they approve, call
   `approveAndResume(runId, { approvedTools: ["refund"], resolvedBy: "<their user id>" })`.
   The agent runs again and can now call `refund`.

If they refuse, don't resume. The run stays suspended; you can discard it or keep it for the
record.

:::caution Who approves
`resolvedBy` is required: take it from your authenticated session. It is recorded with the
approval. The SDK refuses an empty value (`AILU_APPROVER_REQUIRED`), and the engine refuses a
grant where the approver is the agent that asked.
:::

## Approve one call at a time

By default a signature on `refund` lets the agent call `refund` again, with any arguments, until
the run is resumed again. To make each signature cover one call, set the agent's scope:

```ts
createGraph({ name: "refunds" }).agentNode("assistant", {
  model,
  tools,
  suspendForApproval: true,
  approvalScope: "call"
});
```

In Python: `agent_node("assistant", ..., approval_scope="call")`; on a catalog carrier:
`approvalScope: "call"`.

Each gated call then waits with its own `approvalKey` (equal to its `callKey`). Resume with that
key — `approvedTools: [{ name: "refund", key: request.approvalKey }]` — and only that call runs,
once: a grant by name unlocks nothing, and another call, or the same one again, waits for a new
signature.

:::caution A resumed agent starts again
On a resume the agent runs again from its first step, so it issues again the calls it made before
it stopped. With `approvalScope: "call"`, a call already approved in an earlier resume waits again
rather than running a second time silently — show the signer that the call repeats one already
signed. See ADR 0051 D6.
:::

## Approve only above a threshold

Some calls are fine on their own and others need a person: a refund of 20 € can go, a refund of
2 000 € waits. Put the threshold on the tool:

```ts
tools.register(
  {
    id: "refund" as ToolId,
    name: "refund",
    description: "Refunds an order.",
    inputSchema,
    outputSchema,
    permissions: ["payments:write"],
    requiresApproval: true,
    approvalWhen: [{ argument: "amount", above: 500 }]
  },
  refundHandler
);
```

In Python: `Tool(refund, requires_approval=True, approval_when=[{"argument": "amount", "above": 500}])`.

The engine reads `amount` from each call's input:

- `500` or less: the call runs, no gate.
- above `500`, absent, or not a number (`"600"` written as text is not a number): the call waits.
  The request names what crossed — `condition: "amount 600 > 500"` — and carries the call's
  `input` and its `approvalKey`.

The approval is for that call, not for the tool. Resume with its key:

```ts
approveAndResume(runId, {
  approvedTools: [{ name: "refund", key: request.approvalKey }],
  resolvedBy: "<their user id>"
});
```

A grant by name alone unlocks nothing: the call waits again. Another call above the threshold —
even a smaller one — waits too. `argument` is a top-level field of the input and `above` a number;
anything else is refused when the agent is built.

## Approve only for named values

One tool can reach many places — `a2a_delegate` sends a task to whichever remote agent its
`agentName` names. To ask a person only when the call goes to some of them, name them:

```ts
approvalWhen: [{ argument: "agentName", in: ["Nordlys"] }]
```

In Python: `approval_when=[{"argument": "agentName", "in": ["Nordlys"]}]`.

- `agentName` is `"Nordlys"`: the call waits — `condition: 'agentName = "Nordlys"'` — and its
  approval is for that call, resumed with its key as above.
- Another name (`"Veritas"`): the call runs, no gate.
- Absent, or not a string: the call waits.

The names are compared byte for byte: `"nordlys"` and `" Nordlys"` are other names. A condition
has one test — `above` or `in` — and a tool may carry several conditions: a call that crosses any
one of them waits, and the request names every one it crossed. `in` with no value, or an empty
value, is refused when the agent is built.

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
