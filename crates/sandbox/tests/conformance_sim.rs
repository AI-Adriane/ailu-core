//! The conformance suite run against the simulated backend, from outside the crate: it uses
//! the public API only, as any backend's own test would.

use ailu_sandbox::conformance::{
    catalogue, run_cases, run_cases_with, run_conformance, CaseOutcome, ConformanceReport,
    ConformanceTarget, Instance, ProbeKind, ProbeProgram, SimTarget,
};
use ailu_sandbox::sim::SimBackend;
use ailu_sandbox::{
    CancelToken, CollectRequest, CreateRequest, ExecOutcome, ExecRequest, IdempotencyKey,
    RawCollected, RawCreated, SandboxBackend, SandboxContext, SandboxError, SandboxHandle,
};
use async_trait::async_trait;

/// The simulator passes at least this many cases; raised as the suite grows.
const MIN_PASSED: usize = 131;

/// The case that measures instead of judging.
const MEASURED: &[&str] = &["network.relay_measured"];

/// The cases the simulator cannot take part in. It offers every hook and every probe but one:
/// it runs a program in a single step, so there is no moment to catch it running.
const NOT_APPLICABLE: &[&str] = &["idempotence.backend_busy_on_running_key"];

/// What a failing run prints: the id and the reason of each failed case.
fn failures(report: &ConformanceReport) -> Vec<String> {
    report
        .failures()
        .map(|result| format!("{}: {:?}", result.case.id, result.outcome))
        .collect()
}

#[tokio::test]
async fn the_simulator_passes_every_case_that_applies_to_it() {
    let report = run_conformance(&SimTarget::new()).await;
    assert!(
        report.is_conformant(),
        "failed cases: {:#?}",
        failures(&report)
    );
    assert_eq!(report.cases.len(), catalogue().len());
    let skipped: Vec<_> = report
        .not_applicable_cases()
        .map(|result| result.case.id)
        .collect();
    assert_eq!(skipped, NOT_APPLICABLE, "cases the simulator skipped");
    assert_eq!(
        ids_where(&report, |o| matches!(o, CaseOutcome::Measured(_))),
        MEASURED
    );
    assert!(
        report.is_complete(NOT_APPLICABLE),
        "the simulator leaves a case unexplained"
    );
    assert_eq!(
        report.passed() + report.measured() + report.not_applicable(),
        report.cases.len()
    );
    assert!(
        report.passed() >= MIN_PASSED,
        "only {} cases passed out of {}",
        report.passed(),
        report.cases.len()
    );
}

/// The simulator without its probes.
struct NoProbes(SimTarget);

#[async_trait]
impl ConformanceTarget for NoProbes {
    type Backend = SimBackend;

    fn name(&self) -> String {
        "simulator without probes".to_owned()
    }

    async fn fresh(&self) -> Instance<SimBackend> {
        self.0.fresh().await
    }

    fn probe(&self, _kind: &ProbeKind) -> Option<ProbeProgram> {
        None
    }
}

/// The simulator without its hooks.
struct NoHooksTarget(SimTarget);

#[async_trait]
impl ConformanceTarget for NoHooksTarget {
    type Backend = SimBackend;

    fn name(&self) -> String {
        "simulator without hooks".to_owned()
    }

    async fn fresh(&self) -> Instance<SimBackend> {
        let fresh = self.0.fresh().await;
        let hidden = Instance::new(fresh.backend, fresh.clock);
        // Only the hooks are hidden: the proof signer is not a hook.
        match fresh.proof {
            Some(kit) => hidden.with_proof_kit(kit),
            None => hidden,
        }
    }

    fn probe(&self, kind: &ProbeKind) -> Option<ProbeProgram> {
        self.0.probe(kind)
    }
}

