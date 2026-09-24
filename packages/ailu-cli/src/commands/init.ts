import { mkdir, writeFile } from "node:fs/promises";
import { dirname } from "node:path";

const graphTemplate = (id: string): string => `id: ${id}
version: 1.0.0
name: ${id}
entryNodeId: start
channels:
  state:
    type: object
    reducer: merge
    default: {}
nodes:
  - id: start
    type: action
    label: Start
edges: []
`;

const agentTemplate = (id: string): string => `id: ${id}
description: ${id} agent
prompt: ${id}.prompt
tools: []
`;

const promptTemplate = (id: string): string => `name: ${id}
template: "Hello {{name}}"
variables:
  - name
`;

export const initCommand = async (
  kind: "graph" | "agent" | "prompt",
  id: string,
  outFile: string
): Promise<number> => {
  const content =
    kind === "graph" ? graphTemplate(id) : kind === "agent" ? agentTemplate(id) : promptTemplate(id);
  await mkdir(dirname(outFile), { recursive: true });
  await writeFile(outFile, content, "utf8");
  process.stdout.write(`Initialized ${kind} template at ${outFile}\n`);
  return 0;
};
