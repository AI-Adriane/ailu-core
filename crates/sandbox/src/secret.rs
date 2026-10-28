//! A resolved secret value.

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_the_value_it_was_given() {
        assert_eq!(ResolvedSecret::new("hunter2").expose(), "hunter2");
        assert_eq!(ResolvedSecret::new(String::from("x")).expose(), "x");
    }
}
