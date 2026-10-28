//! Execution limits and request bounds.

use crate::error::LimitField;
use serde::{Deserialize, Serialize};

const MIB: u64 = 1024 * 1024;

/// Resource limits of one sandbox. The default is deliberately low.
///
/// The ceiling comes from the deployment, never from the agent: a request above it is
/// refused, not silently clamped.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Limits {
    /// CPU share in millicores (1000 = one core).
    pub cpu_millicores: u32,
    pub memory_bytes: u64,
    pub wall_time_ms: u64,
    /// Output cap per stream.
    pub output_bytes: u64,
    pub max_processes: u32,
    pub ephemeral_disk_bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            cpu_millicores: 250,
            memory_bytes: 256 * MIB,
            wall_time_ms: 30_000,
            output_bytes: MIB,
            max_processes: 32,
            ephemeral_disk_bytes: 64 * MIB,
        }
    }
}

impl Limits {
    /// First field that is zero, if any. A program cannot run under a zero limit.
    pub fn first_zero(&self) -> Option<LimitField> {
        self.fields()
            .into_iter()
            .find(|(_, value)| *value == 0)
            .map(|(field, _)| field)
    }

    /// First field strictly above the same field of `ceiling`, if any.
    pub fn first_above(&self, ceiling: &Limits) -> Option<LimitField> {
        self.fields()
            .into_iter()
            .zip(ceiling.fields())
            .find(|((_, requested), (_, allowed))| requested > allowed)
            .map(|((field, _), _)| field)
    }

    fn fields(&self) -> [(LimitField, u64); 6] {
        [
            (LimitField::CpuMillicores, u64::from(self.cpu_millicores)),
            (LimitField::MemoryBytes, self.memory_bytes),
            (LimitField::WallTimeMs, self.wall_time_ms),
            (LimitField::OutputBytes, self.output_bytes),
            (LimitField::MaxProcesses, u64::from(self.max_processes)),
            (LimitField::EphemeralDiskBytes, self.ephemeral_disk_bytes),
        ]
    }
}

/// Bounds on the *request itself* (not on what the program may consume). Deployment-set
/// ceilings; every request outside them is refused before any backend is reached.
///
/// These bounds may be wider than what an approver can read: on top of them, a call with more
/// than 32 arguments, or an argument or displayed field over 256 characters, is refused as
/// `unreviewable_call` (see the approval module).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestBounds {
    pub max_argv_count: u32,
    pub max_argv_bytes: u32,
    pub max_stdin_bytes: u32,
    /// v1: one `exec` per sandbox. Not checked by the pure validators (they see a single
    /// request); the guard enforces it from the sandbox state.
    pub max_exec_per_sandbox: u32,
    pub max_collect_patterns: u32,
    pub max_collect_files: u32,
    pub max_collect_bytes: u64,
    pub max_ttl_ms: u64,
    pub max_input_bytes: u64,
    pub max_input_files: u32,
}

impl Default for RequestBounds {
    fn default() -> Self {
        Self {
            max_argv_count: 64,
            max_argv_bytes: 32 * 1024,
            max_stdin_bytes: 64 * 1024,
            max_exec_per_sandbox: 1,
            max_collect_patterns: 8,
            max_collect_files: 256,
            max_collect_bytes: 16 * MIB,
            max_ttl_ms: 15 * 60 * 1000,
            max_input_bytes: 4 * MIB,
            max_input_files: 64,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_limits_are_low() {
        let l = Limits::default();
        assert_eq!(l.cpu_millicores, 250);
        assert_eq!(l.memory_bytes, 256 * 1024 * 1024);
        assert_eq!(l.wall_time_ms, 30_000);
        assert_eq!(l.output_bytes, 1024 * 1024);
        assert_eq!(l.max_processes, 32);
        assert_eq!(l.ephemeral_disk_bytes, 64 * 1024 * 1024);
        assert_eq!(l.first_zero(), None);
    }

    #[test]
    fn default_bounds_match_the_contract_proposal() {
        let b = RequestBounds::default();
        assert_eq!(b.max_argv_count, 64);
        assert_eq!(b.max_argv_bytes, 32 * 1024);
        assert_eq!(b.max_stdin_bytes, 64 * 1024);
        assert_eq!(b.max_exec_per_sandbox, 1);
        assert_eq!(b.max_collect_patterns, 8);
        assert_eq!(b.max_collect_files, 256);
        assert_eq!(b.max_collect_bytes, 16 * 1024 * 1024);
        assert_eq!(b.max_ttl_ms, 900_000);
        assert_eq!(b.max_input_bytes, 4 * 1024 * 1024);
    }

    #[test]
    fn first_above_names_the_offending_field() {
        let ceiling = Limits::default();
        assert_eq!(Limits::default().first_above(&ceiling), None);
        let over = Limits {
            wall_time_ms: ceiling.wall_time_ms + 1,
            ..Limits::default()
        };
        assert_eq!(over.first_above(&ceiling), Some(LimitField::WallTimeMs));
        let lower = Limits {
            memory_bytes: 1,
            ..Limits::default()
        };
        assert_eq!(lower.first_above(&ceiling), None);
    }

    #[test]
    fn first_zero_names_the_offending_field() {
        let zero = Limits {
            max_processes: 0,
            ..Limits::default()
        };
        assert_eq!(zero.first_zero(), Some(LimitField::MaxProcesses));
    }

    #[test]
    fn limits_reject_unknown_fields() {
        let json = r#"{"cpuMillicores":1,"memoryBytes":1,"wallTimeMs":1,"outputBytes":1,
            "maxProcesses":1,"ephemeralDiskBytes":1,"gpu":1}"#;
        assert!(serde_json::from_str::<Limits>(json).is_err());
    }
}
