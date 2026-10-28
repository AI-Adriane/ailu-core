//! Limits: a program that goes past a limit is killed with a typed error, its output is cut at
//! the bound and marked, and a request above the deployment ceiling is refused, never clamped.

use super::{case, CaseDef};
use crate::conformance::probe_limits;
use crate::conformance::support::{ensure, shape, Env, Lab, Must, Verdict};
use crate::conformance::target::ProbeKind;
use crate::error::{LimitField, LimitKind, PolicyReason, SandboxError};
use crate::journal::SandboxEventKind;
use crate::limits::Limits;
use crate::outcome::Truncation;
use crate::policy::SandboxPolicy;

pub(super) fn cases() -> Vec<CaseDef> {
    vec![
        case!(
            Limits,
            "limits.wall_time",
            RealBackend,
            "a program that runs past the wall-time limit is killed with limit_exceeded.wall_time",
            wall_time
        ),
        case!(
            Limits,
            "limits.memory",
            RealBackend,
            "a program that uses more than the memory limit is killed with limit_exceeded.memory",
            memory
        ),
        case!(
            Limits,
            "limits.processes",
            RealBackend,
            "a program that starts too many processes is killed with limit_exceeded.processes",
            processes
        ),
        case!(
            Limits,
            "limits.disk",
            RealBackend,
            "a program that fills the ephemeral disk is killed with limit_exceeded.disk",
            disk
        ),
        case!(
            Limits,
            "limits.output_truncated_and_marked",
            RealBackend,
            "output past its bound is cut and marked as truncated, not an error",
            output_truncated_and_marked
        ),
        case!(
            Limits,
            "limits.ceiling_refused_not_clamped",
            Guard,
            "a request above the deployment ceiling is refused and one at it reaches the backend unchanged",
            ceiling_refused_not_clamped
        ),
    ]
}

/// Run `probe` under the probe limits and require that it is killed for `kind`.
async fn killed_for(lab: &Lab<'_>, probe: ProbeKind, kind: LimitKind) -> Verdict {
    let env = lab.env().await;
    let exec = Env::exec_of(&lab.probe(&probe)?);
    let ctx = env.ctx("tenant-a", "node-1");
    let mut create = env.create_req();
    create.limits = probe_limits();
    let (handle, claim) = env.ready(&ctx, create, &exec).await?;
    let result = env.run(&ctx, &handle, exec, &claim).await;
    ensure!(
        matches!(&result, Err(SandboxError::LimitExceeded { kind: got, .. }) if *got == kind),
        "the probe was not killed for {kind:?}: got {}",
        shape(&result)
    );
    let code = shape(&result);
    ensure!(
        env.journal
            .events()
            .iter()
            .any(|event| event.kind == SandboxEventKind::SandboxExecFailed
                && event.code.as_deref() == Some(code)),
        "the kill is not in the journal"
    );
    // What the program produced before it was killed can still be read.
    env.gather(&ctx, &handle)
        .await
        .must("collect after a kill")?;
    env.guard.destroy(&ctx, &handle).await.must("destroy")?;
    Ok(())
}

async fn wall_time(lab: &Lab<'_>) -> Verdict {
    killed_for(lab, ProbeKind::WallTime, LimitKind::WallTime).await
}

async fn memory(lab: &Lab<'_>) -> Verdict {
    killed_for(lab, ProbeKind::Memory, LimitKind::Memory).await
}

async fn processes(lab: &Lab<'_>) -> Verdict {
    killed_for(lab, ProbeKind::Processes, LimitKind::Processes).await
}

async fn disk(lab: &Lab<'_>) -> Verdict {
    killed_for(lab, ProbeKind::Disk, LimitKind::Disk).await
}

async fn output_truncated_and_marked(lab: &Lab<'_>) -> Verdict {
    let env = lab.env().await;
    let exec = Env::exec_of(&lab.probe(&ProbeKind::HugeOutput)?);
    let ctx = env.ctx("tenant-a", "node-1");
    let limits = probe_limits();
    let bound = limits.output_bytes;
    let mut create = env.create_req();
    create.limits = limits;
    let (handle, claim) = env.ready(&ctx, create, &exec).await?;
    let outcome = env.run(&ctx, &handle, exec, &claim).await.must("exec")?;
    ensure!(
        outcome.truncated && outcome.truncation.contains(&Truncation::Stdout),
        "an output past its bound is not marked as truncated"
    );
    let collected = env.gather(&ctx, &handle).await.must("collect")?;
    ensure!(
        collected.stdout.len() as u64 <= bound,
        "the collected output is longer than its bound"
    );
    ensure!(
        collected.truncated,
        "the collected output is not marked as truncated"
    );
    env.guard.destroy(&ctx, &handle).await.must("destroy")?;
    Ok(())
}

async fn ceiling_refused_not_clamped(lab: &Lab<'_>) -> Verdict {
    let exec = Env::success(lab)?;
    let ceiling = Limits {
        cpu_millicores: 500,
        memory_bytes: 512 * 1024 * 1024,
        wall_time_ms: 60_000,
        output_bytes: 2 * 1024 * 1024,
        max_processes: 64,
        ephemeral_disk_bytes: 128 * 1024 * 1024,
    };
    let mut policy = SandboxPolicy::default();
    policy.ceiling = ceiling.clone();
    let env = lab.env_with(policy).await;
    let ctx = env.ctx("tenant-a", "node-1");

    // One field at a time, a unit above the ceiling.
    let above = [
        (
            LimitField::CpuMillicores,
            Limits {
                cpu_millicores: ceiling.cpu_millicores + 1,
                ..ceiling.clone()
            },
        ),
        (
            LimitField::MemoryBytes,
            Limits {
                memory_bytes: ceiling.memory_bytes + 1,
                ..ceiling.clone()
            },
        ),
        (
            LimitField::WallTimeMs,
            Limits {
                wall_time_ms: ceiling.wall_time_ms + 1,
                ..ceiling.clone()
            },
        ),
        (
            LimitField::OutputBytes,
            Limits {
                output_bytes: ceiling.output_bytes + 1,
                ..ceiling.clone()
            },
        ),
        (
            LimitField::MaxProcesses,
            Limits {
                max_processes: ceiling.max_processes + 1,
                ..ceiling.clone()
            },
        ),
        (
            LimitField::EphemeralDiskBytes,
            Limits {
                ephemeral_disk_bytes: ceiling.ephemeral_disk_bytes + 1,
                ..ceiling.clone()
            },
        ),
    ];
    for (field, limits) in above {
        let mut create = env.create_req();
        create.limits = limits;
        let refused = env.guard.request_approval(&ctx, &create, &exec);
        ensure!(
            matches!(&refused, Err(SandboxError::PolicyDenied(PolicyReason::LimitAboveCeiling { field: got })) if *got == field),
            "{field:?} above the ceiling: expected policy_denied.limit_above_ceiling, got {}",
            shape(&refused)
        );
    }
    ensure!(
        env.tap.total() == 0,
        "a request above the ceiling reached the backend"
    );

    // At the ceiling the request goes through untouched.
    let mut create = env.create_req();
    create.limits = ceiling.clone();
    env.ready(&ctx, create, &exec).await?;
    let seen = env.tap.last_create().map(|request| request.limits);
    ensure!(
        seen.as_ref() == Some(&ceiling),
        "the limits the backend received are not the limits that were requested"
    );
    Ok(())
}
