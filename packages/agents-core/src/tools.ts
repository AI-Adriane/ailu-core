export type ZodSchema<T> = {
  parse(input: unknown): T;
};

export type ToolId = string & { readonly __brand: "ToolId" };

/**
 * One condition on a gated tool's arguments (ADR 0046): the call needs approval when the top-level
 * input field `argument` is above `above` — or absent, or not a number. Data, never an expression.
 */
export type ApprovalCondition = {
  argument: string;
  above: number;
};

export type ToolDefinition<TInput, TOutput> = {
  id: ToolId;
  name: string;
  description: string;
  inputSchema: ZodSchema<TInput>;
  outputSchema: ZodSchema<TOutput>;
  permissions: string[];
  requiresApproval?: boolean;
  /**
   * With `requiresApproval`, gate per call instead of always (ADR 0046): the Rust engine opens the
   * gate when an argument is absent, not a number, or above its threshold, and the grant is that
   * call (its arguments' fingerprint), never the tool for the rest of the run.
   */
  approvalWhen?: readonly ApprovalCondition[];
  /**
   * JSON Schema for the tool's input, advertised to the LLM. `inputSchema` only
   * validates (`.parse`); this is what the provider needs to emit tool calls.
   */
  jsonSchema?: Record<string, unknown>;
};

export type ToolHandler<TInput, TOutput> = (input: TInput) => Promise<TOutput>;

export interface ToolRegistry {
  register<TInput, TOutput>(
    definition: ToolDefinition<TInput, TOutput>,
    handler: ToolHandler<TInput, TOutput>
  ): void;
  resolve(
    id: ToolId
  ): { definition: ToolDefinition<unknown, unknown>; handler: ToolHandler<unknown, unknown> } | undefined;
  list(): ToolDefinition<unknown, unknown>[];
}

type Entry = {
  definition: ToolDefinition<unknown, unknown>;
  handler: ToolHandler<unknown, unknown>;
};

export class InMemoryToolRegistry implements ToolRegistry {
  private readonly entries = new Map<ToolId, Entry>();

  public register<TInput, TOutput>(
    definition: ToolDefinition<TInput, TOutput>,
    handler: ToolHandler<TInput, TOutput>
  ): void {
    this.entries.set(definition.id, {
      definition: definition as ToolDefinition<unknown, unknown>,
      handler: handler as ToolHandler<unknown, unknown>
    });
  }

  public resolve(
    id: ToolId
  ): { definition: ToolDefinition<unknown, unknown>; handler: ToolHandler<unknown, unknown> } | undefined {
    return this.entries.get(id);
  }

  public list(): ToolDefinition<unknown, unknown>[] {
    return [...this.entries.values()].map((entry) => entry.definition);
  }
}
