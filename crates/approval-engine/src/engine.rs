//! The approval engine: file requests, resolve them (no self-approval), query.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use ailu_graph_core::{NodeId, RunId};
use serde_json::Value;

use crate::error::ApprovalError;
use crate::types::{ApprovalId, ApprovalRequest, ApprovalStatus};

pub(crate) fn now_string() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
        .to_string()
}

pub struct RequestApprovalParams {
    pub run_id: RunId,
    pub node_id: NodeId,
    pub requested_by: String,
    pub subject: Value,
}

pub trait ApprovalEngine {
    fn request(&self, params: RequestApprovalParams) -> ApprovalRequest;
    fn approve(&self, id: &ApprovalId, resolved_by: &str)
        -> Result<ApprovalRequest, ApprovalError>;
    fn reject(
        &self,
        id: &ApprovalId,
        resolved_by: &str,
        reason: &str,
    ) -> Result<ApprovalRequest, ApprovalError>;
    fn get_pending(&self, run_id: Option<&RunId>) -> Vec<ApprovalRequest>;
    fn get_by_id(&self, id: &ApprovalId) -> Option<ApprovalRequest>;
}

#[derive(Default)]
pub struct InMemoryApprovalEngine {
    approvals: RefCell<HashMap<String, ApprovalRequest>>,
    seq: Cell<u64>,
}

impl InMemoryApprovalEngine {
    pub fn new() -> Self {
        Self::default()
    }

    fn ensure_can_resolve(
        request: &ApprovalRequest,
        resolved_by: &str,
    ) -> Result<(), ApprovalError> {
        if request.status != ApprovalStatus::Pending {
            return Err(ApprovalError::AlreadyResolved(request.id.0.clone()));
        }
        // An agent never approves its own request — the resolver must be a different
        // principal. This is the core governance invariant.
        if request.requested_by == resolved_by {
            return Err(ApprovalError::SelfApproval(request.id.0.clone()));
        }
        Ok(())
    }
}

impl ApprovalEngine for InMemoryApprovalEngine {
    fn request(&self, params: RequestApprovalParams) -> ApprovalRequest {
        let n = self.seq.get();
        self.seq.set(n + 1);
        let request = ApprovalRequest {
            id: ApprovalId(format!("approval-{n}")),
            run_id: params.run_id,
            node_id: params.node_id,
            requested_by: params.requested_by,
            subject: params.subject,
            status: ApprovalStatus::Pending,
            resolved_by: None,
            resolved_at: None,
            rejection_reason: None,
            created_at: now_string(),
        };
        self.approvals
            .borrow_mut()
            .insert(request.id.0.clone(), request.clone());
        request
    }

    fn approve(
        &self,
        id: &ApprovalId,
        resolved_by: &str,
    ) -> Result<ApprovalRequest, ApprovalError> {
        let mut approvals = self.approvals.borrow_mut();
        let request = approvals
            .get(id.0.as_str())
            .ok_or_else(|| ApprovalError::NotFound(id.0.clone()))?
            .clone();
        Self::ensure_can_resolve(&request, resolved_by)?;
        let resolved = ApprovalRequest {
            status: ApprovalStatus::Approved,
            resolved_by: Some(resolved_by.to_owned()),
            resolved_at: Some(now_string()),
            rejection_reason: None,
            ..request
        };
        approvals.insert(id.0.clone(), resolved.clone());
        Ok(resolved)
    }

    fn reject(
        &self,
        id: &ApprovalId,
        resolved_by: &str,
        reason: &str,
    ) -> Result<ApprovalRequest, ApprovalError> {
        let mut approvals = self.approvals.borrow_mut();
        let request = approvals
            .get(id.0.as_str())
            .ok_or_else(|| ApprovalError::NotFound(id.0.clone()))?
            .clone();
        Self::ensure_can_resolve(&request, resolved_by)?;
        let resolved = ApprovalRequest {
            status: ApprovalStatus::Rejected,
            resolved_by: Some(resolved_by.to_owned()),
            resolved_at: Some(now_string()),
            rejection_reason: Some(reason.to_owned()),
            ..request
        };
        approvals.insert(id.0.clone(), resolved.clone());
        Ok(resolved)
    }

    fn get_pending(&self, run_id: Option<&RunId>) -> Vec<ApprovalRequest> {
        self.approvals
            .borrow()
            .values()
            .filter(|request| request.status == ApprovalStatus::Pending)
            .filter(|request| run_id.map(|id| &request.run_id == id).unwrap_or(true))
            .cloned()
            .collect()
    }

    fn get_by_id(&self, id: &ApprovalId) -> Option<ApprovalRequest> {
        self.approvals.borrow().get(id.0.as_str()).cloned()
    }
}

#[cfg(test)]
mod tests {
    use ailu_graph_core::{NodeId, RunId};
    use serde_json::json;

    use super::*;

    fn params() -> RequestApprovalParams {
        RequestApprovalParams {
            run_id: RunId::from("run-1"),
            node_id: NodeId::from("assistant"),
            requested_by: "assistant".to_owned(),
            subject: json!({ "description": "tool:refund" }),
        }
    }

    #[test]
    fn files_and_lists_pending_requests() {
        let engine = InMemoryApprovalEngine::new();
        let request = engine.request(params());
        assert_eq!(request.status, ApprovalStatus::Pending);
        assert_eq!(engine.get_pending(Some(&RunId::from("run-1"))).len(), 1);
        assert_eq!(engine.get_pending(Some(&RunId::from("other"))).len(), 0);
    }

    #[test]
    fn approves_when_resolved_by_a_different_principal() {
        let engine = InMemoryApprovalEngine::new();
        let request = engine.request(params());
        let resolved = engine.approve(&request.id, "alice").unwrap();
        assert_eq!(resolved.status, ApprovalStatus::Approved);
        assert_eq!(resolved.resolved_by.as_deref(), Some("alice"));
        assert!(engine.get_pending(None).is_empty());
    }

    #[test]
    fn forbids_self_approval() {
        let engine = InMemoryApprovalEngine::new();
        let request = engine.request(params());
        // requested_by == "assistant"
        assert_eq!(
            engine.approve(&request.id, "assistant"),
            Err(ApprovalError::SelfApproval(request.id.0.clone()))
        );
    }

    #[test]
    fn rejects_unknown_and_double_resolution() {
        let engine = InMemoryApprovalEngine::new();
        assert!(matches!(
            engine.approve(&ApprovalId("nope".to_owned()), "alice"),
            Err(ApprovalError::NotFound(_))
        ));
        let request = engine.request(params());
        engine.approve(&request.id, "alice").unwrap();
        assert!(matches!(
            engine.reject(&request.id, "bob", "too late"),
            Err(ApprovalError::AlreadyResolved(_))
        ));
    }
}
