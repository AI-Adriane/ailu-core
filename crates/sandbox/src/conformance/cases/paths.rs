//! Paths: nothing leaves the working root. A path with `..`, an absolute path, a backslash or a
//! NUL byte is refused wherever it can be put, an outgoing link is refused at collect, and an
//! artifact of another tenant is not found.

use super::{case, CaseDef};
use crate::conformance::support::{
    ensure, expect_code, expect_one_of, skip, Env, Lab, Must, Stop, Verdict,
};
use crate::conformance::target::ProbeKind;
use crate::ids::{SandboxStep, TenantId};
use crate::path::LogicalPath;
use crate::request::{CollectRequest, CreateRequest, ExecRequest, InputRef};
use ailu_fs_backend::ArtifactRef;
use serde_json::json;

pub(super) fn cases() -> Vec<CaseDef> {
    vec![
        case!(
            Paths,
            "paths.invalid_forms_refused",
            Guard,
            "..., an absolute path, a backslash and a NUL byte are refused in every path a request holds",
            invalid_forms_refused
        ),
        case!(
            Paths,
            "paths.outgoing_link_refused",
            RealBackend,
            "a symbolic link that leaves the working root is refused at collect",
            outgoing_link_refused
        ),
        case!(
            Paths,
            "paths.foreign_artifact_not_found",
            RealBackend,
            "an input that is another tenant's artifact is not found and nothing is created",
            foreign_artifact_not_found
        ),
    ]
}

/// The four spellings the path rule refuses, each with the pieces around it that a real path
/// would have.
const INVALID: [&str; 5] = [
    "../outside",
    "a/../../outside",
    "/absolute",
    "dir\\file",
    "a\0b",
];

async fn invalid_forms_refused(lab: &Lab<'_>) -> Verdict {
    let env = lab.env().await;
    let exec = Env::success(lab)?;
    let ctx = env.ctx("tenant-a", "node-1");
    let (handle, claim) = env.ready(&ctx, env.create_req(), &exec).await?;
    env.run(&ctx, &handle, exec, &claim).await.must("exec")?;
    for raw in INVALID {
        ensure!(
            LogicalPath::new(raw).is_err(),
            "a logical path accepted an invalid spelling"
        );
        let cwd = json!({ "argv": ["tool"], "cwd": raw });
        ensure!(
            serde_json::from_value::<ExecRequest>(cwd).is_err(),
            "an exec request accepted an invalid working directory"
        );
        let dest = json!({
            "artifact": { "id": "a", "version": 1 },
            "dest": raw
        });
        ensure!(
            serde_json::from_value::<InputRef>(dest).is_err(),
            "an input accepted an invalid destination"
        );
        let pattern = CollectRequest {
            patterns: vec![raw.to_owned()],
            max_files: 1,
            max_bytes: 1024,
        };
        let refused = env
            .guard
            .collect(&ctx, &handle, pattern, env.run_started(), &Env::never())
            .await;
        expect_code(
            &refused,
            "invalid_path",
            "a collect pattern with an invalid path",
        )?;
    }
    ensure!(
        env.tap.count(SandboxStep::Collect) == 0,
        "a collect with an invalid path reached the backend"
    );
    Ok(())
}

async fn outgoing_link_refused(lab: &Lab<'_>) -> Verdict {
    let env = lab.env().await;
    let exec = Env::exec_of(&lab.probe(&ProbeKind::OutgoingLink)?);
    let ctx = env.ctx("tenant-a", "node-1");
    let (handle, claim) = env.ready(&ctx, env.create_req(), &exec).await?;
    env.run(&ctx, &handle, exec, &claim).await.must("exec")?;
    let refused = env.gather(&ctx, &handle).await;
    expect_code(
        &refused,
        "invalid_path",
        "a collect that meets an outgoing link",
    )?;
    env.guard.destroy(&ctx, &handle).await.must("destroy")?;
    Ok(())
}

fn with_input(env: &Env, artifact: &ArtifactRef) -> Result<CreateRequest, Stop> {
    let Ok(dest) = LogicalPath::new("in/data.txt") else {
        skip!("the input destination is not a valid path");
    };
    Ok(CreateRequest {
        inputs: vec![InputRef {
            artifact: artifact.clone(),
            dest,
        }],
        ..env.create_req()
    })
}

async fn foreign_artifact_not_found(lab: &Lab<'_>) -> Verdict {
    let env = lab.env().await;
    let exec = Env::success(lab)?;
    let artifact = ArtifactRef {
        id: "conformance-artifact".into(),
        version: 1,
    };
    if !env
        .hooks
        .provide_artifact(&TenantId::new("tenant-a"), &artifact, b"input bytes")
        .await
    {
        skip!("the target cannot make an artifact available as an input");
    }
    // The control: the tenant's own artifact is accepted.
    let own = env.ctx("tenant-a", "own");
    env.ready(&own, with_input(&env, &artifact)?, &exec).await?;

    // The same reference from another tenant, at the backend and through the guard.
    let other = env.ctx("tenant-b", "foreign");
    let direct = env.b_create(&other, with_input(&env, &artifact)?).await;
    expect_code(
        &direct,
        "not_found",
        "the backend, for another tenant's artifact",
    )?;
    let ctx = env.ctx("tenant-b", "through-the-guard");
    let create = with_input(&env, &artifact)?;
    let claim = env.approve(&ctx, &create, &exec).must("approval request")?;
    let refused = env
        .guard
        .create(
            &ctx,
            create,
            &exec,
            Some(&claim),
            env.run_started(),
            &Env::never(),
        )
        .await;
    expect_one_of(
        &refused,
        &["not_found", "backend_error"],
        "the guard, for another tenant's artifact",
    )
}
