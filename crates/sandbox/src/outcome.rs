//! Outcome of an `exec`, as declared by the backend.

use crate::error::LimitKind;
use serde::{Deserialize, Serialize};

/// How the program ended, as declared by the backend. Not independently verified.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum DeclaredExit {
    /// Normal exit with a status code.
    Code { code: i32 },
    /// Terminated by a signal (name as reported by the backend).
    Signal { name: String },
    /// Killed by the backend because a limit was exceeded.
    Killed { cause: LimitKind },
}

/// Which output stream was cut at its size bound.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Truncation {
    Stdout,
    Stderr,
}

/// Result of one `exec`. Values prefixed `declared_` come from the backend and are named so
/// because the engine cannot guarantee their accuracy.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecOutcome {
    pub declared_exit: DeclaredExit,
    /// `true` when at least one stream was cut at the output bound. This is not an error.
    pub truncated: bool,
    /// Which streams were cut; empty when `truncated` is `false`.
    #[serde(default)]
    pub truncation: Vec<Truncation>,
    pub declared_duration_ms: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outcome_round_trips_with_camel_case_fields() {
        let outcome = ExecOutcome {
            declared_exit: DeclaredExit::Code { code: 0 },
            truncated: true,
            truncation: vec![Truncation::Stdout],
            declared_duration_ms: 12,
        };
        let json = serde_json::to_value(&outcome).unwrap();
        assert_eq!(json["declaredExit"]["kind"], "code");
        assert_eq!(json["declaredDurationMs"], 12);
        assert_eq!(json["truncation"][0], "stdout");
        let back: ExecOutcome = serde_json::from_value(json).unwrap();
        assert_eq!(back, outcome);
    }

    #[test]
    fn killed_exit_names_the_limit() {
        let exit = DeclaredExit::Killed {
            cause: LimitKind::WallTime,
        };
        let json = serde_json::to_value(&exit).unwrap();
        assert_eq!(json["cause"], "wall_time");
    }
}
