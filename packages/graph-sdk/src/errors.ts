import type { GraphValidationError } from "@ailu/graph-core";

/**
 * Discriminated-union result type used across the SDK's "safe" entry points
 * (e.g. {@link GraphBuilder.safeCompile}). Mirrors Zod's `safeParse` ergonomics.
 */
export type Result<T, E> = { success: true; data: T } | { success: false; error: E };

/** The "teach" metadata every Ailu error carries (ADR: errors-that-teach) — a stable
 * machine-readable `code`, a one-line `hint` (the fix), and a deep `docUrl`. An AI agent or a
 * human reads these to self-correct without leaving the stack trace. */
export type ErrorTeach = { code: string; hint?: string; docUrl?: string };

/** Where the error-code catalog lives; each error deep-links to its own anchor. */
const ERROR_DOC_BASE =
  "https://github.com/AI-Adriane/ailu-core/blob/main/docs-site/docs/reference/errors.md";
// GitHub heading anchors lowercase the text and keep underscores, so `ADR_FOO` → `#adr_foo`.
const docFor = (code: string): string => `${ERROR_DOC_BASE}#${code.toLowerCase()}`;

/** Base class for every error thrown by `@ailu/graph-sdk`. Carries a stable `code`,
 * an actionable `hint`, and a `docUrl` — the `.message` stays exactly what was thrown
 * (so existing assertions hold); the teaching lives in the extra fields + {@link format}. */
export class AiluSdkError extends Error {
  public readonly code: string;
  public readonly hint?: string;
  public readonly docUrl: string;

  public constructor(message: string, teach: ErrorTeach) {
    super(message);
    this.name = "AiluSdkError";
    this.code = teach.code;
    this.hint = teach.hint;
    this.docUrl = teach.docUrl ?? docFor(teach.code);
  }

  /** Message + hint + doc link, for logs / CLI / an agent reading the failure to self-correct. */
  public format(): string {
    return [this.message, this.hint ? `→ ${this.hint}` : undefined, `   docs: ${this.docUrl}`]
      .filter((line): line is string => line !== undefined)
      .join("\n");
  }
}

/** Thrown when `.compile()` is called on a graph that fails validation. */
export class GraphCompileError extends AiluSdkError {
  public readonly errors: GraphValidationError[];

  public constructor(errors: GraphValidationError[]) {
    const summary = errors.map((error) => `${error.code}: ${error.message}`).join("; ");
    super(`Graph failed to compile: ${summary}`, {
      code: "ADR_GRAPH_COMPILE",
      hint: "Fix the validation errors above — each carries its own code (e.g. a dangling edge, a missing entry node, an unknown channel)."
    });
    this.name = "GraphCompileError";
    this.errors = errors;
  }
}

/** Thrown when two nodes are added under the same id. */
export class DuplicateNodeError extends AiluSdkError {
  public constructor(nodeId: string) {
    super(`A node with id '${nodeId}' was already added to this graph.`, {
      code: "ADR_DUPLICATE_NODE",
      hint: `Give the node a unique id, or remove the earlier '.node("${nodeId}", …)'.`
    });
    this.name = "DuplicateNodeError";
  }
}

/** Thrown when an action node is added without an executable handler. */
export class MissingHandlerError extends AiluSdkError {
  public constructor(nodeId: string) {
    super(`Node '${nodeId}' is an action node but no handler was provided.`, {
      code: "ADR_MISSING_HANDLER",
      hint: `Pass a handler — '.node("${nodeId}", async () => ({ … }))' — or use '.agentNode'/a component node instead.`
    });
    this.name = "MissingHandlerError";
  }
}

/** Thrown when a builder method references a node id that has not been added yet. */
export class UnknownNodeError extends AiluSdkError {
  public constructor(nodeId: string, context: string) {
    super(`${context} references node '${nodeId}', which has not been added to this graph.`, {
      code: "ADR_UNKNOWN_NODE",
      hint: `Add the node with '.node("${nodeId}", …)' (or '.agentNode'/'.humanGate') BEFORE referencing it in an edge or condition.`
    });
    this.name = "UnknownNodeError";
  }
}

/**
 * Thrown when an agent's `middleware[]` names a GOVERNANCE kind (redact / approvalGate /
 * fsPolicy). The governed layer is engine-injected and sealed — a user may only append
 * EFFICIENCY middleware (compress / terse / contextBudget) — so an ungoverned stack is
 * unrepresentable (ADR 0025 phase 3d, the governed-by-construction invariant).
 */
export class GovernanceMiddlewareRejectedError extends AiluSdkError {
  public constructor(kind: string) {
    super(
      `Middleware kind '${kind}' is a governance middleware: it is engine-injected and cannot ` +
        `be supplied by a user. Agent middleware may only be efficiency kinds (compress, terse, contextBudget).`,
      {
        code: "ADR_GOVERNANCE_MIDDLEWARE_REJECTED",
        hint: "Remove the governance kind from middleware[]; governance (redact/approvalGate/fsPolicy) is sealed and applied by the engine. Only compress/terse/contextBudget are user-supplied."
      }
    );
    this.name = "GovernanceMiddlewareRejectedError";
  }
}

/** Thrown when `resume`/`approve` is called but no suspended state exists for that run id on
 * this `CompiledGraph` instance (the Rust path keeps suspended state per-instance). */
export class ResumeStateNotFoundError extends AiluSdkError {
  public constructor(runId: string) {
    super(
      `No suspended state for run '${runId}'. On the Rust engine, resume/approve must follow a ` +
        "suspended run on the same CompiledGraph instance.",
      {
        code: "ADR_NO_SUSPENDED_STATE",
        hint: "Call resume/approve on the SAME CompiledGraph that returned the suspended run, before the process restarts — or rehydrate the run from a persisted checkpoint (control plane)."
      }
    );
    this.name = "ResumeStateNotFoundError";
  }
}
