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
    /// The case measured something the contract cannot judge (for instance what an allowed
    /// host that relays traffic lets through). It is neither a pass nor a failure.
    Measured(String),
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

    /// Cases that measured something the contract cannot judge.
    pub fn measured(&self) -> usize {
        self.count(|o| matches!(o, CaseOutcome::Measured(_)))
    }

    /// `true` when no case failed **and** at least one passed: an empty report, or one where
    /// nothing could run, proves nothing. Cases that are not applicable do not fail a target,
    /// but a production backend must account for them: read [`ConformanceReport::is_complete`].
    pub fn is_conformant(&self) -> bool {
        self.failed() == 0 && self.passed() > 0
    }

    /// `true` when the report is conformant and leaves nothing unexplained: every case that is
    /// not applicable is one the caller accepts (`accepted_na` lists their ids), except the
    /// cases of the class [`ConformanceClass::SimulatorOnly`], which no real backend can run.
    /// A case of the guard or of the backend that the target could not run, and that the
    /// caller did not declare, makes the report incomplete.
    pub fn is_complete(&self, accepted_na: &[&str]) -> bool {
        self.is_conformant()
            && self.not_applicable_cases().all(|result| {
                result.case.class == ConformanceClass::SimulatorOnly
                    || accepted_na.contains(&result.case.id)
            })
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
    /// `complete` is computed with no accepted case: see [`ConformanceReport::to_json_accepting`].
    pub fn to_json(&self) -> serde_json::Value {
        self.to_json_accepting(&[])
    }

    /// The report as JSON, with `complete` computed as [`ConformanceReport::is_complete`] does
    /// for the accepted not-applicable cases.
    pub fn to_json_accepting(&self, accepted_na: &[&str]) -> serde_json::Value {
        let mut value = serde_json::to_value(self).unwrap_or(serde_json::Value::Null);
        if let Some(object) = value.as_object_mut() {
            object.insert("passed".to_owned(), self.passed().into());
            object.insert("failed".to_owned(), self.failed().into());
            object.insert("notApplicable".to_owned(), self.not_applicable().into());
            object.insert("measured".to_owned(), self.measured().into());
            object.insert("conformant".to_owned(), self.is_conformant().into());
            object.insert("complete".to_owned(), self.is_complete(accepted_na).into());
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
    fn a_failure_or_a_run_that_proved_nothing_is_not_conformant() {
        assert!(
            !report(vec![]).is_conformant(),
            "an empty report proves nothing"
        );
        assert!(
            !report(vec![("a", CaseOutcome::NotApplicable("x".to_owned()))]).is_conformant(),
            "nothing ran"
        );
        assert!(
            !report(vec![("a", CaseOutcome::Measured("x".to_owned()))]).is_conformant(),
            "a measure is not a pass"
        );
        assert!(report(vec![
            ("a", CaseOutcome::Passed),
            ("b", CaseOutcome::NotApplicable("x".to_owned())),
            ("c", CaseOutcome::Measured("x".to_owned()))
        ])
        .is_conformant());
        assert!(!report(vec![
            ("a", CaseOutcome::Passed),
            ("b", CaseOutcome::Failed("x".to_owned()))
        ])
        .is_conformant());
    }

    #[test]
    fn a_report_is_complete_when_every_skipped_case_is_accepted() {
        let skipped = |class| Case {
            id: "idempotence.skipped",
            class,
            group: Group::Idempotence,
            description: "a case",
        };
        let mut report = report(vec![("a", CaseOutcome::Passed)]);
        report.cases.push(CaseResult {
            case: skipped(ConformanceClass::RealBackend),
            outcome: CaseOutcome::NotApplicable("no probe".to_owned()),
        });
        assert!(report.is_conformant());
        assert!(!report.is_complete(&[]), "a skipped case nobody accepted");
        assert!(report.is_complete(&["idempotence.skipped"]));
        assert_eq!(report.to_json()["complete"], false);
        assert_eq!(
            report.to_json_accepting(&["idempotence.skipped"])["complete"],
            true
        );
        // A case only a simulator can run does not hold a real backend back.
        report.cases[1].case = skipped(ConformanceClass::SimulatorOnly);
        assert!(report.is_complete(&[]));
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
        assert_eq!(json["complete"], false);
        assert_eq!(json["measured"], 0);
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