/// The cases that are skipped when the simulator's hooks are hidden: the ones that need a hook,
/// and the one it can never run.
const NEED_HOOKS: &[&str] = &[
    "lifecycle.slot_returned_once_after_ttl",
    "lifecycle.no_residue_after_destroy",
    "lifecycle.ttl_sweep",
    "idempotence.backend_busy_on_running_key",
    "idempotence.restart_replay",
    "idempotence.replay_after_destroy",
    "idempotence.marker_without_result",
    "idempotence.retention_covers_max_retention",
    "idempotence.key_kept_through_the_deadline",
    "secrets.empty_environment",
    "secrets.granted_secret_reaches_program_only",
    "secrets.foreign_reference_refused",
    "secrets.fewer_than_granted_fails_closed",
    "secrets.reresolved_on_replay",
    "paths.cwd_honoured",
    "paths.inputs_materialized",
    "paths.foreign_artifact_not_found",
    "tenants.stores_apart",
];

/// The only cases that run nothing in a sandbox, so need no probe.
const NEED_NO_PROBE: &[&str] = &[
    "tenants.no_shared_domain",
    "tenants.sandbox_ids_never_reused",
    "governance.unsafe_text",
    "bounds.malformed_requests_refused",
    "proof.signer_idempotent_by_key",
    "proof.signer_refuses_malformed",
    "proof.kind_signed",
    "proof.unpinned_keys_refused",
    "proof.chain_tamper_detected",
    "proof.mixed_chain_rejected",
];

fn ids_where(
    report: &ConformanceReport,
    wanted: impl Fn(&CaseOutcome) -> bool,
) -> Vec<&'static str> {
    report
        .cases
        .iter()
        .filter(|result| wanted(&result.outcome))
        .map(|result| result.case.id)
        .collect()
}

#[tokio::test]
async fn a_case_that_needs_a_hook_the_target_lacks_is_not_applicable_never_passed() {
    let report = run_conformance(&NoHooksTarget(SimTarget::new())).await;
    assert!(report.is_conformant(), "{:#?}", failures(&report));
    assert!(
        !report.is_complete(&[]),
        "the skipped cases are not accepted"
    );
    assert!(report.is_complete(NEED_HOOKS));
    let skipped = ids_where(&report, |o| matches!(o, CaseOutcome::NotApplicable(_)));
    assert_eq!(skipped, NEED_HOOKS);
    for result in report.not_applicable_cases() {
        let CaseOutcome::NotApplicable(reason) = &result.outcome else {
            unreachable!("filtered to the cases that were not applicable");
        };
        assert!(!reason.is_empty(), "{} gives no reason", result.case.id);
    }
}

#[tokio::test]
async fn a_case_that_needs_a_probe_the_target_lacks_is_not_applicable_never_passed() {
    let report = run_conformance(&NoProbes(SimTarget::new())).await;
    assert!(report.is_conformant(), "{:#?}", failures(&report));
    assert!(!report.is_complete(&[]), "almost every case was skipped");
    assert_eq!(report.to_json()["complete"], false);
    let passed = ids_where(&report, |o| matches!(o, CaseOutcome::Passed));
    assert_eq!(passed, NEED_NO_PROBE);
    assert_eq!(
        report.not_applicable(),
        report.cases.len() - NEED_NO_PROBE.len()
    );
}

#[tokio::test]
async fn the_report_is_serializable_and_holds_no_content_of_a_run() {
    let report = run_conformance(&SimTarget::new()).await;
    let json = report.to_json_accepting(NOT_APPLICABLE);
    assert_eq!(json["conformant"], true);
    assert_eq!(json["complete"], true);
    assert_eq!(
        report.to_json()["complete"],
        false,
        "the skipped case is not accepted"
    );
    assert_eq!(json["measured"], 1);
    assert_eq!(json["failed"], 0);
    assert_eq!(
        json["cases"].as_array().map(Vec::len),
        Some(catalogue().len())
    );
    let text = json.to_string();
    for content in ["cnf-s3cr3t", "unique-stdin-token", "hello", "s3cr3t-stdin"] {
        assert!(!text.contains(content), "{content} is in the report");
    }
}

