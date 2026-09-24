import type { AgentResult } from "@ailu-ai/agents-core";

/**
 * The final answer of an agent run: the text after the last `final:` marker in its `reasoning`
 * (the whole `reasoning` when there is no marker). An empty string for a missing result.
 *
 * ```ts
 * const out = await app.run({ question: "What is a checkpoint?" });
 * console.log(finalAnswer(out.channels.agentResult));
 * ```
 *
 * For a typed answer, run the agent with a `structuredOutput` middleware and read
 * `result.structuredOutput` instead.
 */
export const finalAnswer = (result: Pick<AgentResult, "reasoning"> | undefined): string => {
  const reasoning = result?.reasoning ?? "";
  const marker = reasoning.toLowerCase().lastIndexOf("final:");
  return (marker === -1 ? reasoning : reasoning.slice(marker + "final:".length)).trim();
};
