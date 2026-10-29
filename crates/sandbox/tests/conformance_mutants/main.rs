//! The proof that the suite detects: backends with one deliberate defect each, and the cases
//! that must catch them. A case that cannot fail proves nothing, so each mutant names the
//! case that fails on it.

mod backend;

use ailu_sandbox::conformance::{run_conformance, CaseOutcome, ConformanceReport, SimTarget};
use ailu_sandbox::LimitKind;
use backend::{MutantTarget, Mutation};

fn failed(report: &ConformanceReport) -> Vec<&'static str> {
    report
        .cases
        .iter()
        .filter(|result| matches!(result.outcome, CaseOutcome::Failed(_)))
        .map(|result| result.case.id)
        .collect()
}

/// Run the suite on the mutant and require that `expected` fail on it.
async fn assert_caught(mutation: Mutation, expected: &[&str]) {
    let report = run_conformance(&MutantTarget::new(mutation)).await;
    let caught = failed(&report);
    assert!(!report.is_conformant(), "{mutation:?} was not detected");
    for id in expected {
        assert!(
            caught.contains(id),
            "{mutation:?} was not caught by {id}; failed: {caught:?}"
        );
    }
}

macro_rules! mutant {
    ($name:ident, $mutation:expr, [$($id:literal),+ $(,)?]) => {
        #[tokio::test]
        async fn $name() {
            assert_caught($mutation, &[$($id),+]).await;
        }
    };
}

// ---- lifecycle ------------------------------------------------------------------------

mutant!(
    a_second_exec_is_accepted,
    Mutation::SecondExecAllowed,
    ["lifecycle.backend_second_exec"]
);
mutant!(
    a_second_collect_is_accepted,
    Mutation::SecondCollectAllowed,
    ["lifecycle.backend_second_collect"]
);
mutant!(
    a_second_destroy_is_an_error,
    Mutation::DestroyNotIdempotent,
    ["lifecycle.destroy_idempotent"]
);
mutant!(
    destroy_releases_nothing,
    Mutation::DestroyKeepsResources,
    ["lifecycle.no_residue_after_destroy"]
);
mutant!(
    nothing_is_swept_after_the_ttl,
    Mutation::NeverSweeps,
    ["lifecycle.ttl_sweep"]
);

// ---- idempotence ----------------------------------------------------------------------

mutant!(
    another_request_under_the_same_key_gets_the_first_answer,
    Mutation::NoConflictCheck,
    ["idempotence.backend_conflict_in_every_state"]
);
mutant!(
    a_replay_runs_the_program_again,
    Mutation::RerunOnReplay,
    [
        "idempotence.restart_replay",
        "idempotence.replay_after_destroy"
    ]
);
mutant!(
    a_replay_answers_with_another_result,
    Mutation::ReplayChangesResult,
    ["idempotence.restart_replay"]
);
mutant!(
    no_marker_is_written,
    Mutation::NoMarker,
    ["idempotence.marker_without_result"]
);
mutant!(
    the_marker_is_ignored_after_a_crash,
    Mutation::RerunsAfterCrash,
    ["idempotence.marker_without_result"]
);
mutant!(
    answers_are_forgotten_after_a_day,
    Mutation::ForgetsAfterADay,
    ["idempotence.retention_covers_max_retention"]
);
mutant!(
    a_key_is_forgotten_at_its_deadline_itself,
    Mutation::ForgetsAtTheDeadline,
    ["idempotence.key_kept_through_the_deadline"]
);
mutant!(
    a_later_deadline_on_a_repeated_key_is_served,
    Mutation::AcceptsLaterDeadlineOnRepeat,
    ["idempotence.deadline_sealed_at_first_call"]
);
mutant!(
    the_store_is_not_kept_per_tenant,
    Mutation::TenantBlindStore,
    ["tenants.key_of_another_tenant_refused"]
);
mutant!(
    the_lookup_follows_the_key_not_the_context,
    Mutation::IgnoresContextTenant,
    ["tenants.key_of_another_tenant_refused"]
);
mutant!(
    a_shorter_deadline_on_a_repeated_key_is_refused,
    Mutation::RefusesShorterDeadline,
    ["idempotence.deadline_sealed_at_first_call"]
);
mutant!(
    a_deadline_beyond_what_the_backend_keeps_is_sealed,
    Mutation::SealsBeyondMaxRetention,
    ["idempotence.deadline_beyond_max_retention_refused"]
);
mutant!(
    the_declared_retention_shrinks_while_the_backend_runs,
    Mutation::DeclaredRetentionDrifts,
    ["idempotence.max_retention_declared"]
);

// ---- limits ---------------------------------------------------------------------------

