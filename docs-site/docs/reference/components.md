---
title: "Components"
description: "Every built-in component, with its parameters. Components are ready-made nodes that run inside the engine."
---

import Components from "./_components.md";

# Components

A component is a ready-made node: prompt templating, validation, parsing, retrieval, ranking,
text processing. Components run inside the engine, without calling back into JavaScript, and they
work on every runner, including [`runCatalogGraph`](../guides/long-running.md).

```ts file=packages/graph-sdk/examples/docs/fragments.ts region=component
```

Parameters named `from`, `query` or `queryFrom` name the channel a component reads; `into` names
the channel it writes. The two integration components, `httpFetch` and `webSearch`, call
external services and are added with `.node()`.

`httpFetch` accepts public HTTP(S) destinations on standard ports. It validates DNS answers
at connection time and disables redirects. When using `urlFrom`, configure `allowedOrigins`
with exact HTTPS origins, for example `allowedOrigins: ["https://api.example.com"]`.
This requirement also applies when the channel is empty and `url` supplies a fallback.
An empty allowlist denies every destination. Private, local, metadata and special IP ranges,
URL credentials, and `Host`/`:authority` header overrides are rejected. Policy failures use
the existing `{ ok: false, error }` result.

Migration: channel URLs previously worked without destination authorization, and requests
could follow redirects or use private/custom-port endpoints. Those requests now fail closed.
Configure the allowed HTTPS origins and use final public URLs directly. A custom `fetchImpl`
is a trusted host capability; network implementations must honor the supplied
`init.dispatcher` and `init.redirect` (Node's `fetch` does), or enforce an equally strict
egress policy themselves. Offline fakes can continue returning response-shaped objects.

To list components from code, use `componentCatalog`; for a JSON Schema of each component's
parameters, `componentSchemas()`.

<Components />
