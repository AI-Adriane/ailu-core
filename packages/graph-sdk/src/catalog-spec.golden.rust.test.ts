import { readFileSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";

import { afterEach, describe, expect, it, vi } from "vitest";

import {
  council,
  createGraph,
  exampleGraphs,
  HostNodeBindingError,
  InMemoryToolRegistry,
  model,
  runCatalogGraph,
  rustEngineAvailable,
  type GraphDefinition
} from "./index.js";
import type { ToolId } from "@ailu-ai/agents-core";

/**
 * ADR 0045 D3.2 — golden cases for the catalog spec. Each case is a catalog definition plus the
 * host's bindings; its `expected` is the `EngineSpec` the TypeScript SDK built for it (what
 * `runCatalogGraph` hands the engine, without the per-call `runId` / `initialData` / `inbox` /
 * `streamTokens`), the warnings it printed, or the error it threw. The engine's
 * `spec_from_catalog` (`engineSpecFromCatalog`) must give the same for every case; the same file
 * is checked by the Rust tests of `crates/runtime-bridge`.
 *
 * `AILU_UPDATE_CATALOG_GOLDEN=1` rewrites the file from the TypeScript SDK.
 */
const FIXTURE = fileURLToPath(
  new URL("../../../crates/runtime-bridge/tests/fixtures/catalog_spec_golden.json", import.meta.url)
);

type CatalogInput = {
  graph: unknown;
  subgraphs?: unknown[];
  hostNodes?: string[];
  hostTools?: string[];
  providerKeys?: Record<string, string>;
  fsPolicy?: unknown[];
  skills?: unknown[];
};

/** What the SDK refuses: a binding (with its node and reason), or a carrier or graph it threw on. */
type GoldenError =
  | { kind: "hostNodeBinding"; nodeId: string; reason: string }
  | { kind: "invalidCarrier"; nodeId: string }
  | { kind: "invalidDefinition" };

type GoldenExpected =
  { spec: Record<string, unknown>; warnings: string[] } | { error: GoldenError };

type GoldenCase = { name: string; input: CatalogInput; expected: GoldenExpected };

/** A case to record; a TypeScript `TypeError` is recorded as the error the case declares. */
type CaseSource = { name: string; input: CatalogInput; throws?: GoldenError };

const graph = (
  id: string,
  nodes: unknown[],
  extra: Record<string, unknown> = {}
): GraphDefinition =>
  ({
    id,
    version: "1",
    name: id,
    channels: {},
    nodes,
    edges: [],
    entryNodeId: (nodes[0] as { id?: string } | undefined)?.id ?? "a",
    ...extra
  }) as unknown as GraphDefinition;

const action = (id: string, metadata?: unknown, type = "action"): Record<string, unknown> =>
  metadata === undefined ? { id, type, label: id } : { id, type, label: id, metadata };

const passthrough = { parse: (value: unknown) => value };

const gatedTools = (): InMemoryToolRegistry => {
  const tools = new InMemoryToolRegistry();
  tools.register(
    {
      id: "refund" as ToolId,
      name: "refund",
      description: "Issues a refund.",
      inputSchema: passthrough,
      outputSchema: passthrough,
      permissions: [],
      requiresApproval: true,
      jsonSchema: { type: "object", properties: { amount: { type: "number" } } }
    },
    async () => ({ ok: true })
  );
  tools.register(
    {
      id: "lookup" as ToolId,
      name: "lookup",
      description: "Looks an order up.",
      inputSchema: passthrough,
      outputSchema: passthrough,
      permissions: []
    },
    async () => ({ found: true })
  );
  return tools;
};

/** Every case, as JSON (what crosses to the engine). */
const caseSources = (): CaseSource[] => {
  const builderAgents = createGraph({ name: "builder-agents" })
    .channel("q", { type: "string", default: "" })
    .agentNode("everything", {
      prompt: { system: "You answer." },
      model: "openai:gpt-4o",
      tools: gatedTools(),
      maxIterations: 4,
      outputChannel: "out",
      outputStyle: "terse",
      contextBudget: 4000,
      todosChannel: "__todos",
      inputBlocksChannel: "blocks",
      visibleChannels: ["q"],
      memory: { namespace: "acme", topK: 3, recall: "vector" },
      skills: { namespace: "acme", required: ["triage@1"], advisoryK: 2 },
      suspendForApproval: true,
      middleware: [{ kind: "compress" }, { kind: "reflection", params: { threshold: 0.7 } }]
    })
    .agentNode("profiled", { prompt: { system: "Deep." }, profile: "governed-deep" })
    .agentNode("tierOnly", {
      prompt: { system: "Fast." },
      model: model.fast,
      outputChannel: "fast"
    })
    .agentNode("local", {
      prompt: { system: "Local." },
      model: model.openaiCompatible({
        baseURL: "http://localhost:1234/v1",
        model: "qwen2.5",
        apiKeyEnv: "LOCAL_KEY"
      }),
      outputChannel: "local"
    })
    .agentNode("searcher", {
      prompt: { system: "Search." },
      model: "mistral:mistral-large-latest",
      webSearch: { maxUses: 2, allowedDomains: ["example.com"] },
      outputChannel: "searched"
    })
    .edge("everything", "profiled")
    .edge("profiled", "tierOnly")
    .edge("tierOnly", "local")
    .edge("local", "searcher")
    .compile().definition;

  const builderFanOut = createGraph({ name: "builder-fan-out" })
    .channel("items", { type: "json", default: [] })
    .mapAgents("fan", {
      overChannel: "items",
      joinAt: "summaries",
      subAgent: { prompt: { system: "Summarize the item." }, tools: gatedTools() },
      suspendForApproval: true
    })
    .node("after", async () => ({}))
    .edge("fan", "after")
    .compile().definition;

  const taskChild = createGraph({ name: "dig-task", id: "dig-task" })
    .channel("objective", { type: "string", default: "" })
    .agentNode("dig__agent", {
      prompt: { system: "Research." },
      outputChannel: "report",
      outputStyle: "terse"
    })
    .compile().definition;

  const gatedChild = graph("sub-gated", [
    action("c_draft"),
    action("c_gate", undefined, "human-gate"),
    action("c_assistant", {
      agent: {
        provider: "anthropic",
        toolNames: ["refund"],
        suspendForApproval: true,
        approvalToolNames: ["refund"]
      }
    }),
    action("c_clean", { component: { kind: "textCleaner", params: { from: "a", into: "b" } } }),
    action("c_publish", undefined, "tool")
  ]);
  const nestedChild = graph("sub-nested", [
    action("n_step"),
    { id: "n_sub", type: "subgraph", label: "n_sub", subgraphId: "sub-gated" }
  ]);
  const parentWith = (subgraphId: string, extra: Record<string, unknown> = {}): GraphDefinition =>
    graph("parent", [
      action("p_first"),
      { id: "sub", type: "subgraph", label: "sub", subgraphId, ...extra }
    ]);

  const examples = exampleGraphs().map((example): CaseSource => ({
    name: `example ${example.slug}`,
    input: { graph: example.definition }
  }));

  return [
    ...examples,
    {
      name: "example publish-flow with its plain steps bound",
      input: { graph: exampleGraphs()[0]!.definition, hostNodes: ["publish", "write"] }
    },
    {
      name: "council with a gate",
      input: {
        graph: council({
          members: [
            { prompt: { system: "Member A." } },
            { prompt: { system: "Member B." }, model: "mistral:fast" }
          ],
          chair: { prompt: { system: "Chair." }, model: model.frontier },
          humanGate: true
        })
      }
    },
    { name: "builder agents with every option", input: { graph: builderAgents } },
    {
      name: "builder mapAgents fan-out, a plain step bound",
      input: { graph: builderFanOut, hostNodes: ["after"] }
    },
    {
      name: "task node: a subgraph whose child is an agent",
      input: {
        graph: createGraph({ name: "research" })
          .channel("objective", { type: "string", default: "" })
          .node("prepare", async () => ({}))
          .compile().definition,
        subgraphs: [taskChild]
      }
    },
    {
      name: "subgraph: child agent, component, gate and plain steps, a child step bound",
      input: {
        graph: parentWith("sub-gated"),
        subgraphs: [gatedChild],
        hostNodes: ["c_publish", "p_first"]
      }
    },
    {
      name: "subgraph: mapSubgraph fan-out",
      input: {
        graph: parentWith("sub-gated", {
          mapSubgraph: { overChannel: "items", joinAt: "results" }
        }),
        subgraphs: [gatedChild]
      }
    },
    {
      name: "subgraph: nested child, both children given",
      input: { graph: parentWith("sub-nested"), subgraphs: [nestedChild, gatedChild] }
    },
    {
      name: "subgraph: a parent and a child node share an id",
      input: {
        graph: graph("parent", [
          action("x"),
          { id: "sub", type: "subgraph", label: "sub", subgraphId: "dup" }
        ]),
        subgraphs: [
          graph("dup", [
            action("x", { agent: { model: "child-model" } }),
            action("y", { component: { kind: "textCleaner", params: {} } })
          ]),
          graph("dup2", [action("y", { agent: {} }), action("sub")])
        ]
      }
    },
    {
      name: "carrier: an empty agent carrier gets every default",
      input: { graph: graph("g", [action("a", { agent: {} }, "agent")]) }
    },
    {
      name: "carrier: null agent fields",
      input: {
        graph: graph("g", [
          action("a", {
            agent: {
              provider: null,
              model: null,
              tier: null,
              system: null,
              toolNames: null,
              toolSpecs: null,
              approvalToolNames: null,
              outputChannel: null,
              suspendForApproval: null,
              maxIterations: null,
              visibleChannels: null,
              webSearch: null,
              memory: null,
              skills: null
            }
          })
        ])
      }
    },
    {
      name: "carrier: every agent field, unknown fields dropped",
      input: {
        graph: graph("g", [
          action(
            "a",
            {
              agent: {
                provider: "openai",
                model: "gpt-4o",
                tier: "balanced",
                baseURL: "http://localhost:8000/v1",
                apiKeyEnv: "LOCAL_KEY",
                system: "Be brief.",
                toolNames: ["refund", "lookup"],
                toolSpecs: [
                  {
                    name: "refund",
                    description: "Refunds.",
                    jsonSchema: { type: "object" },
                    extra: true
                  },
                  { name: "lookup" },
                  { name: "nulls", description: null, jsonSchema: null },
                  "not-an-object",
                  42,
                  ["also", "not"]
                ],
                maxIterations: 7,
                suspendForApproval: true,
                approvalToolNames: ["refund"],
                outputChannel: "answer",
                outputStyle: "terse",
                contextBudget: 1200,
                todosChannel: "__todos",
                inputBlocksChannel: "blocks",
                visibleChannels: ["q", "answer"],
                webSearch: { maxUses: 1, blockedDomains: ["bad.example"] },
                memory: { namespace: "m", topK: 5, recall: "both" },
                skills: { namespace: "s", required: [], advisoryK: 1 },
                enableFs: true,
                resolvedMiddleware: [
                  { kind: "terse" },
                  { kind: "contextBudget", params: { chars: 900 } }
                ],
                mcpConnectionId: "conn-1",
                actionConnections: { slack: "conn-2" },
                usesApprovalEngine: true,
                toolBindings: [{ name: "refund" }],
                somethingElse: { nested: [1, 2.5, "x"] }
              }
            },
            "agent"
          )
        ])
      }
    },
    {
      name: "carrier: suspendForApproval only when true, empty provider kept",
      input: {
        graph: graph("g", [
          action("a", { agent: { suspendForApproval: "true", provider: "" } }),
          action("b", { agent: { suspendForApproval: 1 } }),
          action("c", { agent: { suspendForApproval: false } }),
          action("d", { agent: { suspendForApproval: true } })
        ])
      }
    },
    {
      name: "carrier: fields of the wrong type pass through to the engine",
      input: {
        graph: graph("g", [
          action("a", {
            agent: {
              provider: 42,
              toolNames: "refund",
              approvalToolNames: { refund: true },
              outputChannel: 7,
              maxIterations: "3",
              enableFs: null,
              resolvedMiddleware: null
            }
          })
        ])
      }
    },
    {
      name: "carrier: component params that are not an object read as {}",
      input: {
        graph: graph("g", [
          action("arr", { component: { kind: "textCleaner", params: [1, 2] } }),
          action("str", { component: { kind: "textCleaner", params: "x" } }),
          action("nul", { component: { kind: "textCleaner", params: null } }),
          action("none", { component: { kind: "textCleaner" } }),
          action("ok", {
            component: {
              kind: "promptBuilder",
              params: { template: "Hi {{name}}", into: "p", extra: [true] }
            }
          })
        ])
      }
    },
    {
      name: "carrier: a component without a usable kind falls through",
      input: {
        graph: graph("g", [
          action("empty", { component: { kind: "", params: {} }, agent: { model: "m" } }),
          action("number", { component: { kind: 5, params: {} } }),
          action("missing", { component: { params: {} } }),
          action("string", { component: "textCleaner" }),
          action("array", { component: [{ kind: "textCleaner" }] }),
          action("null", { component: null })
        ])
      }
    },
    {
      name: "carrier: precedence — component, then mapAgents, then agent",
      input: {
        graph: graph("g", [
          action("c", {
            component: { kind: "textCleaner", params: {} },
            mapAgents: { overChannel: "i", joinAt: "o", subAgent: {} },
            agent: {}
          }),
          action("m", {
            mapAgents: { overChannel: "i", joinAt: "o", subAgent: { model: "s" } },
            agent: { model: "a" }
          }),
          action("bad-m", { mapAgents: { overChannel: "i", joinAt: "o" }, agent: { model: "a" } })
        ])
      }
    },
    {
      name: "carrier: malformed mapAgents carriers warn and fall through",
      input: {
        graph: graph("g", [
          action("no-over", { mapAgents: { joinAt: "o", subAgent: {} } }),
          action("empty-join", { mapAgents: { overChannel: "i", joinAt: "", subAgent: {} } }),
          action("array-sub", { mapAgents: { overChannel: "i", joinAt: "o", subAgent: [] } }),
          action("number-over", { mapAgents: { overChannel: 1, joinAt: "o", subAgent: {} } }),
          action("gate-with-bad-map", { mapAgents: {} }, "human-gate"),
          action("string-map", { mapAgents: "fan" }),
          action("array-map", { mapAgents: [] }),
          action("null-map", { mapAgents: null })
        ])
      }
    },
    {
      name: "carrier: mapAgents suspendForApproval only when true, sub-agent defaults",
      input: {
        graph: graph("g", [
          action("t", {
            mapAgents: { overChannel: "i", joinAt: "o", subAgent: {}, suspendForApproval: true }
          }),
          action("f", {
            mapAgents: {
              overChannel: "i",
              joinAt: "o",
              subAgent: { provider: null },
              suspendForApproval: "yes"
            }
          })
        ])
      }
    },
    {
      name: "carrier: metadata or agent that is not an object",
      input: {
        graph: graph("g", [
          action("s", "agent"),
          action("arr", [{ agent: {} }]),
          action("n", 3),
          action("nul", null),
          action("agent-string", { agent: "anthropic" }),
          action("agent-array", { agent: [{}] }),
          action("agent-null", { agent: null }),
          action("agent-number", { agent: 1 })
        ])
      }
    },
    {
      name: "carrier: gates and subgraph nodes with a carrier take it",
      input: {
        graph: graph("g", [
          action("gate", { agent: { model: "m" } }, "human-gate"),
          {
            id: "sub",
            type: "subgraph",
            label: "sub",
            subgraphId: "x",
            metadata: { component: { kind: "textCleaner" } }
          },
          action("plain-gate", undefined, "human-gate"),
          { id: "plain-sub", type: "subgraph", label: "plain-sub", subgraphId: "x" },
          action("agent-type-without-carrier", undefined, "agent"),
          action("tool-type", undefined, "tool")
        ])
      }
    },
    {
      name: "bindings: host tools are deduplicated in order",
      input: {
        graph: graph("g", [action("a"), action("b")]),
        hostNodes: ["b"],
        hostTools: ["send", "lookup", "send"]
      }
    },
    {
      name: "settings: provider keys, fs policy and skills pass through",
      input: {
        graph: graph("g", [action("a", { agent: { enableFs: true } })]),
        providerKeys: { openai: "sk-test", mistral: "mk" },
        fsPolicy: [
          { glob: "scratch/**", verb: "write" },
          { glob: "secret/**", verb: "deny" }
        ],
        skills: [
          {
            name: "triage",
            version: "1",
            namespace: "acme",
            body: "Triage first.",
            description: "d"
          }
        ]
      }
    },
    {
      name: "bindings: a node bound twice",
      input: { graph: graph("g", [action("a"), action("b")]), hostNodes: ["a", "b", "a"] }
    },
    {
      name: "bindings: an agent node",
      input: {
        graph: graph("g", [action("a"), action("agent", { agent: {} })]),
        hostNodes: ["agent"]
      }
    },
    {
      name: "bindings: a component node",
      input: {
        graph: graph("g", [action("c", { component: { kind: "textCleaner", params: {} } })]),
        hostNodes: ["c"]
      }
    },
    {
      name: "bindings: a mapAgents node",
      input: {
        graph: graph("g", [
          action("m", { mapAgents: { overChannel: "i", joinAt: "o", subAgent: {} } })
        ]),
        hostNodes: ["m"]
      }
    },
    {
      name: "bindings: a human gate",
      input: { graph: graph("g", [action("gate", undefined, "human-gate")]), hostNodes: ["gate"] }
    },
    {
      name: "bindings: a subgraph node",
      input: { graph: parentWith("sub-gated"), subgraphs: [gatedChild], hostNodes: ["sub"] }
    },
    {
      name: "bindings: an id no graph has",
      input: { graph: graph("g", [action("a")]), subgraphs: [gatedChild], hostNodes: ["a", "nope"] }
    },
    {
      name: "bindings: a binding error comes before a carrier error",
      input: {
        graph: graph("g", [action("a", { agent: { toolSpecs: "refund" } })]),
        hostNodes: ["missing"]
      }
    },
    {
      name: "carrier: toolSpecs that is not a list",
      input: {
        graph: graph("g", [
          action("ok", { agent: {} }),
          action("a", { agent: { toolSpecs: { name: "x" } } })
        ])
      },
      throws: { kind: "invalidCarrier", nodeId: "a" }
    },
    {
      name: "carrier: toolSpecs false",
      input: { graph: graph("g", [action("a", { agent: { toolSpecs: false } })]) },
      throws: { kind: "invalidCarrier", nodeId: "a" }
    },
    {
      name: "carrier: a null toolSpecs entry",
      input: { graph: graph("g", [action("a", { agent: { toolSpecs: [{ name: "x" }, null] } })]) },
      throws: { kind: "invalidCarrier", nodeId: "a" }
    },
    {
      name: "carrier: a mapAgents sub-agent toolSpecs that is not a list",
      input: {
        graph: graph("g", [
          action("m", {
            mapAgents: { overChannel: "i", joinAt: "o", subAgent: { toolSpecs: "x" } }
          })
        ])
      },
      throws: { kind: "invalidCarrier", nodeId: "m" }
    },
    {
      name: "graph: no node list",
      input: {
        graph: { id: "g", version: "1", name: "g", channels: {}, edges: [], entryNodeId: "a" }
      },
      throws: { kind: "invalidDefinition" }
    }
  ];
};

/** JSON round trip: what the case looks like once it crossed to the engine. */
const asJson = <T>(value: T): T => JSON.parse(JSON.stringify(value)) as T;

const PER_CALL_FIELDS = [
  "runId",
  "initialData",
  "inbox",
  "streamTokens",
  "state",
  "approvedTools",
  "replayJournal"
];

/** The spec the engine received, as `spec_from_catalog` returns it: no per-call fields, `hostNodeIds`. */
const staticSpec = (specJson: string): Record<string, unknown> => {
  const spec = JSON.parse(specJson) as Record<string, unknown>;
  for (const field of PER_CALL_FIELDS) delete spec[field];
  if ("jsNodeIds" in spec) {
    spec.hostNodeIds = spec.jsNodeIds;
    delete spec.jsNodeIds;
  }
  return spec;
};

const napi = (): {
  engineRun: (...args: unknown[]) => Promise<string>;
  engineSpecFromCatalog: (inputJson: string) => string;
} => createRequire(import.meta.url)("@ailu-ai/napi");

const completed = JSON.stringify({
  state: {
    runId: "run",
    graphId: "g",
    currentNodeId: "a",
    status: "completed",
    channels: {},
    version: 1,
    createdAt: "0",
    updatedAt: "0"
  },
  status: "completed",
  pendingApprovals: []
});

/**
 * What the SDK does with a case: run it through `runCatalogGraph` with the engine's `engineRun`
 * stubbed, and keep the spec it was handed, the warnings printed, or the error thrown.
 */
const sdkResult = async (input: CatalogInput, declared?: GoldenError): Promise<GoldenExpected> => {
  const warnings: string[] = [];
  let received: string | undefined;
  const warn = vi.spyOn(console, "warn").mockImplementation((message: unknown) => {
    warnings.push(String(message).replace(/^\[ailu\] /, ""));
  });
  const run = vi.spyOn(napi(), "engineRun").mockImplementation(async (specJson: unknown) => {
    received = specJson as string;
    return completed;
  });
  try {
    await runCatalogGraph(input.graph as GraphDefinition, {
      subgraphs: input.subgraphs as GraphDefinition[] | undefined,
      nodes: input.hostNodes?.map((id) => ({ id, execute: () => ({}) })),
      tools: input.hostTools?.map((name) => ({ name, execute: async () => ({}) })),
      providerKeys: input.providerKeys,
      fsPolicy: input.fsPolicy as never,
      skills: input.skills as never
    });
  } catch (error) {
    if (error instanceof HostNodeBindingError) {
      const reason = error.message.slice(`Host node '${error.nodeId}' can't be bound: `.length, -1);
      return { error: { kind: "hostNodeBinding", nodeId: error.nodeId, reason } };
    }
    if (error instanceof TypeError && declared !== undefined) {
      return { error: declared };
    }
    throw error;
  } finally {
    warn.mockRestore();
    run.mockRestore();
  }
  if (received === undefined) throw new Error("the engine was never called");
  return { spec: staticSpec(received), warnings };
};

/** What the engine's `spec_from_catalog` gives for a case, in the golden shape. */
const engineResult = (input: CatalogInput): GoldenExpected => {
  const out = JSON.parse(napi().engineSpecFromCatalog(JSON.stringify(input))) as
    | { spec: Record<string, unknown>; warnings: string[] }
    | { error: { kind: string; nodeId?: string; reason?: string } };
  if ("error" in out) {
    const { kind, nodeId, reason } = out.error;
    return {
      error: (kind === "invalidDefinition"
        ? { kind }
        : reason === undefined
          ? { kind, nodeId }
          : { kind, nodeId, reason }) as GoldenError
    };
  }
  return out;
};

const readGolden = (): GoldenCase[] => JSON.parse(readFileSync(FIXTURE, "utf8")) as GoldenCase[];

const native = rustEngineAvailable() ? describe : describe.skip;

native("@ailu-ai/graph-sdk — catalog spec golden cases (ADR 0045 D3.2)", () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  if (process.env.AILU_UPDATE_CATALOG_GOLDEN === "1") {
    it("rewrites the golden file from the TypeScript SDK", async () => {
      const cases: GoldenCase[] = [];
      for (const source of caseSources()) {
        const input = asJson(source.input);
        cases.push({ name: source.name, input, expected: await sdkResult(input, source.throws) });
      }
      writeFileSync(FIXTURE, `${JSON.stringify(cases, null, 2)}\n`);
    });
    return;
  }

  it("covers every case the file lists", () => {
    expect(readGolden().map((golden) => golden.name)).toEqual(
      caseSources().map((source) => source.name)
    );
  });

  it("the SDK builds the golden spec for every case", async () => {
    for (const golden of readGolden()) {
      const declared = "error" in golden.expected ? golden.expected.error : undefined;
      expect({ name: golden.name, ...(await sdkResult(golden.input, declared)) }).toEqual({
        name: golden.name,
        ...golden.expected
      });
    }
  });

  it("the engine's spec_from_catalog builds the golden spec for every case", () => {
    for (const golden of readGolden()) {
      expect({ name: golden.name, ...engineResult(golden.input) }).toEqual({
        name: golden.name,
        ...golden.expected
      });
    }
  });
});
