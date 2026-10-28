//! Request types of the sandbox port.
//!
//! All of them reject unknown fields: a request cannot carry a `tenant`, a volume mount or
//! any other field the contract does not define. There is deliberately no host-volume
//! variant anywhere in [`CreateRequest`].

use crate::limits::Limits;
use crate::path::LogicalPath;
use ailu_fs_backend::ArtifactRef;
use serde::{Deserialize, Serialize};

/// One allowed network destination. Exact host name only: no wildcard, no IP range.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostRule {
    pub host: String,
    pub port: u16,
}

/// Network access of a sandbox. Deny by default.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", content = "rules", rename_all = "camelCase")]
pub enum NetworkPolicy {
    #[default]
    Deny,
    Allow(Vec<HostRule>),
}

/// Opaque reference to a secret, resolved by the host on the backend side. Holds no value.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SecretRef(pub String);

/// Grant of one secret as one environment variable. Holds the name and the reference, never
/// the value.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnvGrant {
    pub name: String,
    pub secret_ref: SecretRef,
}

/// An input file: a versioned artifact of the current run, materialized at `dest`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InputRef {
    pub artifact: ArtifactRef,
    pub dest: LogicalPath,
}

/// Request to reserve a sandbox.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateRequest {
    /// Opaque isolation profile, resolved by the deployment.
    pub profile: String,
    #[serde(default)]
    pub limits: Limits,
    #[serde(default)]
    pub network: NetworkPolicy,
    #[serde(default)]
    pub inputs: Vec<InputRef>,
    #[serde(default)]
    pub env: Vec<EnvGrant>,
    /// Mandatory time-to-live, used as a sweep if the host crashes before `destroy`.
    pub ttl_ms: u64,
}

/// Request to run one program. Never a string interpreted by a shell.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecRequest {
    /// Program followed by its arguments; `argv[0]` is the program. Must not be empty.
    pub argv: Vec<String>,
    pub cwd: LogicalPath,
    #[serde(default)]
    pub stdin: Option<Vec<u8>>,
}

/// Request to collect output files, as glob patterns under the working directory.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CollectRequest {
    pub patterns: Vec<String>,
    pub max_files: u32,
    pub max_bytes: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn network_defaults_to_deny() {
        assert_eq!(NetworkPolicy::default(), NetworkPolicy::Deny);
        let req: CreateRequest =
            serde_json::from_value(json!({"profile": "p", "ttlMs": 1000})).unwrap();
        assert_eq!(req.network, NetworkPolicy::Deny);
        assert_eq!(req.limits, Limits::default());
        assert!(req.inputs.is_empty() && req.env.is_empty());
    }

    #[test]
    fn network_allow_round_trips() {
        let policy = NetworkPolicy::Allow(vec![HostRule {
            host: "api.example.com".to_owned(),
            port: 443,
        }]);
        let json = serde_json::to_value(&policy).unwrap();
        assert_eq!(json["mode"], "allow");
        assert_eq!(
            serde_json::from_value::<NetworkPolicy>(json).unwrap(),
            policy
        );
    }

    #[test]
    fn create_request_has_no_way_to_mount_a_host_volume() {
        let req = json!({"profile": "p", "ttlMs": 1, "mounts": [{"host": "/", "path": "/h"}]});
        assert!(serde_json::from_value::<CreateRequest>(req).is_err());
        let req = json!({"profile": "p", "ttlMs": 1, "volumes": ["/:/h"]});
        assert!(serde_json::from_value::<CreateRequest>(req).is_err());
    }

    #[test]
    fn requests_reject_an_agent_supplied_tenant() {
        let create = json!({"profile": "p", "ttlMs": 1, "tenantId": "other"});
        assert!(serde_json::from_value::<CreateRequest>(create).is_err());
        let exec = json!({"argv": ["ls"], "cwd": ".", "tenantId": "other"});
        assert!(serde_json::from_value::<ExecRequest>(exec).is_err());
        let collect = json!({"patterns": [], "maxFiles": 1, "maxBytes": 1, "tenant": "x"});
        assert!(serde_json::from_value::<CollectRequest>(collect).is_err());
    }

    #[test]
    fn exec_cwd_is_validated_while_deserializing() {
        let bad = json!({"argv": ["ls"], "cwd": "../etc"});
        assert!(serde_json::from_value::<ExecRequest>(bad).is_err());
        let ok = json!({"argv": ["ls"], "cwd": "work//out"});
        let req: ExecRequest = serde_json::from_value(ok).unwrap();
        assert_eq!(req.cwd.as_str(), "work/out");
        assert_eq!(req.stdin, None);
    }

    #[test]
    fn env_grant_carries_a_reference_not_a_value() {
        let grant: EnvGrant =
            serde_json::from_value(json!({"name": "API_KEY", "secretRef": "ref-1"})).unwrap();
        assert_eq!(grant.secret_ref, SecretRef("ref-1".to_owned()));
        let with_value = json!({"name": "API_KEY", "secretRef": "ref-1", "value": "hunter2"});
        assert!(serde_json::from_value::<EnvGrant>(with_value).is_err());
    }
}
