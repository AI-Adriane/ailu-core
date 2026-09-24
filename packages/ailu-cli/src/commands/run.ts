import { readFile } from "node:fs/promises";
import { watch as fsWatch } from "node:fs";

import {
  GraphRuntime,
  InMemoryCheckpointer,
  InMemoryConditionRegistry,
  InMemoryEventBus,
  InMemoryNodeRegistry
} from "../../../graph-runtime/src/index.js";
import { compileGraphFile } from "../../../graph-ailu/src/compiler/compile-graph-file.js";

const runOnce = async (file: string, input: Record<string, unknown>): Promise<void> => {
  const content = await readFile(file, "utf8");
  const compiled = compileGraphFile(content, file);
  if (compiled.result === undefined) {
    for (const diagnostic of compiled.diagnostics) {
      process.stderr.write(`${diagnostic.code}: ${diagnostic.message}\n`);
    }
    return;
  }
  const graph = compiled.result;
  const registry = new InMemoryNodeRegistry();
  for (const node of graph.nodes) {
    registry.register(node.id, async () => ({}));
  }
  const runtime = new GraphRuntime({
    graph,
    nodeRegistry: registry,
    conditionRegistry: new InMemoryConditionRegistry(),
    checkpointer: new InMemoryCheckpointer(),
    eventBus: new InMemoryEventBus()
  });
  for await (const event of runtime.stream(`run:${Date.now()}` as never, input, "debug")) {
    process.stdout.write(`${JSON.stringify(event)}\n`);
  }
};

export const runCommand = async (file: string, inputJson: string | undefined, watch: boolean): Promise<number> => {
  const input =
    inputJson === undefined || inputJson.trim().length === 0
      ? {}
      : ((JSON.parse(inputJson) as Record<string, unknown>) ?? {});

  await runOnce(file, input);
  if (!watch) {
    return 0;
  }
  fsWatch(file, async () => {
    await runOnce(file, input);
  });
  return 0;
};
