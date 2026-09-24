import assert from "node:assert/strict";

// #region example
import { docQaReferenceDefinition, replayCatalogGraph, runCatalogGraph } from "@ailu-ai/graph-sdk";

const definition = docQaReferenceDefinition();

// 1. Record: with AILU_LLM_RECORD=1 the engine journals every model call and timestamp.
process.env.AILU_LLM_RECORD = "1";
const recorded = await runCatalogGraph(definition, {
  initialData: { question: "How does Ailu resume a run?", documents: "Ailu checkpoints after every node." }
});
delete process.env.AILU_LLM_RECORD;
// Store these two with the run: they are the evidence.
const { entryState, replayJournal } = recorded;

// 2. Replay, later and elsewhere: the run is re-derived from its entry state
//    using the recorded model outputs. No model is called.
const replayed = await replayCatalogGraph(definition, entryState!, "audit-1", replayJournal!);
console.log(JSON.stringify(replayed.state.channels.answer) === JSON.stringify(recorded.state.channels.answer)); // true
// #endregion example

assert.equal(recorded.status, "completed");
assert.deepEqual(replayed.state.channels.answer, recorded.state.channels.answer);
