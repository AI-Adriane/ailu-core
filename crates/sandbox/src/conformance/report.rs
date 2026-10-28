//! The cases, their outcomes and the report of a run.

use serde::Serialize;

/// What a case needs from the target in order to run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConformanceClass {
    /// A property of the guard: policy, approval, idempotence, registry, redaction, events.
    /// It holds whatever the backend and runs against the guard with any conformant backend.
    /// The suite makes the backend misbehave by wrapping it, so no hook is needed.
    Guard,
    /// Needs a fault that only a simulator can produce on demand: a host crash at a precise
    /// point, days of clock in one step. Not applicable to a target whose hooks cannot.
    SimulatorOnly,
    /// An obligation of the backend itself, observed through the backend, its probes and its
    /// hooks: isolation, limits, network, residues, sweeps, durable idempotence.
    RealBackend,
}

/// The groups of the ADR, in the order the suite runs them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Group {
    Lifecycle,
    Idempotence,
    Limits,
    Network,
    Secrets,
    Paths,
    /// Multi-tenant.
    Tenants,
    Governance,
    Events,
    /// What a backend hands back to the guard, and the checks the guard makes on it. It holds
    /// the guard's share of the ADR's proof group, the part that needs no signer.
    Delivery,
    Keys,
    Bounds,
    /// Engine invariants.
    Engine,
}

impl Group {
    /// The stable name of the group, the prefix of the ids of its cases.
    pub fn name(self) -> &'static str {
        match self {
            Group::Lifecycle => "lifecycle",
            Group::Idempotence => "idempotence",
            Group::Limits => "limits",
            Group::Network => "network",
            Group::Secrets => "secrets",
            Group::Paths => "paths",
            Group::Tenants => "tenants",
            Group::Governance => "governance",
            Group::Events => "events",
            Group::Delivery => "delivery",
            Group::Keys => "keys",
            Group::Bounds => "bounds",
            Group::Engine => "engine",
        }
    }
}

/// One case of the suite. The id is stable: a report from one version can be compared with
/// another.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct Case {
    /// `<group>.<name>`, stable.
    pub id: &'static str,
    pub class: ConformanceClass,
    pub group: Group,
    pub description: &'static str,
}

/// How a case ended.
///
/// A reason never holds content of the run (no argument, no output, no secret): only codes
/// and fixed words.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "status", content = "reason", rename_all = "snake_case")]
pub enum CaseOutcome {
    /// The attack failed in a typed and journaled way, or the obligation was observed.
    Passed,
    /// The target does not meet the obligation.
    Failed(String),
    /// The target cannot provoke or observe what the case needs. This is not a pass: a
    /// production backend lists these in its report.
    NotApplicable(String),
}

/// A case and how it ended.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CaseResult {
    pub case: Case,
    pub outcome: CaseOutcome,
}

/// The result of a run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ConformanceReport {
    /// The target's own name.
    pub target: String,
    /// Every case that ran, in order.
    pub cases: Vec<CaseResult>,
}

impl ConformanceReport {
    fn count(&self, wanted: fn(&CaseOutcome) -> bool) -> usize {
        self.cases.iter().filter(|c| wanted(&c.outcome)).count()
    }

    /// Cases that passed.
    pub fn passed(&self) -> usize {
        self.count(|o| matches!(o, CaseOutcome::Passed))
    }

    /// Cases that failed.
    pub fn failed(&self) -> usize {
        self.count(|o| matches!(o, CaseOutcome::Failed(_)))
    }

    /// Cases the target could not provoke or observe.
    pub fn not_applicable(&self) -> usize {
        self.count(|o| matches!(o, CaseOutcome::NotApplicable(_)))
    }

    /// `true` when no case failed. Cases that are not applicable do not fail a target, but a
    /// production backend must list them: read [`ConformanceReport::not_applicable_cases`].
    pub fn is_conformant(&self) -> bool {
        self.failed() == 0
    }

    /// The cases that failed, with their reasons.
    pub fn failures(&self) -> impl Iterator<Item = &CaseResult> {
        self.cases
            .iter()
            .filter(|c| matches!(c.outcome, CaseOutcome::Failed(_)))
    }

