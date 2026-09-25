---
title: "Install"
description: "Install the Ailu SDK, set an API key, or run offline."
---

# Install

## Requirements

- Node.js 22 or later.
- macOS (x64 or Apple silicon), Linux with glibc (x64 or arm64), or Windows x64. The Rust engine
  ships prebuilt for these platforms. Alpine (musl) and Windows on ARM are not supported yet.

## Start a new project

```bash
npm create @ailu-ai@latest my-app
cd my-app
npm install
npm start
```

`npm start` runs `app.ts`: a small graph that stops at a human gate and resumes.
`npm run inspect` opens the same graph in the browser inspector.

## Add Ailu to an existing project

```bash
npm install @ailu-ai/graph-sdk
```

The SDK is an ES module. Run TypeScript files with [tsx](https://tsx.is): `npx tsx app.ts`.

To check that the engine loaded on your machine:

```ts file=packages/graph-sdk/examples/docs/fragments.ts region=engine-check
```

If it prints `false`, your platform has no prebuilt engine. `compile()` then throws
`RustEngineRequiredError`; there is no slower fallback.

## Set an API key

Agents call a model provider. Set the key of the provider you use as an environment variable:

| Provider | `model.` | Variable |
| --- | --- | --- |
| Anthropic | `model.anthropic(...)` | `ANTHROPIC_API_KEY` |
| OpenAI | `model.openai(...)` | `OPENAI_API_KEY` |
| Google Gemini | `model.gemini(...)` | `GEMINI_API_KEY` or `GOOGLE_API_KEY` |
| Mistral | `model.mistral(...)` | `MISTRAL_API_KEY` |
| OpenRouter | `model.openrouter(...)` | `OPENROUTER_API_KEY` |
| MiniMax | `model.minimax(...)` | `MINIMAX_API_KEY` |
| Hugging Face | `model.huggingface(...)` | `HF_TOKEN` or `HUGGINGFACE_API_KEY` |
| Ollama (local) | `model.ollama(...)` | `AILU_USE_OLLAMA=1` |
| LM Studio (local) | `model.lmstudio(...)` | `AILU_USE_LMSTUDIO=1` |

A missing key is an error that names the variable to set. See
[Models and providers](./reference/models.md) for tiers and custom endpoints.

## Run without an API key

Set `AILU_LLM_MOCK=1` to run offline. Every agent without a key then answers from the engine's
deterministic mock: it calls each of its tools once, then answers `done`. Use it to try the
examples, and in tests and CI.

```bash
AILU_LLM_MOCK=1 npx tsx app.ts
```

:::caution
The mock is only used when you ask for it. Without `AILU_LLM_MOCK=1`, a run with no key fails.
:::

## Other tools

- **CLI**: `npm install -g @ailu-ai/cli` gives you `ailu validate`, `ailu compile` and
  `ailu diff` for YAML graphs. See [YAML and the CLI](./guides/yaml-and-cli.md).
- **Python**: `pip install ailu`. It validates and compiles graphs and runs prebuilt agents; it
  does not run graphs yet. See [Python and other languages](./guides/python.md).

Next: the [quickstart](./quickstart.md).
