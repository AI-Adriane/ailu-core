//! How an agent's seed message is cut down to a context budget (ADR 0014, ADR 0025 3d, ADR 0052).
//!
//! The ReAct loop builds the seed as `Input: <json>\nState: <json object>`. The `before_run`
//! middleware that run before the budget — memory recall, the governed brain, skills — each
//! **prepend** a block and a blank line, so the seed the budget sees reads:
//!
//! ```text
//! <skills block>\n\n<brain block>\n\n<memory block>\n\nInput: <json>\nState: {<entries>}
//! ```
//!
//! Engine 2.6 and earlier kept the first `chars` characters ([`BudgetTrim::HeadCut`]): with enough
//! prepended context, the `Input` / `State` part — the user's request — fell off the end, and the
//! agent answered without it. [`BudgetTrim::KeepRequest`] always keeps the request and spends what
//! is left of the budget on the rest, cutting the lowest priority first. Both are pure functions of
//! the seed and the budget: the same seed gives the same prompt, byte for byte.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// State channels that hold the user's request by convention and are never cut: the run input
/// (`input`) and the question of the SDK's RAG / answer agents (`question`).
pub const REQUEST_CHANNELS: &[&str] = &["input", "question"];

/// Marks a cut.
const MARK: &str = "…";
/// What separates a prepended block from the next one (and from `Input:`).
const BLOCK_SEPARATOR: &str = "\n\n";
const INPUT_PREFIX: &str = "Input: ";
const STATE_PREFIX: &str = "\nState: ";

/// How a seed over budget is cut. Recorded in the replay journal (ADR 0052): a journal recorded
/// before the field existed replays with [`BudgetTrim::HeadCut`], so its prompts stay identical.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BudgetTrim {
    /// Engine ≤ 2.6: keep the first `chars` characters, then `…`. Cuts the request when context
    /// was prepended. Kept only so that a run recorded with it replays byte-identically.
    HeadCut,
    /// Keep the request (the `Input:` line and the [`REQUEST_CHANNELS`] of `State`), then fill
    /// the budget in this order: the rest of `State` (ordinary channels, then reserved `__*`
    /// ones), and only if the whole `Input` / `State` fits, the prepended context — kept from its
    /// end, so skills (prepended last, read first) go before the brain and memory.
    KeepRequest,
}

/// Cut `content` to `chars` characters with `trim`. `None` when it already fits (left as is).
pub fn trim_seed(content: &str, chars: usize, trim: BudgetTrim) -> Option<String> {
    if content.chars().count() <= chars {
        return None;
    }
    Some(match trim {
        BudgetTrim::HeadCut => head_cut(content, chars),
        // A seed this module does not recognise (not built by the ReAct loop) keeps the head cut.
        BudgetTrim::KeepRequest => match Seed::parse(content) {
            Some(seed) => seed.keep_request(chars),
            None => head_cut(content, chars),
        },
    })
}

fn head_cut(content: &str, chars: usize) -> String {
    content.chars().take(chars).collect::<String>() + MARK
}

fn len(text: &str) -> usize {
    text.chars().count()
}

/// The last `count` characters of `text`.
fn tail(text: &str, count: usize) -> String {
    let skip = len(text).saturating_sub(count);
    text.chars().skip(skip).collect()
}

/// Fill order of a `State` entry: lower is kept first.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Rank {
    /// A [`REQUEST_CHANNELS`] entry: never cut.
    Request,
    Ordinary,
    /// A reserved `__*` channel (engine / control-plane plumbing, e.g. `__brainRecall`).
    Reserved,
}

struct Entry {
    /// `"key":value`, exactly as the seed renders it.
    text: String,
    rank: Rank,
}

/// A seed split into its prepended context, its `Input:` line and its `State` entries.
struct Seed<'a> {
    /// Everything before `Input:` minus the blank line that ends it; empty without context.
    context: &'a str,
    /// `Input: <json>` (compact JSON, so it holds no raw newline).
    input_line: &'a str,
    /// The `State` object's entries, in the seed's order.
    entries: Vec<Entry>,
}

