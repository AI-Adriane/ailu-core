//! The operator's allow-list for the variables an agent's `apiKeyEnv` may name.
//!
//! An agent pointed at a custom OpenAI-compatible endpoint names the environment variable that
//! holds the endpoint's key (`apiKeyEnv`), and the engine reads that variable. When one process
//! runs graphs for many organisations, that lets a graph read any variable of the host. The
//! operator closes it with [`API_KEY_ENV_ALLOWLIST_VAR`]: a comma-separated list of exact
//! variable names and prefixes ending in `*` (`AILU_ENDPOINT_*`).
//!
//! - unset: every name is allowed, as before (a standalone SDK user is not affected);
//! - set, even to an empty string: a name that matches no entry is refused before the variable is
//!   read, whether or not it exists. An empty list allows none.

use std::fmt;

/// The operator variable that restricts which variables `apiKeyEnv` may name.
pub const API_KEY_ENV_ALLOWLIST_VAR: &str = "AILU_API_KEY_ENV_ALLOWLIST";

/// The parsed allow-list: exact names, and prefixes (an entry ending in `*`, the `*` dropped).
/// Matching is case-sensitive, as variable names are; only a trailing `*` is a wildcard.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ApiKeyEnvAllowlist {
    exact: Vec<String>,
    prefixes: Vec<String>,
}

impl ApiKeyEnvAllowlist {
    /// Parse the value of [`API_KEY_ENV_ALLOWLIST_VAR`]. Entries are trimmed; blank ones are
    /// ignored, so an empty or blank value allows no name.
    #[must_use]
    pub fn parse(raw: &str) -> Self {
        let mut allowlist = Self::default();
        for entry in raw
            .split(',')
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
        {
            match entry.strip_suffix('*') {
                Some(prefix) => allowlist.prefixes.push(prefix.to_owned()),
                None => allowlist.exact.push(entry.to_owned()),
            }
        }
        allowlist
    }

    /// Whether `name` is one of the exact names or starts with one of the prefixes.
    #[must_use]
    pub fn allows(&self, name: &str) -> bool {
        self.exact.iter().any(|exact| exact == name)
            || self
                .prefixes
                .iter()
                .any(|prefix| name.starts_with(prefix.as_str()))
    }
}

/// Why the key named by `apiKeyEnv` could not be resolved. Names the variable, never a value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiKeyEnvError {
    /// [`API_KEY_ENV_ALLOWLIST_VAR`] is set and matches no entry for this name.
    NotAllowed { var: String },
    /// The name is allowed, but the variable is unset or empty.
    NotSet { var: String },
}

impl fmt::Display for ApiKeyEnvError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAllowed { var } => write!(
                f,
                "the custom OpenAI-compatible endpoint names apiKeyEnv '{var}', which \
                 {API_KEY_ENV_ALLOWLIST_VAR} does not allow: this host only reads endpoint keys \
                 from the variables its operator listed there"
            ),
            Self::NotSet { var } => write!(
                f,
                "the custom OpenAI-compatible endpoint names apiKeyEnv '{var}', but that \
                 environment variable is not set"
            ),
        }
    }
}

impl std::error::Error for ApiKeyEnvError {}

/// Resolve the key named by `api_key_env` under the operator's allow-list.
///
/// `allowlist` is the raw value of [`API_KEY_ENV_ALLOWLIST_VAR`] (`None`: unset) and `read`
/// reads one variable; both are injected so the policy is tested without touching the process
/// environment. A blank or absent name → `Ok(None)` (a keyless endpoint). A name the allow-list
/// refuses → [`ApiKeyEnvError::NotAllowed`], without calling `read`. An allowed name whose
/// variable is unset or empty → [`ApiKeyEnvError::NotSet`].
///
/// # Errors
///
/// [`ApiKeyEnvError`], as above.
pub fn resolve_api_key_env(
    api_key_env: Option<&str>,
    allowlist: Option<&str>,
    read: impl Fn(&str) -> Option<String>,
) -> Result<Option<String>, ApiKeyEnvError> {
    let Some(var) = api_key_env.map(str::trim).filter(|name| !name.is_empty()) else {
        return Ok(None);
    };
    if let Some(raw) = allowlist {
        if !ApiKeyEnvAllowlist::parse(raw).allows(var) {
            return Err(ApiKeyEnvError::NotAllowed {
                var: var.to_owned(),
            });
        }
    }
    match read(var) {
        Some(value) if !value.is_empty() => Ok(Some(value)),
        _ => Err(ApiKeyEnvError::NotSet {
            var: var.to_owned(),
        }),
    }
}

