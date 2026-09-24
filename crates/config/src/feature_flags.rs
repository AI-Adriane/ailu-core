//! Feature flags — the Rust port of `@ailu-ai/config`'s `feature-flags.ts`.
//!
//! Flags are env-driven and default to disabled. A flag named `multi-agent`
//! maps to the env key `FEATURE_MULTI_AGENT` (dashes → underscores, uppercased,
//! `FEATURE_` prefix), and a value is "truthy" only when its trimmed,
//! lowercased form equals `"true"` — faithfully mirroring the TS semantics.

use std::collections::HashMap;

/// The closed set of feature flags, mirroring the TS `FEATURE_FLAGS` tuple.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FeatureFlag {
    /// `streaming`
    Streaming,
    /// `subgraphs`
    Subgraphs,
    /// `multi-agent`
    MultiAgent,
    /// `eval`
    Eval,
    /// `fleet`
    Fleet,
}

/// All feature flags, in declaration order (mirrors `FEATURE_FLAGS`).
pub const FEATURE_FLAGS: [FeatureFlag; 5] = [
    FeatureFlag::Streaming,
    FeatureFlag::Subgraphs,
    FeatureFlag::MultiAgent,
    FeatureFlag::Eval,
    FeatureFlag::Fleet,
];

impl FeatureFlag {
    /// The wire/flag name as used in the TS API (e.g. `"multi-agent"`).
    pub fn name(self) -> &'static str {
        match self {
            FeatureFlag::Streaming => "streaming",
            FeatureFlag::Subgraphs => "subgraphs",
            FeatureFlag::MultiAgent => "multi-agent",
            FeatureFlag::Eval => "eval",
            FeatureFlag::Fleet => "fleet",
        }
    }

    /// The environment key for this flag (e.g. `"FEATURE_MULTI_AGENT"`),
    /// mirroring `toEnvKey`: dashes → underscores, uppercased, `FEATURE_`
    /// prefix.
    pub fn env_key(self) -> String {
        format!("FEATURE_{}", self.name().replace('-', "_").to_uppercase())
    }
}

/// Whether a string value counts as enabling a flag.
///
/// Mirrors `isTruthy`: `None` is false; otherwise the value is trimmed,
/// lowercased, and compared to `"true"`.
fn is_truthy(value: Option<&String>) -> bool {
    match value {
        None => false,
        Some(v) => v.trim().to_lowercase() == "true",
    }
}

/// Whether the given flag is enabled in the source map.
///
/// Mirrors `isEnabled(flag, source)`.
pub fn is_enabled(flag: FeatureFlag, source: &HashMap<String, String>) -> bool {
    is_truthy(source.get(&flag.env_key()))
}

/// Read every flag from the source map into a name → enabled map.
///
/// Mirrors `getAllFlags(source)`. The returned map keys are the flag *names*
/// (`"multi-agent"`, not `MULTI_AGENT`) to match the TS record shape.
pub fn get_all_flags(source: &HashMap<String, String>) -> HashMap<&'static str, bool> {
    FEATURE_FLAGS
        .iter()
        .map(|flag| (flag.name(), is_enabled(*flag, source)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn returns_true_when_flag_is_enabled() {
        let enabled = is_enabled(
            FeatureFlag::Streaming,
            &map(&[("FEATURE_STREAMING", "true")]),
        );
        assert!(enabled);
    }

    #[test]
    fn returns_false_when_flag_is_disabled_or_missing() {
        let disabled_explicit = is_enabled(
            FeatureFlag::Streaming,
            &map(&[("FEATURE_STREAMING", "false")]),
        );
        let disabled_missing = is_enabled(FeatureFlag::Subgraphs, &HashMap::new());
        assert!(!disabled_explicit);
        assert!(!disabled_missing);
    }

    #[test]
    fn returns_all_flags_as_a_record() {
        let flags = get_all_flags(&map(&[
            ("FEATURE_STREAMING", "true"),
            ("FEATURE_SUBGRAPHS", "false"),
            ("FEATURE_MULTI_AGENT", "true"),
            ("FEATURE_EVAL", "false"),
            ("FEATURE_FLEET", "true"),
        ]));

        assert_eq!(flags.get("streaming"), Some(&true));
        assert_eq!(flags.get("subgraphs"), Some(&false));
        assert_eq!(flags.get("multi-agent"), Some(&true));
        assert_eq!(flags.get("eval"), Some(&false));
        assert_eq!(flags.get("fleet"), Some(&true));
        assert_eq!(flags.len(), 5);
    }

    #[test]
    fn env_key_maps_dashes_and_uppercases() {
        assert_eq!(FeatureFlag::MultiAgent.env_key(), "FEATURE_MULTI_AGENT");
        assert_eq!(FeatureFlag::Streaming.env_key(), "FEATURE_STREAMING");
        assert_eq!(FeatureFlag::Eval.env_key(), "FEATURE_EVAL");
    }

    #[test]
    fn truthy_is_trimmed_and_case_insensitive() {
        assert!(is_enabled(
            FeatureFlag::Fleet,
            &map(&[("FEATURE_FLEET", "  TRUE  ")])
        ));
        assert!(is_enabled(
            FeatureFlag::Fleet,
            &map(&[("FEATURE_FLEET", "True")])
        ));
        assert!(!is_enabled(
            FeatureFlag::Fleet,
            &map(&[("FEATURE_FLEET", "1")])
        ));
        assert!(!is_enabled(
            FeatureFlag::Fleet,
            &map(&[("FEATURE_FLEET", "yes")])
        ));
    }
}
