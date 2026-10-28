//! The proof that the suite detects: backends with one deliberate defect each, and the cases
//! that must catch them. A case that cannot fail proves nothing, so each mutant names the
//! case that fails on it.

mod backend;

use ailu_sandbox::conformance::{run_conformance, CaseOutcome, ConformanceReport};
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
    ["idempotence.retention_covers_the_ceiling"]
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
    ["limits.output_truncated_and_marked"]
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
    an_artifact_is_available_to_every_tenant,
    Mutation::TenantBlindArtifacts,
    ["paths.foreign_artifact_not_found"]
);