/// [`resolve_api_key_env`] against the process environment.
///
/// An allow-list that is set but not valid UTF-8 is read lossily: its entries then match no real
/// name, so the policy fails closed rather than open.
///
/// # Errors
///
/// [`ApiKeyEnvError`], as for [`resolve_api_key_env`].
pub fn resolve_api_key_env_from_process(
    api_key_env: Option<&str>,
) -> Result<Option<String>, ApiKeyEnvError> {
    let allowlist = std::env::var_os(API_KEY_ENV_ALLOWLIST_VAR)
        .map(|value| value.to_string_lossy().into_owned());
    resolve_api_key_env(api_key_env, allowlist.as_deref(), |name| {
        std::env::var(name).ok()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn env_with(var: &'static str, value: &'static str) -> impl Fn(&str) -> Option<String> {
        move |name| (name == var).then(|| value.to_owned())
    }

    #[test]
    fn exact_entries_match_only_the_whole_name() {
        let allowlist = ApiKeyEnvAllowlist::parse("GATEWAY_KEY,VLLM_KEY");
        assert!(allowlist.allows("GATEWAY_KEY"));
        assert!(allowlist.allows("VLLM_KEY"));
        assert!(!allowlist.allows("GATEWAY_KEY_2"));
        assert!(!allowlist.allows("GATEWAY"));
        assert!(!allowlist.allows("gateway_key"), "names are case-sensitive");
    }

    #[test]
    fn a_trailing_star_is_a_prefix() {
        let allowlist = ApiKeyEnvAllowlist::parse("AILU_ENDPOINT_*");
        assert!(allowlist.allows("AILU_ENDPOINT_"));
        assert!(allowlist.allows("AILU_ENDPOINT_VLLM"));
        assert!(!allowlist.allows("AILU_ENDPOINT"));
        assert!(!allowlist.allows("OPENAI_API_KEY"));
        assert!(!allowlist.allows("DATABASE_URL"));
    }

    #[test]
    fn only_a_trailing_star_is_a_wildcard() {
        let allowlist = ApiKeyEnvAllowlist::parse("AILU_*_KEY");
        assert!(!allowlist.allows("AILU_VLLM_KEY"));
        assert!(allowlist.allows("AILU_*_KEY"));
    }

    #[test]
    fn entries_are_trimmed_and_blank_ones_ignored() {
        let allowlist = ApiKeyEnvAllowlist::parse(" GATEWAY_KEY , ,AILU_ENDPOINT_* ,");
        assert!(allowlist.allows("GATEWAY_KEY"));
        assert!(allowlist.allows("AILU_ENDPOINT_X"));
        assert!(!allowlist.allows(""));
        assert!(!allowlist.allows(" GATEWAY_KEY "));
    }

    #[test]
    fn an_empty_or_blank_allowlist_allows_no_name() {
        for raw in ["", "   ", ",", " , "] {
            let allowlist = ApiKeyEnvAllowlist::parse(raw);
            assert!(!allowlist.allows("GATEWAY_KEY"), "{raw:?}");
            assert!(!allowlist.allows(""), "{raw:?}");
        }
    }

    #[test]
    fn without_an_allowlist_every_name_is_read_as_before() {
        assert_eq!(
            resolve_api_key_env(
                Some("MY_GATEWAY_KEY"),
                None,
                env_with("MY_GATEWAY_KEY", "k")
            ),
            Ok(Some("k".to_owned()))
        );
        assert_eq!(
            resolve_api_key_env(Some("MY_GATEWAY_KEY"), None, |_| None),
            Err(ApiKeyEnvError::NotSet {
                var: "MY_GATEWAY_KEY".to_owned()
            })
        );
    }

    #[test]
    fn no_name_is_a_keyless_endpoint_under_any_policy() {
        for allowlist in [None, Some(""), Some("GATEWAY_KEY")] {
            assert_eq!(resolve_api_key_env(None, allowlist, |_| None), Ok(None));
            assert_eq!(
                resolve_api_key_env(Some("  "), allowlist, |_| None),
                Ok(None)
            );
        }
    }

    #[test]
    fn an_allowed_name_is_read() {
        assert_eq!(
            resolve_api_key_env(
                Some("AILU_ENDPOINT_VLLM"),
                Some("AILU_ENDPOINT_*"),
                env_with("AILU_ENDPOINT_VLLM", "vllm-secret")
            ),
            Ok(Some("vllm-secret".to_owned()))
        );
        assert_eq!(
            resolve_api_key_env(Some(" GATEWAY_KEY "), Some("GATEWAY_KEY"), |name| {
                (name == "GATEWAY_KEY").then(|| "g".to_owned())
            }),
            Ok(Some("g".to_owned())),
            "the name is trimmed before it is matched and read"
        );
    }

    #[test]
    fn a_refused_name_is_never_read_whether_or_not_it_exists() {
        let reads = Cell::new(0);
        let read = |_: &str| {
            reads.set(reads.get() + 1);
            Some("host-secret".to_owned())
        };
        for allowlist in ["AILU_ENDPOINT_*", ""] {
            assert_eq!(
                resolve_api_key_env(Some("DATABASE_URL"), Some(allowlist), read),
                Err(ApiKeyEnvError::NotAllowed {
                    var: "DATABASE_URL".to_owned()
                })
            );
            assert_eq!(
                resolve_api_key_env(Some("AILU_TEST_NOT_SET"), Some(allowlist), |_| None),
                Err(ApiKeyEnvError::NotAllowed {
                    var: "AILU_TEST_NOT_SET".to_owned()
                })
            );
        }
        assert_eq!(reads.get(), 0, "a refused variable is not read");
    }

    #[test]
    fn an_allowed_but_missing_variable_is_not_set() {
        assert_eq!(
            resolve_api_key_env(Some("GATEWAY_KEY"), Some("GATEWAY_KEY"), |_| Some(
                String::new()
            )),
            Err(ApiKeyEnvError::NotSet {
                var: "GATEWAY_KEY".to_owned()
            })
        );
    }

    #[test]
    fn the_refusal_names_the_variable_and_the_allowlist_never_a_value() {
        let error = resolve_api_key_env(Some("DATABASE_URL"), Some("AILU_ENDPOINT_*"), |_| {
            Some("postgres://user:hunter2@db".to_owned())
        })
        .expect_err("refused");
        let message = error.to_string();
        assert!(message.contains("DATABASE_URL"), "{message}");
        assert!(message.contains(API_KEY_ENV_ALLOWLIST_VAR), "{message}");
        assert!(!message.contains("hunter2"), "{message}");
        assert!(!message.contains("AILU_ENDPOINT_*"), "{message}");
    }
}