mutant!(
    the_wall_time_limit_is_not_enforced,
    Mutation::LimitIgnored(LimitKind::WallTime),
    ["limits.wall_time"]
);
mutant!(
    the_memory_limit_is_not_enforced,
    Mutation::LimitIgnored(LimitKind::Memory),
    ["limits.memory"]
);
mutant!(
    the_process_limit_is_not_enforced,
    Mutation::LimitIgnored(LimitKind::Processes),
    ["limits.processes"]
);
mutant!(
    the_disk_limit_is_not_enforced,
    Mutation::LimitIgnored(LimitKind::Disk),
    ["limits.disk"]
);
mutant!(
    truncated_output_is_not_marked,
    Mutation::OutputNotMarked,
    [
        "limits.output_truncated_and_marked",
        "delivery.backend_truncates_at_bounds"
    ]
);

// ---- network --------------------------------------------------------------------------

mutant!(
    the_allow_list_is_not_applied,
    Mutation::AllowListIgnored,
    [
        "network.default_deny",
        "network.dns_denied_by_default",
        "network.off_list_host",
        "network.redirect_off_list"
    ]
);
mutant!(
    the_network_is_closed_whatever_was_asked,
    Mutation::RefusesEverything,
    ["network.allowed_host_reaches"]
);
mutant!(
    a_private_address_is_reached,
    Mutation::Reaches("private.example.com"),
    ["network.private_ipv4", "network.dns_private_resolution"]
);
mutant!(
    a_loopback_address_is_reached,
    Mutation::Reaches("loopback.example.com"),
    ["network.loopback"]
);
mutant!(
    a_link_local_address_is_reached,
    Mutation::Reaches("linklocal.example.com"),
    ["network.link_local"]
);
mutant!(
    a_unique_local_ipv6_address_is_reached,
    Mutation::Reaches("ula.example.com"),
    ["network.ipv6_unique_local"]
);
mutant!(
    a_documentation_ipv6_address_is_reached,
    Mutation::Reaches("doc6.example.com"),
    ["network.ipv6_special_block"]
);
mutant!(
    a_translated_ipv6_address_is_reached,
    Mutation::Reaches("nat64.example.com"),
    ["network.ipv6_translated"]
);
mutant!(
    a_mapped_private_address_is_reached,
    Mutation::Reaches("mapped.example.com"),
    ["network.ipv4_mapped_private"]
);
mutant!(
    a_mapped_public_address_is_reached,
    Mutation::Reaches("mappedpub.example.com"),
    ["network.ipv4_mapped_public"]
);
mutant!(
    the_ipv4_metadata_endpoint_is_reached,
    Mutation::Reaches("meta4.example.com"),
    ["network.metadata_ipv4"]
);
mutant!(
    the_ipv6_metadata_endpoint_is_reached,
    Mutation::Reaches("meta6.example.com"),
    ["network.metadata_ipv6"]
);
mutant!(
    a_rebinding_name_is_reached,
    Mutation::Reaches("rebind.example.com"),
    ["network.dns_rebinding"]
);
mutant!(
    a_probe_that_crashes_is_not_a_refusal,
    Mutation::ProbeCrashes,
    [
        "network.off_list_host",
        "network.private_ipv4",
        "network.default_deny"
    ]
);
mutant!(
    a_name_that_rebinds_late_is_reached,
    Mutation::Reaches("rebind3.example.com"),
    ["network.dns_rebinding_late"]
);
mutant!(
    any_port_of_an_allowed_host_is_reached,
    Mutation::PortIgnored,
    ["network.port_not_on_list"]
);
mutant!(
    a_chain_of_redirects_has_no_end,
    Mutation::Reaches("loop.example.com"),
    ["network.redirect_limit"]
);

// ---- secrets --------------------------------------------------------------------------

mutant!(
    host_variables_are_inherited,
    Mutation::InheritsHostEnvironment,
    ["secrets.empty_environment"]
);
mutant!(
    resolved_secrets_are_not_handed_back,
    Mutation::SecretsNotHandedBack,
    [
        "secrets.granted_secret_reaches_program_only",
        "secrets.reresolved_on_replay"
    ]
);
mutant!(
    secrets_are_not_resolved_again_on_replay,
    Mutation::NoSecretsOnReplay,
    ["secrets.reresolved_on_replay"]
);
mutant!(
    a_secret_reference_resolves_for_every_tenant,
    Mutation::TenantBlindSecrets,
    ["secrets.foreign_reference_refused"]
);

// ---- paths ----------------------------------------------------------------------------

mutant!(
    an_outgoing_link_is_followed,
    Mutation::OutgoingLinkFollowed,
    ["paths.outgoing_link_refused"]
);
mutant!(
    a_link_two_directories_down_is_followed,
    Mutation::DeepLinkFollowed,
    ["paths.outgoing_link_deep_refused"]
);
mutant!(
    an_artifact_is_available_to_every_tenant,
    Mutation::TenantBlindArtifacts,
    ["paths.foreign_artifact_not_found"]
);