    /// The cases that were not applicable, with their reasons.
    pub fn not_applicable_cases(&self) -> impl Iterator<Item = &CaseResult> {
        self.cases
            .iter()
            .filter(|c| matches!(c.outcome, CaseOutcome::NotApplicable(_)))
    }

    /// The outcome of one case.
    pub fn outcome_of(&self, id: &str) -> Option<&CaseOutcome> {
        self.cases
            .iter()
            .find(|c| c.case.id == id)
            .map(|c| &c.outcome)
    }

    /// The report as JSON. It holds ids, classes, counts and reasons, never content of a run.
    pub fn to_json(&self) -> serde_json::Value {
        let mut value = serde_json::to_value(self).unwrap_or(serde_json::Value::Null);
        if let Some(object) = value.as_object_mut() {
            object.insert("passed".to_owned(), self.passed().into());
            object.insert("failed".to_owned(), self.failed().into());
            object.insert("notApplicable".to_owned(), self.not_applicable().into());
            object.insert("conformant".to_owned(), self.is_conformant().into());
        }
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn case(id: &'static str) -> Case {
        Case {
            id,
            class: ConformanceClass::Guard,
            group: Group::Lifecycle,
            description: "a case",
        }
    }

    fn report(outcomes: Vec<(&'static str, CaseOutcome)>) -> ConformanceReport {
        ConformanceReport {
            target: "target".to_owned(),
            cases: outcomes
                .into_iter()
                .map(|(id, outcome)| CaseResult {
                    case: case(id),
                    outcome,
                })
                .collect(),
        }
    }

    #[test]
    fn a_report_counts_each_outcome() {
        let report = report(vec![
            ("a", CaseOutcome::Passed),
            ("b", CaseOutcome::Passed),
            ("c", CaseOutcome::Failed("why".to_owned())),
            ("d", CaseOutcome::NotApplicable("no probe".to_owned())),
        ]);
        assert_eq!(
            (report.passed(), report.failed(), report.not_applicable()),
            (2, 1, 1)
        );
        assert_eq!(report.failures().count(), 1);
        assert_eq!(report.not_applicable_cases().count(), 1);
        assert_eq!(
            report.outcome_of("d"),
            Some(&CaseOutcome::NotApplicable("no probe".to_owned()))
        );
        assert_eq!(report.outcome_of("missing"), None);
    }

    #[test]
    fn only_a_failure_makes_a_target_non_conformant() {
        assert!(report(vec![]).is_conformant());
        assert!(report(vec![
            ("a", CaseOutcome::Passed),
            ("b", CaseOutcome::NotApplicable("x".to_owned()))
        ])
        .is_conformant());
        assert!(!report(vec![("a", CaseOutcome::Failed("x".to_owned()))]).is_conformant());
    }

    #[test]
    fn the_json_holds_ids_classes_counts_and_reasons() {
        let json = report(vec![
            ("lifecycle.a", CaseOutcome::Passed),
            (
                "lifecycle.b",
                CaseOutcome::Failed("expected ok, got busy".to_owned()),
            ),
        ])
        .to_json();
        assert_eq!(json["target"], "target");
        assert_eq!(json["passed"], 1);
        assert_eq!(json["failed"], 1);
        assert_eq!(json["notApplicable"], 0);
        assert_eq!(json["conformant"], false);
        assert_eq!(json["cases"][0]["case"]["id"], "lifecycle.a");
        assert_eq!(json["cases"][0]["case"]["class"], "guard");
        assert_eq!(json["cases"][0]["outcome"]["status"], "passed");
        assert_eq!(json["cases"][1]["outcome"]["status"], "failed");
        assert_eq!(
            json["cases"][1]["outcome"]["reason"],
            "expected ok, got busy"
        );
    }

    #[test]
    fn group_names_are_the_prefix_of_case_ids() {
        for group in [
            Group::Lifecycle,
            Group::Idempotence,
            Group::Limits,
            Group::Network,
            Group::Secrets,
            Group::Paths,
            Group::Tenants,
            Group::Governance,
            Group::Events,
            Group::Delivery,
            Group::Keys,
            Group::Bounds,
            Group::Engine,
        ] {
            assert_eq!(
                serde_json::to_value(group).unwrap(),
                group.name(),
                "the wire name of a group is its prefix"
            );
        }
    }
}
