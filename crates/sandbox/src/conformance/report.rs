//! The cases, their outcomes and the report of a run.

use super::runner::catalogue;
use serde::Serialize;
use std::collections::HashSet;

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

/// The groups of the suite, in the order the suite runs them.
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
    /// the guard's share of the proof group of the sandbox port design, the part that needs no signer.
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

    /// What the report lacks or has too much of, compared with the catalogue of the suite: a
    /// case of the catalogue that is not in the report, an id the catalogue does not know, an
    /// id that appears twice. Empty when the report covers the catalogue exactly once. The
    /// reasons hold ids only.
    pub fn coverage_gaps(&self) -> Vec<String> {
        let mut gaps = Vec::new();
        let known: Vec<&'static str> = catalogue().into_iter().map(|case| case.id).collect();
        let mut seen: HashSet<&str> = HashSet::new();
        for result in &self.cases {
            let id = result.case.id;
            if !known.contains(&id) {
                gaps.push(format!("unknown case {id}"));
            } else if !seen.insert(id) {
                gaps.push(format!("case {id} appears more than once"));
            }
        }
        for id in known {
            if !seen.contains(id) {
                gaps.push(format!("case {id} is missing"));
            }
        }
        gaps
    }

    /// `true` when no case failed, at least one passed **and** the report covers the whole
    /// catalogue of the suite, each case once ([`ConformanceReport::coverage_gaps`] is empty).
    /// An empty report, one where nothing could run, or one from a run of a few cases
    /// ([`run_cases`](crate::conformance::run_cases)) proves nothing about the backend. Cases
    /// that are not applicable do not fail a target, but a production backend must account for
    /// them: that is what [`ConformanceReport::is_complete`] adds.
    pub fn is_conformant(&self) -> bool {
        self.failed() == 0 && self.passed() > 0 && self.coverage_gaps().is_empty()
    }

    /// `true` when the report is conformant and leaves nothing unexplained: every case that is
    /// not applicable is one the caller accepts (`accepted_na` lists their ids), except the
    /// cases of the class [`ConformanceClass::SimulatorOnly`], which no real backend can run.
    /// A case of the guard or of the backend that the target could not run, and that the
    /// caller did not declare, makes the report incomplete. Coverage of the catalogue is
    /// already a condition of being conformant, so a report with a missing, unknown or
    /// repeated case is never complete.
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
            object.insert("coverageGaps".to_owned(), self.coverage_gaps().into());
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

    /// A report of the whole catalogue, every case passed.
    fn full() -> ConformanceReport {
        ConformanceReport {
            target: "target".to_owned(),
            cases: catalogue()
                .into_iter()
                .map(|case| CaseResult {
                    case,
                    outcome: CaseOutcome::Passed,
                })
                .collect(),
        }
    }

    #[test]
    fn a_failure_or_a_run_that_proved_nothing_is_not_conformant() {
        assert!(full().is_conformant());
        assert!(
            !report(vec![]).is_conformant(),
            "an empty report proves nothing"
        );
        let mut nothing_ran = full();
        for result in &mut nothing_ran.cases {
            result.outcome = CaseOutcome::NotApplicable("x".to_owned());
        }
        assert!(!nothing_ran.is_conformant(), "nothing ran");
        let mut measured = full();
        for result in &mut measured.cases {
            result.outcome = CaseOutcome::Measured("x".to_owned());
        }
        assert!(!measured.is_conformant(), "a measure is not a pass");
        let mut mixed = full();
        mixed.cases[1].outcome = CaseOutcome::NotApplicable("x".to_owned());
        mixed.cases[2].outcome = CaseOutcome::Measured("x".to_owned());
        assert!(mixed.is_conformant());
        let mut failed = full();
        failed.cases[0].outcome = CaseOutcome::Failed("x".to_owned());
        assert!(!failed.is_conformant());
    }

    #[test]
    fn a_report_that_does_not_cover_the_catalogue_is_neither_conformant_nor_complete() {
        // A run of one case, passed.
        let mut one = full();
        one.cases.truncate(1);
        assert_eq!(one.passed(), 1);
        assert!(!one.is_conformant());
        assert!(!one.is_complete(&[]));
        assert_eq!(one.to_json()["conformant"], false);
        assert_eq!(one.coverage_gaps().len(), catalogue().len() - 1);
        // One case missing from the middle.
        let mut missing = full();
        missing.cases.remove(3);
        assert!(!missing.is_conformant());
        assert!(!missing.is_complete(&[]));
        assert_eq!(missing.coverage_gaps().len(), 1);
        // An id nobody knows, added to a whole report: ignored silently before.
        let mut unknown = full();
        unknown.cases.push(CaseResult {
            case: case("lifecycle.invented"),
            outcome: CaseOutcome::Passed,
        });
        assert!(!unknown.is_conformant());
        assert!(!unknown.is_complete(&[]));
        assert_eq!(
            unknown.coverage_gaps(),
            vec!["unknown case lifecycle.invented".to_owned()]
        );
        // An id twice.
        let mut twice = full();
        let again = twice.cases[0].clone();
        twice.cases.push(again);
        assert!(!twice.is_conformant());
        assert!(!twice.is_complete(&[]));
        assert_eq!(twice.coverage_gaps().len(), 1);
        assert!(full().coverage_gaps().is_empty());
        assert_eq!(full().to_json()["coverageGaps"], serde_json::json!([]));
    }

    #[test]
    fn a_report_is_complete_when_every_skipped_case_is_accepted() {
        let mut report = full();
        let skipped = report
            .cases
            .iter()
            .position(|result| result.case.class == ConformanceClass::RealBackend)
            .expect("the suite has a case of the backend");
        let id = report.cases[skipped].case.id;
        report.cases[skipped].outcome = CaseOutcome::NotApplicable("no probe".to_owned());
        assert!(report.is_conformant());
        assert!(!report.is_complete(&[]), "a skipped case nobody accepted");
        assert!(report.is_complete(&[id]));
        assert_eq!(report.to_json()["complete"], false);
        assert_eq!(report.to_json_accepting(&[id])["complete"], true);
        // A case only a simulator can run does not hold a real backend back.
        let simulator = report
            .cases
            .iter()
            .position(|result| result.case.class == ConformanceClass::SimulatorOnly)
            .expect("the suite has a case only a simulator can run");
        report.cases[skipped].outcome = CaseOutcome::Passed;
        report.cases[simulator].outcome = CaseOutcome::NotApplicable("no hook".to_owned());
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