// ---- tenants --------------------------------------------------------------------------

mutant!(
    every_sandbox_gets_the_same_isolation_domain,
    Mutation::SharedDomain,
    ["tenants.no_shared_domain"]
);
mutant!(
    a_destroyed_domain_goes_to_another_tenant,
    Mutation::RecycledDomain,
    ["tenants.no_shared_domain"]
);
mutant!(
    a_sandbox_id_is_handed_out_twice,
    Mutation::ReusedSandboxId,
    ["tenants.sandbox_ids_never_reused"]
);
mutant!(
    a_sandbox_is_found_by_its_id_whoever_asks,
    Mutation::IgnoresTenantOnLookup,
    ["tenants.backend_not_found"]
);

// ---- delivery -------------------------------------------------------------------------

mutant!(
    a_declared_digest_is_stale,
    Mutation::StaleDigest,
    ["delivery.backend_digests_match"]
);
mutant!(
    a_file_outside_the_patterns_is_delivered,
    Mutation::FileOutsidePatterns,
    ["delivery.backend_paths_match_patterns"]
);
mutant!(
    a_path_is_delivered_twice,
    Mutation::DuplicatePath,
    ["delivery.backend_paths_match_patterns"]
);
mutant!(
    a_delivery_goes_past_every_bound,
    Mutation::PastTheBounds,
    ["delivery.backend_truncates_at_bounds"]
);

#[tokio::test]
async fn the_report_of_a_failing_secret_handling_holds_no_secret() {
    for mutation in [Mutation::SecretsNotHandedBack, Mutation::TenantBlindSecrets] {
        let report = run_conformance(&MutantTarget::new(mutation)).await;
        assert!(!report.is_conformant(), "{mutation:?}");
        let text = format!("{} {:?}", report.to_json(), report);
        assert!(
            !text.contains("cnf-s3cr3t"),
            "the report of {mutation:?} holds a secret value"
        );
    }
}

// ---- not a defect ---------------------------------------------------------------------

#[tokio::test]
async fn a_backend_that_gives_the_executor_back_still_passes_every_case() {
    let baseline = run_conformance(&SimTarget::new()).await;
    let report = run_conformance(&MutantTarget::new(Mutation::YieldsOnce)).await;
    assert!(report.is_conformant(), "failed: {:?}", failed(&report));
    assert_eq!(report.passed(), baseline.passed());
    assert_eq!(report.not_applicable(), baseline.not_applicable());
}

mutant!(
    the_files_are_not_delivered,
    Mutation::DropsFiles,
    [
        "delivery.success_output_delivered",
        "delivery.backend_digests_match",
        "delivery.backend_paths_match_patterns"
    ]
);
mutant!(
    the_standard_output_is_not_delivered,
    Mutation::DropsOutput,
    [
        "delivery.success_output_delivered",
        "delivery.backend_digests_match"
    ]
);

// ---- what the backend receives ---------------------------------------------------------

mutant!(
    the_working_directory_is_ignored,
    Mutation::IgnoresCwd,
    ["paths.cwd_honoured"]
);
mutant!(
    the_inputs_are_not_materialized,
    Mutation::SkipsInputs,
    ["paths.inputs_materialized"]
);
mutant!(
    the_exec_digest_leaves_out_the_working_directory,
    Mutation::DigestIgnoresCwd,
    ["idempotence.backend_digest_covers_the_request"]
);
mutant!(
    the_create_digest_leaves_out_the_secret_grants,
    Mutation::DigestIgnoresEnv,
    ["idempotence.backend_digest_covers_the_request"]
);
mutant!(
    domains_come_from_a_name_prefix,
    Mutation::DomainFromTenantPrefix8,
    ["tenants.no_shared_domain"]
);

mutant!(
    the_cancel_token_is_never_consulted,
    Mutation::IgnoresCancel,
    ["idempotence.backend_consults_cancel_token"]
);

#[tokio::test]
async fn a_case_that_panics_is_reported_failed_and_the_others_still_run() {
    let report = run_conformance(&MutantTarget::new(Mutation::PanicsOnCreate)).await;
    assert!(!report.is_conformant(), "a panic was not reported");
    assert_eq!(
        report.cases.len(),
        ailu_sandbox::conformance::catalogue().len()
    );
    let panicked = report
        .cases
        .iter()
        .filter(|result| matches!(&result.outcome, CaseOutcome::Failed(why) if why == "the case panicked"))
        .count();
    assert!(panicked > 0, "no case was reported as panicked");
    assert!(
        report.passed() > 0,
        "the cases that do not reach the backend were lost"
    );
}
