# ADR 0056 — An evaluator says why it did not score

- Status: Proposed, with the evaluator status change (see the CHANGELOG).
- Context: the `evaluator` component read a missing channel as an empty string. Two missing inputs scored `1.0` under every metric, an array or an object was flattened to JSON text and scored, and nothing said why a score was what it was. A missing or unusable input could therefore pass.

## Decision

The component classifies each input before scoring. An absent or `null` channel is missing. A string, a number, a boolean or an `AgentResult` (its final answer) is text; a non-finite number has no JSON form and is read as `null`, so as missing. An array or any other object is `invalidType`. A channel holding `""` is present and empty, so two deliberately empty values are `scored` with `1.0`; with `emptyIsMissing` (default `false`) a text that is empty after trimming is missing instead.

The status vocabulary is `scored`, `missingExpected`, `missingActual`, `missingBoth` and `invalidType`, written to the optional `statusInto` channel, plus `error`. The Rust enum is `#[non_exhaustive]` and the TypeScript type is a string union that may grow. If one side is `invalidType` the status is `invalidType` whatever the other side. `error` means the evaluation could not run (for example a score outside `[0, 1]`): it fails the node and is never written to a channel.

For every status other than `scored` the score is `0` and the pass flag is `false` whatever the threshold. The outputs keep their types (a number and a boolean), so existing channels do not break; the status is a separate channel, not a new shape for the score.

`statusInto` must be non-blank and differ from `into`, `passInto`, `expectedFrom` and `actualFrom`, so that the status text cannot land in the numeric score channel or over an input.

## Consequences

- **Without `statusInto`, a score of `0` is not proof of an evaluation.** A consumer that reads a result without a status must treat it as "not qualified", never as `scored`. The same holds for results written before this change, for a replay of a run started on an earlier version, and for a resumed run whose earlier evaluations had no status: the engine cannot mark a report already written, so reading the absence of the status channel as "not qualified" is the consumer's obligation.
- Graphs that relied on two missing inputs scoring `1.0`, or on arrays and objects being compared as text, change result. That is the defect this removes; the CHANGELOG records it under "Changed (behaviour)".
- `tokenF1` and `overlap` tokenize on alphanumeric characters, so two texts with none (for example `"!!"` and `"??"`) are both empty and score `1.0`, while `exact` scores `0.0`. This is unchanged and documented.
- The TypeScript fallback mirrors the Rust component, including the detection of an `AgentResult` (an object with a string `reasoning`), the definition of white space used by `trim`, and the reading of non-finite numbers.
- Every SDK gets the behaviour, since it lives in the native component; none needs an API of its own.
