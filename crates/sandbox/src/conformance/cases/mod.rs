//! The cases, one module per group of the ADR.

use super::report::Case;
use super::support::{BoxFut, Lab, Verdict};

/// A case and the function that runs it.
pub(crate) struct CaseDef {
    pub(crate) case: Case,
    pub(crate) run: CaseFn,
}

pub(crate) type CaseFn = for<'a, 'b> fn(&'a Lab<'b>) -> BoxFut<'a, Verdict>;

/// Every case, in the order the groups run.
pub(crate) fn all() -> Vec<CaseDef> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn ids_are_unique_and_start_with_their_group() {
        let all = all();
        let ids: HashSet<_> = all.iter().map(|def| def.case.id).collect();
        assert_eq!(ids.len(), all.len(), "an id is used twice");
        for def in &all {
            let prefix = format!("{}.", def.case.group.name());
            assert!(
                def.case.id.starts_with(&prefix),
                "{} is not in the group {}",
                def.case.id,
                def.case.group.name()
            );
            assert!(!def.case.description.is_empty());
        }
    }
}
