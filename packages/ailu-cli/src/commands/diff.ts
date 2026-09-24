import { readFile } from "node:fs/promises";

import { compileGraphFile } from "../../../graph-ailu/src/compiler/compile-graph-file.js";

const parseInput = (value: string): { file: string; version: string } => {
  const [file, version] = value.split("@");
  return { file: file ?? "", version: version ?? "" };
};

export const diffCommand = async (left: string, right: string): Promise<number> => {
  const leftRef = parseInput(left);
  const rightRef = parseInput(right);
  const leftContent = await readFile(leftRef.file, "utf8");
  const rightContent = await readFile(rightRef.file, "utf8");
  const leftCompiled = compileGraphFile(leftContent, leftRef.file);
  const rightCompiled = compileGraphFile(rightContent, rightRef.file);
  if (leftCompiled.result === undefined || rightCompiled.result === undefined) {
    process.stderr.write("Unable to diff invalid graph files.\n");
    return 1;
  }
  const leftNodes = new Set(leftCompiled.result.nodes.map((n) => String(n.id)));
  const rightNodes = new Set(rightCompiled.result.nodes.map((n) => String(n.id)));
  const addedNodes = [...rightNodes].filter((id) => !leftNodes.has(id));
  const removedNodes = [...leftNodes].filter((id) => !rightNodes.has(id));

  const leftEdges = new Set(leftCompiled.result.edges.map((e) => String(e.id)));
  const rightEdges = new Set(rightCompiled.result.edges.map((e) => String(e.id)));
  const addedEdges = [...rightEdges].filter((id) => !leftEdges.has(id));
  const removedEdges = [...leftEdges].filter((id) => !rightEdges.has(id));

  const leftChannels = new Set(Object.keys(leftCompiled.result.channels));
  const rightChannels = new Set(Object.keys(rightCompiled.result.channels));
  const addedChannels = [...rightChannels].filter((id) => !leftChannels.has(id));
  const removedChannels = [...leftChannels].filter((id) => !rightChannels.has(id));

  process.stdout.write(
    [
      `Diff ${leftRef.version} -> ${rightRef.version}`,
      `+ nodes: ${addedNodes.join(", ") || "-"}`,
      `- nodes: ${removedNodes.join(", ") || "-"}`,
      `+ edges: ${addedEdges.join(", ") || "-"}`,
      `- edges: ${removedEdges.join(", ") || "-"}`,
      `+ channels: ${addedChannels.join(", ") || "-"}`,
      `- channels: ${removedChannels.join(", ") || "-"}`
    ].join("\n") + "\n"
  );
  return 0;
};
