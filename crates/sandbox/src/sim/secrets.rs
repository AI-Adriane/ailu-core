//! An in-memory secret store for the simulated backend.

use crate::ids::{SandboxContext, TenantId};
use crate::request::SecretRef;
use crate::secret::{ResolvedSecret, SecretError, SecretResolver};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};
use zeroize::Zeroizing;

/// A [`SecretResolver`] over a table the test fills: `(tenant, reference) -> value`.
///
/// A reference is only ever visible to the tenant it was stored for; asking for it as another
/// tenant is `NotFound`, exactly like an unknown reference. The values are wiped when the
/// store is dropped, and `Debug` shows counts only.
#[derive(Default)]
pub struct SimSecrets {
    values: Mutex<HashMap<(TenantId, SecretRef), Zeroizing<String>>>,
    unavailable: AtomicBool,
    resolutions: AtomicU64,
}

impl SimSecrets {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Store `value` under `reference` for `tenant`.
    pub fn set(&self, tenant: &TenantId, reference: &str, value: &str) {
        self.values
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(
                (tenant.clone(), SecretRef(reference.to_owned())),
                Zeroizing::new(value.to_owned()),
            );
    }

    /// Make the store answer `Unavailable` (or answer again).
    pub fn set_unavailable(&self, unavailable: bool) {
        self.unavailable.store(unavailable, Ordering::SeqCst);
    }

    /// How many successful resolutions were made.
    pub fn resolutions(&self) -> u64 {
        self.resolutions.load(Ordering::Relaxed)
    }
}

#[async_trait]
impl SecretResolver for SimSecrets {
    async fn resolve(
        &self,
        ctx: &SandboxContext,
        reference: &SecretRef,
    ) -> Result<ResolvedSecret, SecretError> {
        if self.unavailable.load(Ordering::SeqCst) {
            return Err(SecretError::Unavailable);
        }
        let values = self.values.lock().unwrap_or_else(PoisonError::into_inner);
        let value = values
            .get(&(ctx.tenant_id.clone(), reference.clone()))
            .ok_or(SecretError::NotFound)?;
        self.resolutions.fetch_add(1, Ordering::Relaxed);
        Ok(ResolvedSecret::new(value.as_str()))
    }
}

impl std::fmt::Debug for SimSecrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SimSecrets")
            .field(
                "stored",
                &self
                    .values
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .len(),
            )
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ailu_graph_core::{NodeId, RunId};

    fn ctx(tenant: &str) -> SandboxContext {
        SandboxContext {
            tenant_id: TenantId::new(tenant),
            run_id: RunId::from("run"),
            node_id: NodeId::from("node"),
            attempt: 1,
        }
    }

    #[tokio::test]
    async fn a_reference_belongs_to_its_tenant_only() {
        let store = SimSecrets::new();
        store.set(&TenantId::new("a"), "db", "s3cr3t-value");
        let reference = SecretRef("db".to_owned());
        let value = store.resolve(&ctx("a"), &reference).await.unwrap();
        assert_eq!(value.expose(), "s3cr3t-value");
        assert_eq!(
            store.resolve(&ctx("b"), &reference).await.err(),
            Some(SecretError::NotFound),
            "another tenant's reference looks unknown"
        );
        assert_eq!(
            store
                .resolve(&ctx("a"), &SecretRef("nope".to_owned()))
                .await
                .err(),
            Some(SecretError::NotFound)
        );
        store.set_unavailable(true);
        assert_eq!(
            store.resolve(&ctx("a"), &reference).await.err(),
            Some(SecretError::Unavailable)
        );
        assert_eq!(store.resolutions(), 1);
    }

    #[test]
    fn debug_shows_a_count_only() {
        let store = SimSecrets::new();
        store.set(&TenantId::new("a"), "ref-name", "s3cr3t-value");
        let shown = format!("{store:?}");
        assert!(
            !shown.contains("s3cr3t") && !shown.contains("ref-name"),
            "{shown}"
        );
    }
}
