import assert from "node:assert/strict";

// #region example
import { Ed25519Attestor, InMemoryApprovalEngine, verifyChain, type NodeId, type RunId } from "@ailu-ai/graph-sdk";

const approvals = new InMemoryApprovalEngine();
// Keep the private key in your secret store; publish the public key to your auditors.
const attestor = new Ed25519Attestor();

// An agent asked to refund; a named human approved it.
const request = await approvals.request({
  runId: "run-42" as RunId,
  nodeId: "assistant" as NodeId,
  requestedBy: "assistant",
  subject: { description: "refund order ORD-8830" }
});
const approved = await approvals.approve(request.id, "alice@example.com");

// Sign each decision, chained to the previous one.
const first = attestor.attest(approved, null);
const records = [first];

// Anyone holding the records can check them. Also check that record.publicKey is your key.
console.log(verifyChain(records)); // true
// #endregion example

assert.equal(verifyChain(records), true);
assert.equal(first.resolvedBy, "alice@example.com");
const tampered = [{ ...first, resolvedBy: "mallory" }];
assert.equal(verifyChain(tampered), false);