impl<'a> Seed<'a> {
    /// Recognise a ReAct seed. The `Input` / `State` part is compact JSON, so it contains a single
    /// raw newline and never a blank line: the last `\n\nInput: ` is where the prepended context
    /// ends. `None` unless re-rendering the parsed `State` gives back its exact text — the kept
    /// entries are then verbatim.
    fn parse(content: &'a str) -> Option<Self> {
        let (context, core) = match content.rfind(&format!("{BLOCK_SEPARATOR}{INPUT_PREFIX}")) {
            Some(at) => (&content[..at], &content[at + BLOCK_SEPARATOR.len()..]),
            None if content.starts_with(INPUT_PREFIX) => ("", content),
            None => return None,
        };
        let newline = core.find('\n')?;
        let (input_line, rest) = core.split_at(newline);
        let state = rest.strip_prefix(STATE_PREFIX)?;
        let Ok(Value::Object(map)) = serde_json::from_str::<Value>(state) else {
            return None;
        };
        let entries: Vec<Entry> = map
            .iter()
            .map(|(key, value)| Entry {
                text: format!("{}:{value}", Value::String(key.clone())),
                rank: if REQUEST_CHANNELS.contains(&key.as_str()) {
                    Rank::Request
                } else if key.starts_with("__") {
                    Rank::Reserved
                } else {
                    Rank::Ordinary
                },
            })
            .collect();
        let rendered = format!(
            "{{{}}}",
            entries
                .iter()
                .map(|entry| entry.text.as_str())
                .collect::<Vec<_>>()
                .join(",")
        );
        (rendered == state).then_some(Seed {
            context,
            input_line,
            entries,
        })
    }

    fn core(&self, kept: &[Option<String>], trimmed: bool) -> String {
        let entries = kept.iter().flatten().cloned().collect::<Vec<_>>().join(",");
        let mark = if trimmed { MARK } else { "" };
        format!("{}{STATE_PREFIX}{{{entries}}}{mark}", self.input_line)
    }

