//! A resolved secret value.

use crate::ids::SandboxContext;
use crate::request::SecretRef;
use async_trait::async_trait;
use zeroize::Zeroizing;

/// The value of one secret, resolved by the host on the backend side.
///
/// It has no `Debug`, no `Display`, no `Clone` and no `Serialize`, so it cannot be printed,
/// copied around or written into a request, proof, event or journal by accident. The only way
/// to read it is [`ResolvedSecret::expose`]. The memory is overwritten when the value is
/// dropped.
///
/// ```compile_fail
/// fn needs_debug<T: std::fmt::Debug>() {}
/// needs_debug::<ailu_sandbox::ResolvedSecret>();
/// ```
///
/// ```compile_fail
/// fn needs_display<T: std::fmt::Display>() {}
/// needs_display::<ailu_sandbox::ResolvedSecret>();
/// ```
///
/// ```compile_fail
/// fn needs_clone<T: Clone>() {}
/// needs_clone::<ailu_sandbox::ResolvedSecret>();
/// ```
///
/// ```compile_fail
/// fn needs_serialize<T: serde::Serialize>() {}
/// needs_serialize::<ailu_sandbox::ResolvedSecret>();
/// ```
///
/// The constructor and the accessor, by contrast, are public:
///
/// ```
/// let secret = ailu_sandbox::ResolvedSecret::new("value");
/// assert_eq!(secret.expose(), "value");
/// ```
pub struct ResolvedSecret(Zeroizing<String>);

impl ResolvedSecret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(Zeroizing::new(value.into()))
    }

    /// Borrow the secret value. Never log or serialize the result.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

/// Why a secret reference could not be resolved. Carries no reference and no value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SecretError {
    /// Unknown reference, or a reference that belongs to another tenant: the two look the
    /// same, so the answer discloses nothing about other tenants.
    #[error("secret not found")]
    NotFound,
    /// The secret store could not answer. Worth retrying.
    #[error("secret store unavailable")]
    Unavailable,
}

/// Port to the host's secret store, used by a backend (never by the guard) to turn the
/// references granted in a `CreateRequest` into values.
///
/// It receives the whole [`SandboxContext`], so it can scope every lookup to the tenant and
/// **must refuse** a reference that belongs to another tenant, with the same
/// [`SecretError::NotFound`] as an unknown one. A resolver must not log or keep the values it
/// hands out beyond the call.
#[async_trait]
pub trait SecretResolver: Send + Sync {
    /// Resolve `reference` for the tenant of `ctx`.
    async fn resolve(
        &self,
        ctx: &SandboxContext,
        reference: &SecretRef,
    ) -> Result<ResolvedSecret, SecretError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_the_value_it_was_given() {
        assert_eq!(ResolvedSecret::new("hunter2").expose(), "hunter2");
        assert_eq!(ResolvedSecret::new(String::from("x")).expose(), "x");
    }
}
