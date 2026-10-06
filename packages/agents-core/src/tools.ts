export type ZodSchema<T> = {
  parse(input: unknown): T;
};

export type ToolId = string & { readonly __brand: "ToolId" };

/**
 * One condition on a gated tool's arguments: the call needs approval when the top-level input field
 * `argument` is a number above `above` (ADR 0046) or a string equal — byte for byte — to one of
 * `in` (ADR 0048), or is absent, or not of its test's type. Exactly one test per condition. Data,
 * never an expression.
 */
export type ApprovalCondition =
  { argument: string; above: number } | { argument: string; in: readonly string[] };

export type ToolDefinition<TInput, TOutput> = {
  id: ToolId;
  name: string;
  description: string;
  inputSchema: ZodSchema<TInput>;
  outputSchema: ZodSchema<TOutput>;
  permissions: string[];
  requiresApproval?: boolean;
  /**
   * With `requiresApproval`, gate per call instead of always (ADR 0046, 0048): the Rust engine
   * opens the gate when an argument is absent, not of its test's type, above its threshold or one
   * of its named values, and the grant is that call (its arguments' fingerprint), never the tool
   * for the rest of the run.
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
