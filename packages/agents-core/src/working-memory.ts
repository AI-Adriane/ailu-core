import type { Message } from "@ailu/graph-core";
import type { LLMGateway } from "../../llm-gateway/src/interfaces.js";
import type { BaseStore } from "../../memory-store/src/interfaces.js";

export type WorkingMemory = {
  shortTerm: Message[];
  longTerm: BaseStore;
};

const defaultCountFn = (message: Message): number => {
  const content = typeof (message as { content?: unknown }).content === "string" ? message.content : "";
  return Math.max(1, Math.ceil(content.length / 4));
};

export const compressShortTerm = async (
  messages: Message[],
  llm: LLMGateway,
  maxTokens: number
): Promise<Message[]> => {
  let tokens = messages.reduce((sum, message) => sum + defaultCountFn(message), 0);
  if (tokens <= maxTokens) {
    return messages;
  }

  const keepCount = Math.max(1, Math.floor(messages.length / 2));
  const toSummarize = messages.slice(0, messages.length - keepCount);
  const toKeep = messages.slice(messages.length - keepCount);
  const payload = JSON.stringify(
    toSummarize.map((message) => ({
      role: message.role,
      content: "content" in message ? message.content : ""
    }))
  );
  const completion = await llm.complete({
    provider: "openai",
    model: "working-memory-compressor",
    messages: [{ role: "user", content: `Summarize briefly:\n${payload}` }]
  });

  const summary: Message = {
    id: `summary:${Date.now()}` as Message["id"],
    role: "system",
    content: completion.content,
    createdAt: new Date()
  };
  const compressed = [summary, ...toKeep];
  tokens = compressed.reduce((sum, message) => sum + defaultCountFn(message), 0);
  if (tokens > maxTokens) {
    return compressed.slice(-Math.max(1, Math.floor(maxTokens / 4)));
  }
  return compressed;
};
