import assert from "node:assert/strict";

// #region example
import {
  resumeCatalogGraph,
  runCatalogGraph,
  type GraphDefinition,
  type HostNodeInput
} from "@ailu-ai/graph-sdk";

// A graph saved as data: a person approves, then `send` acts.
const definition = JSON.parse(`{
  "id": "send-after-approval", "version": "1", "name": "send-after-approval",
  "channels": {
    "message": { "type": "string", "reducer": "replace" },
    "receipt": { "type": "string", "reducer": "replace" }
  },
  "nodes": [
    { "id": "approve", "type": "human-gate", "label": "approve" },
    { "id": "send", "type": "action", "label": "send" }
  ],
  "edges": [{ "id": "e1", "from": "approve", "to": "send", "type": "default" }],
  "entryNodeId": "approve"
}`) as GraphDefinition;

// Stands in for your messaging API.
const postMessage = async (text: string): Promise<string> => `msg-${text.length}`;

// Your code for `send`. A retry of this step from the same checkpoint gets the same effect key:
// send at most once per key.
const receipts = new Map<string, string>();
const send = {
  id: "send",
  execute: async ({ channels, effectKey }: HostNodeInput) => {
    const receipt = receipts.get(effectKey) ?? (await postMessage(String(channels.message)));
    receipts.set(effectKey, receipt);
    return { receipt };
  }
};

const paused = await runCatalogGraph(definition, {
  initialData: { message: "Invoice 42 is paid." },
  nodes: [send]
});
console.log(paused.status); // "suspended": nothing sent yet

// Once a person approves: resume with the same binding — it is code, not state.
const done = await resumeCatalogGraph(definition, paused.state, { nodes: [send] });
console.log(done.state.channels.receipt); // "msg-19"
// #endregion example

assert.equal(paused.status, "suspended");
assert.equal(done.status, "completed");
assert.equal(done.state.channels.receipt, "msg-19");
assert.equal(receipts.size, 1);