    fn keep_request(&self, chars: usize) -> String {
        let all: Vec<Option<String>> = self
            .entries
            .iter()
            .map(|entry| Some(entry.text.clone()))
            .collect();
        let core = self.core(&all, false);
        let core_len = len(&core);

        // The whole `Input` / `State` fits: what remains goes to the end of the context.
        if core_len <= chars {
            let overhead = len(MARK) + len(BLOCK_SEPARATOR);
            let room = chars - core_len;
            if room < overhead {
                return core;
            }
            let context = tail(self.context, room - overhead);
            return format!("{MARK}{context}{BLOCK_SEPARATOR}{core}");
        }

        // It does not: the context goes, and `State` is filled by rank. The request entries are
        // kept whatever they cost; the first other entry that does not fit is cut, the rest go.
        let mut kept: Vec<Option<String>> = vec![None; self.entries.len()];
        let mut count = 0;
        // `Input: …\nState: {` + `}` + the closing mark.
        let mut used = len(self.input_line) + len(STATE_PREFIX) + 2 + len(MARK);
        let mut order: Vec<usize> = (0..self.entries.len()).collect();
        order.sort_by_key(|&index| self.entries[index].rank); // stable: seed order within a rank
        let mut open = true;
        for index in order {
            let entry = &self.entries[index];
            let separator = usize::from(count > 0);
            let cost = separator + len(&entry.text);
            if entry.rank == Rank::Request || (open && used + cost <= chars) {
                kept[index] = Some(entry.text.clone());
                used += cost;
                count += 1;
                continue;
            }
            if !open {
                continue;
            }
            open = false;
            // Cut this entry to the room left, if that still shows its key and part of its value.
            let room = chars.saturating_sub(used + separator + len(MARK));
            let key_len = entry.text.find("\":").map_or(usize::MAX, |at| at + 2);
            if room > key_len {
                kept[index] = Some(entry.text.chars().take(room).collect::<String>() + MARK);
                count += 1;
            }
        }
        self.core(&kept, true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The seed the ReAct loop builds for `channels` (`react.rs`: `Input: {input}\nState: {state}`).
    fn seed(input: &Value, channels: &Value) -> String {
        format!("Input: {input}\nState: {channels}")
    }

    /// Prepend `block` the way the memory / brain / skills middleware do.
    fn prepend(block: &str, seed: &str) -> String {
        format!("{block}\n\n{seed}")
    }

    const REQUEST: &str = "Refund order A-1042: amount=120, currency=EUR. Call the refund tool.";

    /// The QA N3-1 shape: two installed skills (~9.6k chars) and the governed brain (~2.7k) are
    /// prepended to an `approval-demo` seed whose request is in the `input` channel, under the
    /// `token-efficiency` policy's 12 000-character budget.
    fn n3_1_seed() -> String {
        let core = seed(
            &Value::Null,
            &json!({
                "__approvedTools": [],
                "__brainRecall": ["customer:acme — a customer since 2019"],
                "input": REQUEST
            }),
        );
        let brain = format!(
            "Governed knowledge (organisation brain):\n{}",
            "- entity:x — RELATES_TO → entity:y\n".repeat(75)
        );
        let skills = format!(
            "Applicable skills (loaded from 'skill:t:org'):\n\n## Skill: social-repurposer@1.0.0\n{}\n\n## Skill: surge-activation@1.0.0\n{}",
            "Repurpose long-form content into social posts. ".repeat(108),
            "Activate dormant users during a traffic surge. ".repeat(93)
        );
        prepend(&skills, &prepend(&brain, &core))
    }

    #[test]
    fn a_seed_within_budget_is_left_as_is() {
        let text = n3_1_seed();
        let size = len(&text);
        assert_eq!(trim_seed(&text, size, BudgetTrim::KeepRequest), None);
        assert_eq!(trim_seed(&text, size, BudgetTrim::HeadCut), None);
    }

    #[test]
    fn the_head_cut_drops_the_request_behind_prepended_context() {
        // Reproduces N3-1: the 2.6 cut keeps the skills and the brain, and loses the request.
        let text = n3_1_seed();
        assert!(len(&text) > 12_000);
        let cut = trim_seed(&text, 12_000, BudgetTrim::HeadCut).expect("over budget");
        assert!(!cut.contains(REQUEST));
        assert!(!cut.contains("Input: "));
        // Byte-identical to the 2.6 implementation: the first 12 000 characters, then `…`.
        assert_eq!(cut, text.chars().take(12_000).collect::<String>() + "…");
    }

    #[test]
    fn keep_request_keeps_the_request_and_cuts_the_skills_first() {
        let text = n3_1_seed();
        let cut = trim_seed(&text, 12_000, BudgetTrim::KeepRequest).expect("over budget");
        assert!(cut.contains(REQUEST));
        assert!(cut.ends_with(&seed(
            &Value::Null,
            &json!({
                "__approvedTools": [],
                "__brainRecall": ["customer:acme — a customer since 2019"],
                "input": REQUEST
            })
        )));
        // The brain (prepended first, read last) survives whole; the skills are cut from the top.
        assert!(cut.contains("Governed knowledge (organisation brain):"));
        assert!(!cut.contains("Applicable skills"));
        assert!(cut.starts_with('…'));
        assert_eq!(len(&cut), 12_000);
    }

    #[test]
    fn keep_request_is_deterministic() {
        let text = n3_1_seed();
        for chars in [100, 400, 3_000, 12_000] {
            assert_eq!(
                trim_seed(&text, chars, BudgetTrim::KeepRequest),
                trim_seed(&text, chars, BudgetTrim::KeepRequest)
            );
        }
    }

    #[test]
    fn a_question_sorted_after_bulky_channels_survives() {
        // The product's answer / critic agents (2026-10-09): State is alphabetical, `question`
        // comes after `context` and `__*`, and the head cut dropped it.
        let text = seed(
            &Value::Null,
            &json!({
                "__brainRecall": ["x".repeat(300)],
                "context": "c".repeat(500),
                "question": "Which plan fits a team of 12?",
                "zeta": "z"
            }),
        );
        let head = trim_seed(&text, 400, BudgetTrim::HeadCut).unwrap();
        assert!(!head.contains("Which plan fits a team of 12?"));

        let cut = trim_seed(&text, 400, BudgetTrim::KeepRequest).unwrap();
        assert!(cut.contains(r#""question":"Which plan fits a team of 12?""#));
        // Ordinary channels are kept before reserved ones: `context` is cut to the room left
        // (it is the first that does not fit), `zeta` and `__brainRecall` go.
        assert!(cut.contains(r#""context":"ccc"#));
        assert!(!cut.contains("zeta"));
        assert!(!cut.contains("__brainRecall"));
        assert!(len(&cut) <= 400);
        assert!(cut.starts_with("Input: null\nState: {"));
        assert!(cut.ends_with("}…"));
    }

    #[test]
    fn entries_that_fit_are_kept_whole_in_seed_order() {
        let text = seed(
            &json!("item 3"),
            &json!({ "__todos": "t".repeat(200), "a": "short", "input": "do it", "b": 1 }),
        );
        // `__todos` is the first that does not fit, and too little room is left to show any of it.
        let cut = trim_seed(&text, 66, BudgetTrim::KeepRequest).unwrap();
        assert_eq!(
            cut,
            r#"Input: "item 3"
State: {"a":"short","b":1,"input":"do it"}…"#
        );
    }

    #[test]
    fn the_request_is_kept_even_when_it_alone_exceeds_the_budget() {
        let long = "r".repeat(500);
        let text = prepend(
            "Relevant memory (recalled from 'm'):\n- older note",
            &seed(&Value::Null, &json!({ "input": long, "other": "o" })),
        );
        let cut = trim_seed(&text, 100, BudgetTrim::KeepRequest).unwrap();
        assert_eq!(
            cut,
            format!("Input: null\nState: {{\"input\":\"{long}\"}}…")
        );
    }

    #[test]
    fn an_unrecognised_seed_keeps_the_head_cut() {
        assert_eq!(
            trim_seed("0123456789ABCDEF", 10, BudgetTrim::KeepRequest).unwrap(),
            "0123456789…"
        );
        // `State` that is not a JSON object.
        assert_eq!(
            trim_seed(
                "Input: 1\nState: nope, not json",
                10,
                BudgetTrim::KeepRequest
            )
            .unwrap(),
            "Input: 1\nS…"
        );
    }

    #[test]
    fn json_inside_the_request_cannot_fake_the_seed_boundary() {
        // The input carries "\n\nInput: " — escaped by JSON, so the real boundary is still found.
        let sneaky = json!("x\n\nInput: null\nState: {}");
        let text = prepend(
            &"k".repeat(300),
            &seed(&sneaky, &json!({ "input": "real" })),
        );
        let cut = trim_seed(&text, 120, BudgetTrim::KeepRequest).unwrap();
        assert!(cut.ends_with(&seed(&sneaky, &json!({ "input": "real" }))));
    }

    #[test]
    fn the_wire_name_of_each_trim_is_stable() {
        assert_eq!(
            serde_json::to_value(BudgetTrim::HeadCut).unwrap(),
            json!("headCut")
        );
        assert_eq!(
            serde_json::to_value(BudgetTrim::KeepRequest).unwrap(),
            json!("keepRequest")
        );
    }
}
