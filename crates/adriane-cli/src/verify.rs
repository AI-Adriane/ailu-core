//! `adriane verify <bundle.json>` — offline verification of a proof bundle
//! exported by the control plane (`POST /runs/:runId/attestations/export`,
//! `AttestationExport` in `product/apps/api/src/approvals/approvals.service.ts`).
//!
//! No network call, no API, no trust in the exporter: this reads the same
//! Ed25519 chain the control plane signed, using the public keys the bundle
//! itself carries, and recomputes every hash and signature locally via
//! [`adriane_approval_engine::attestation`]. An auditor with only the bundle
//! file can run this and reach the same verdict the Studio shows.

use serde::Deserialize;

use adriane_approval_engine::attestation::{verify_attestation, AttestationRecord};

/// The bundle shape written by `exportAttestationChain` (camelCase over the wire).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Bundle {
    pub run_id: String,
    pub tenant_id: String,
    pub exported_at: String,
    /// The exporter's own verdict at export time — reported for comparison, never trusted
    /// in place of recomputing below.
    pub verified: bool,
    pub records: Vec<AttestationRecord>,
}

/// Where a chain stopped verifying, if it did. Distinguishes a broken link (the exporter
/// reordered or dropped a record) from a bad signature (a record was tampered with).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Divergence {
    /// Record `index` (0-based) does not chain from the previous record's `payloadHash`.
    BrokenLink { index: usize },
    /// Record `index` fails its own hash/signature check.
    InvalidSignature { index: usize },
}

/// The full local verdict on a bundle's chain. Pure — no I/O, so it is exhaustively
/// unit-tested without touching the filesystem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyReport {
    pub record_count: usize,
    pub divergence: Option<Divergence>,
}

impl VerifyReport {
    pub fn is_valid(&self) -> bool {
        self.divergence.is_none()
    }
}

/// Recompute the chain's validity record by record, reporting the first point of
/// divergence instead of a single bool (unlike `adriane_approval_engine::attestation::verify_chain`)
/// so a human can be pointed at exactly what broke.
pub fn verify_records(records: &[AttestationRecord]) -> VerifyReport {
    let mut prev: Option<&str> = None;
    for (index, record) in records.iter().enumerate() {
        if record.prev_hash.as_deref() != prev {
            return VerifyReport {
                record_count: records.len(),
                divergence: Some(Divergence::BrokenLink { index }),
            };
        }
        if !verify_attestation(record) {
            return VerifyReport {
                record_count: records.len(),
                divergence: Some(Divergence::InvalidSignature { index }),
            };
        }
        prev = Some(&record.payload_hash);
    }
    VerifyReport {
        record_count: records.len(),
        divergence: None,
    }
}

/// Parse a bundle file's contents. Pure (text in, `Bundle` or message out) for testing.
pub fn parse_bundle(json: &str) -> Result<Bundle, String> {
    serde_json::from_str(json).map_err(|error| format!("not a valid attestation bundle: {error}"))
}

