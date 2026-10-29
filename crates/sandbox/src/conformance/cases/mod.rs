//! The cases, one module per group of the suite.

use super::report::Case;
use super::support::{BoxFut, Lab, Verdict};

mod bounds;
mod deadlines;
mod delivery;
mod engine;
mod events;
mod governance;
mod idempotence;
mod keys;
mod lifecycle;
mod limits;
mod network;
mod paths;
mod secrets;
mod tenants;

/// A case and the function that runs it.
pub(crate) struct CaseDef {
    pub(crate) case: Case,
    pub(crate) run: CaseFn,
}

pub(crate) type CaseFn = for<'a, 'b> fn(&'a Lab<'b>) -> BoxFut<'a, Verdict>;

/// Declare a case: `case!(Group, "group.name", Class, "what it checks", function)`.
macro_rules! case {
    ($group:ident, $id:literal, $class:ident, $description:literal, $run:path) => {
        $crate::conformance::cases::CaseDef {
            case: $crate::conformance::report::Case {
                id: $id,
                group: $crate::conformance::report::Group::$group,
                class: $crate::conformance::report::ConformanceClass::$class,
                description: $description,
            },
            run: |lab| Box::pin($run(lab)),
        }
    };
}

pub(crate) use case;

/// Every case, in the order the groups run.
pub(crate) fn all() -> Vec<CaseDef> {
    let mut all = Vec::new();
    all.extend(lifecycle::cases());
    all.extend(idempotence::cases());
    all.extend(deadlines::cases());
    all.extend(limits::cases());
    all.extend(network::cases());
    all.extend(secrets::cases());
    all.extend(paths::cases());
    all.extend(tenants::cases());
    all.extend(governance::cases());
    all.extend(events::cases());
    all.extend(delivery::cases());
    all.extend(keys::cases());
    all.extend(bounds::cases());
    all.extend(engine::cases());
    all
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
