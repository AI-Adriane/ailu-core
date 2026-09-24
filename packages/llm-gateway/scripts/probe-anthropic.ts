/**
 * Manual prototype probe — run this once a real ANTHROPIC_API_KEY is available, to
 * validate the adapter against the live API before relying on it in agent loops.
 *
 *   ANTHROPIC_API_KEY=sk-ant-... pnpm --filter @ailu/llm-gateway exec tsx scripts/probe-anthropic.ts
 *
 * It sends a representative ReAct-style prompt twice with an identical cacheable
 * prefix (system + tools) and prints token accounting for each call. On the second
 * call `cacheReadTokens` should be > 0 — that confirms the cache prefix is stable
 * and the breakpoints land. If it stays 0, a silent invalidator is at work.
 */
import { AnthropicProviderAdapter } from "../src/anthropic-adapter.js";
import type { LLMRequest } from "../src/types.js";

const SYSTEM = [
  "You are a ReAct agent. Reason step by step, then act.",
  "Always respond with a short final answer.",
  // Pad the prefix past the 4096-token cache minimum for Opus so caching engages.
  "Reference notes:\n" + "All tools are idempotent and safe to retry. ".repeat(400)
].join("\n\n");

const TOOLS: LLMRequest["tools"] = [
  { name: "search", description: "Search the corpus", inputSchema: { query: { type: "string" } } },
  { name: "calculator", description: "Evaluate arithmetic", inputSchema: { expr: { type: "string" } } }
];

const request = (question: string): LLMRequest => ({
  provider: "anthropic",
  model: "claude-opus-4-8",
  system: SYSTEM,
  tools: TOOLS,
  maxTokens: 256,
  messages: [{ role: "user", content: question }]
});

const main = async (): Promise<void> => {
  if (process.env.ANTHROPIC_API_KEY === undefined) {
    console.error("Set ANTHROPIC_API_KEY to run the probe.");
    process.exitCode = 1;
    return;
  }

  const adapter = new AnthropicProviderAdapter();

  console.log("Call 1 (cold cache) …");
  const first = await adapter.complete(request("What is 2 + 2?"));
  console.log(JSON.stringify({ content: first.content.slice(0, 120), usage: first.usage }, null, 2));

  console.log("Call 2 (warm cache, same prefix) …");
  const second = await adapter.complete(request("What is 10 * 3?"));
  console.log(JSON.stringify({ content: second.content.slice(0, 120), usage: second.usage }, null, 2));

  const cached = second.usage.cacheReadTokens ?? 0;
  console.log(
    cached > 0
      ? `\n✓ Cache engaged: ${cached} tokens read from cache on call 2.`
      : "\n⚠ No cache read on call 2 — prefix may be below the cache minimum or a silent invalidator is present."
  );
};

void main();