/// Human-readable report for stdout/stderr. Pure so its exact wording is tested.
pub fn format_report(bundle: &Bundle, report: &VerifyReport) -> String {
    let mut out = format!(
        "run {} · tenant {} · exported {}\n{} record(s)\n",
        bundle.run_id, bundle.tenant_id, bundle.exported_at, report.record_count
    );
    match &report.divergence {
        None => {
            out.push_str("chain OK — every record verifies and links to the one before it.\n");
            if !bundle.verified {
                out.push_str(
                    "note: the bundle's own `verified` field said false; this local check disagrees and wins.\n",
                );
            }
        }
        Some(Divergence::BrokenLink { index }) => {
            out.push_str(&format!(
                "chain BROKEN at record {index}: its prevHash does not match the previous record's payloadHash (reordered, dropped, or inserted record).\n"
            ));
        }
        Some(Divergence::InvalidSignature { index }) => {
            out.push_str(&format!(
                "chain BROKEN at record {index}: signature or content hash does not verify (tampered record).\n"
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use adriane_approval_engine::attestation::Ed25519Attestor;
    use adriane_approval_engine::types::{ApprovalId, ApprovalRequest, ApprovalStatus};
    use adriane_graph_core::{NodeId, RunId};
    use serde_json::json;

    use super::*;

    fn resolved(id: &str) -> ApprovalRequest {
        ApprovalRequest {
            id: ApprovalId(id.to_owned()),
            run_id: RunId::from("run_7f3a9c21e4b0"),
            node_id: NodeId::from("review"),
            requested_by: "review".to_owned(),
            subject: json!({ "description": "tool:stripe.refund" }),
            status: ApprovalStatus::Approved,
            resolved_by: Some("user:carole".to_owned()),
            resolved_at: Some("2026-09-10T14:20:41Z".to_owned()),
            rejection_reason: None,
            created_at: "2026-09-10T14:18:07Z".to_owned(),
        }
    }

    fn bundle_json(records: &[AttestationRecord]) -> String {
        serde_json::to_string(&json!({
            "runId": "run_7f3a9c21e4b0",
            "tenantId": "acme-eu",
            "exportedAt": "2026-09-10T14:33:10Z",
            "verified": true,
            "records": records,
        }))
        .unwrap()
    }

    #[test]
    fn parses_a_real_export_shape() {
        let attestor = Ed25519Attestor::generate();
        let record = attestor.attest(&resolved("apr_1"), None).unwrap();
        let bundle = parse_bundle(&bundle_json(&[record])).expect("parses");
        assert_eq!(bundle.run_id, "run_7f3a9c21e4b0");
        assert_eq!(bundle.tenant_id, "acme-eu");
        assert_eq!(bundle.records.len(), 1);
    }

    #[test]
    fn rejects_garbage_with_a_clear_message() {
        let error = parse_bundle("{\"not\":\"a bundle\"}").unwrap_err();
        assert!(error.contains("not a valid attestation bundle"), "{error}");
    }

    #[test]
    fn verifies_a_real_two_record_chain() {
        let attestor = Ed25519Attestor::generate();
        let first = attestor.attest(&resolved("apr_1"), None).unwrap();
        let second = attestor
            .attest(&resolved("apr_2"), Some(&first.payload_hash))
            .unwrap();
        let report = verify_records(&[first, second]);
        assert!(report.is_valid());
        assert_eq!(report.record_count, 2);
    }

    #[test]
    fn detects_a_broken_link_reported_at_the_right_index() {
        let attestor = Ed25519Attestor::generate();
        let first = attestor.attest(&resolved("apr_1"), None).unwrap();
        let orphan = attestor
            .attest(&resolved("apr_2"), Some("not-the-real-prev-hash"))
            .unwrap();
        let report = verify_records(&[first, orphan]);
        assert_eq!(report.divergence, Some(Divergence::BrokenLink { index: 1 }));
    }

    #[test]
    fn detects_tampering_as_an_invalid_signature_not_a_broken_link() {
        let attestor = Ed25519Attestor::generate();
        let mut record = attestor.attest(&resolved("apr_1"), None).unwrap();
        record.view.resolved_by = "mallory".to_owned(); // signed payload changed after signing
        let report = verify_records(&[record]);
        assert_eq!(
            report.divergence,
            Some(Divergence::InvalidSignature { index: 0 })
        );
    }

    #[test]
    fn an_empty_bundle_is_trivially_valid() {
        assert!(verify_records(&[]).is_valid());
    }

    #[test]
    fn format_report_names_the_run_and_the_verdict() {
        let attestor = Ed25519Attestor::generate();
        let record = attestor.attest(&resolved("apr_1"), None).unwrap();
        let bundle = parse_bundle(&bundle_json(&[record])).unwrap();
        let report = verify_records(&bundle.records);
        let text = format_report(&bundle, &report);
        assert!(text.contains("run_7f3a9c21e4b0"));
        assert!(text.contains("chain OK"));
    }

    #[test]
    fn format_report_flags_a_broken_link_by_index() {
        let attestor = Ed25519Attestor::generate();
        let first = attestor.attest(&resolved("apr_1"), None).unwrap();
        let orphan = attestor.attest(&resolved("apr_2"), Some("wrong")).unwrap();
        let bundle = parse_bundle(&bundle_json(&[first, orphan])).unwrap();
        let report = verify_records(&bundle.records);
        let text = format_report(&bundle, &report);
        assert!(text.contains("BROKEN at record 1"));
    }

    #[test]
    fn format_report_flags_a_disagreement_with_the_exporters_own_verdict() {
        let attestor = Ed25519Attestor::generate();
        let record = attestor.attest(&resolved("apr_1"), None).unwrap();
        let mut raw: serde_json::Value = serde_json::from_str(&bundle_json(&[record])).unwrap();
        raw["verified"] = json!(false); // exporter said invalid; our recompute says valid
        let bundle: Bundle = serde_json::from_value(raw).unwrap();
        let report = verify_records(&bundle.records);
        assert!(report.is_valid());
        assert!(format_report(&bundle, &report).contains("this local check disagrees and wins"));
    }
}
