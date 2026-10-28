//! A resolved secret value.

/// The value of a secret, resolved by the host on the backend side.
///
/// It is deliberately not `Serialize`, not `Clone`, and has no `Display`; `Debug` is masked.
/// The only way to read it is [`ResolvedSecret::expose`], so a leak requires an explicit,
/// greppable call. Requests, proofs, events and journals never carry one.
pub struct ResolvedSecret(String);

impl ResolvedSecret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Borrow the secret value. Handle with care: never log or serialize the result.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for ResolvedSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ResolvedSecret(***)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_is_masked() {
        let secret = ResolvedSecret::new("hunter2");
        assert_eq!(format!("{secret:?}"), "ResolvedSecret(***)");
        assert_eq!(format!("{secret:#?}"), "ResolvedSecret(***)");
        assert_eq!(secret.expose(), "hunter2");
    }

    #[test]
    fn debug_of_a_container_does_not_leak() {
        let wrapped = Some(ResolvedSecret::new("hunter2"));
        assert!(!format!("{wrapped:?}").contains("hunter2"));
    }
}
