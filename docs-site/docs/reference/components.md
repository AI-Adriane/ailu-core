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

To list components from code, use `componentCatalog`; for a JSON Schema of each component's
parameters, `componentSchemas()`.

<Components />