#[tokio::test]
async fn the_catalogue_names_each_case_once_with_its_class_and_group() {
    let cases = catalogue();
    let ids: std::collections::HashSet<_> = cases.iter().map(|case| case.id).collect();
    assert_eq!(ids.len(), cases.len());
    for case in &cases {
        assert!(
            case.id.starts_with(&format!("{}.", case.group.name())),
            "{}",
            case.id
        );
        assert!(!case.description.is_empty(), "{}", case.id);
    }
    let classes: std::collections::HashSet<_> = cases.iter().map(|case| case.class).collect();
    assert_eq!(classes.len(), 3, "the three classes are all in use");
}

/// The simulator whose exec never answers.
struct Hangs(SimTarget);

struct HangingBackend(SimBackend);

#[async_trait]
impl SandboxBackend for HangingBackend {
    fn max_retention(&self) -> u64 {
        self.0.max_retention()
    }

    async fn create(
        &self,
        ctx: &SandboxContext,
        key: &IdempotencyKey,
        req: CreateRequest,
        deadline: u64,
        cancel: &CancelToken,
    ) -> Result<RawCreated, SandboxError> {
        self.0.create(ctx, key, req, deadline, cancel).await
    }

    async fn exec(
        &self,
        _: &SandboxHandle,
        _: &IdempotencyKey,
        _: ExecRequest,
        _: u64,
        _: &CancelToken,
    ) -> Result<ExecOutcome, SandboxError> {
        std::future::pending().await
    }

    async fn collect(
        &self,
        h: &SandboxHandle,
        key: &IdempotencyKey,
        req: CollectRequest,
        deadline: u64,
        cancel: &CancelToken,
    ) -> Result<RawCollected, SandboxError> {
        self.0.collect(h, key, req, deadline, cancel).await
    }

    async fn destroy(&self, h: &SandboxHandle, key: &IdempotencyKey) -> Result<(), SandboxError> {
        self.0.destroy(h, key).await
    }
}

#[async_trait]
impl ConformanceTarget for Hangs {
    type Backend = HangingBackend;

    fn name(&self) -> String {
        "simulator that hangs".to_owned()
    }

    async fn fresh(&self) -> Instance<HangingBackend> {
        let fresh = self.0.fresh().await;
        Instance::new(HangingBackend(fresh.backend), fresh.clock)
    }

    fn probe(&self, kind: &ProbeKind) -> Option<ProbeProgram> {
        self.0.probe(kind)
    }
}

#[tokio::test]
async fn a_backend_that_never_answers_fails_its_case_when_the_caller_sets_a_deadline() {
    let report = run_cases_with(
        &Hangs(SimTarget::new()),
        &["lifecycle.state_order", "bounds.malformed_requests_refused"],
        |_, case| {
            Box::pin(async move {
                tokio::time::timeout(std::time::Duration::from_millis(300), case)
                    .await
                    .unwrap_or_else(|_| CaseOutcome::Failed("timed out".to_owned()))
            })
        },
    )
    .await;
    assert_eq!(report.cases.len(), 2);
    assert_eq!(
        report.outcome_of("lifecycle.state_order"),
        Some(&CaseOutcome::Failed("timed out".to_owned()))
    );
    // A case that never runs the program is not held up.
    assert_eq!(
        report.outcome_of("bounds.malformed_requests_refused"),
        Some(&CaseOutcome::Passed)
    );
    assert!(!report.is_conformant());
}

/// A target whose name panics still gets a report.
struct NamelessPanic(SimTarget);

#[async_trait]
impl ConformanceTarget for NamelessPanic {
    type Backend = SimBackend;

    fn name(&self) -> String {
        panic!("no name")
    }

    async fn fresh(&self) -> Instance<SimBackend> {
        self.0.fresh().await
    }

    fn probe(&self, kind: &ProbeKind) -> Option<ProbeProgram> {
        self.0.probe(kind)
    }
}

#[tokio::test]
async fn a_name_that_panics_does_not_lose_the_report() {
    let report = run_cases(
        &NamelessPanic(SimTarget::new()),
        &["bounds.malformed_requests_refused"],
    )
    .await;
    assert_eq!(report.passed(), 1);
    assert!(!report.target.is_empty());
}
