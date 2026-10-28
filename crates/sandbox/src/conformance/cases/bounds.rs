//! Request bounds: a request outside the bounds of the deployment, or malformed, is refused by
//! the guard before it reaches the backend, with a typed reason that holds no content.

use super::{case, CaseDef};
use crate::conformance::support::{ensure, expect_code, Env, Lab, Must, Verdict};
use crate::ids::SandboxContext;
use crate::limits::Limits;
use crate::path::LogicalPath;
use crate::request::{
    CollectRequest, CreateRequest, EnvGrant, ExecRequest, HostRule, InputRef, NetworkPolicy,
    SecretRef,
};
use ailu_fs_backend::ArtifactRef;

pub(super) fn cases() -> Vec<CaseDef> {
    vec![
        case!(
            Bounds,
            "bounds.request_out_of_bounds_refused",
            Guard,
            "arguments, standard input, inputs, collect patterns, files, bytes and ttl past their bounds are refused",
            request_out_of_bounds_refused
        ),
        case!(
            Bounds,
            "bounds.malformed_requests_refused",
            Guard,
            "an empty or odd network rule, a conflicting input, a zero limit or a bad name is refused",
            malformed_requests_refused
        ),
    ]
}

fn tool(argv: Vec<String>) -> ExecRequest {
    ExecRequest {
        argv,
        cwd: None,
        stdin: None,
    }
}

fn input(index: usize, dest: &str) -> Option<InputRef> {
    Some(InputRef {
        artifact: ArtifactRef {
            id: format!("artifact-{index}").into(),
            version: 1,
        },
        dest: LogicalPath::new(dest).ok()?,
    })
}

/// The call is refused with `code`, as an approval request and as a create, and the backend
/// is not reached.
async fn refused(
    env: &Env,
    ctx: &SandboxContext,
    create: &CreateRequest,
    exec: &ExecRequest,
    code: &str,
    what: &str,
) -> Verdict {
    let asked = env.guard.request_approval(ctx, create, exec);
    expect_code(&asked, code, &format!("{what}, as an approval request"))?;
    let created = env
        .guard
        .create(
            ctx,
            create.clone(),
            exec,
            None,
            env.run_started(),
            &Env::never(),
        )
        .await;
    expect_code(&created, code, &format!("{what}, as a create"))?;
    ensure!(env.tap.total() == 0, "{what} reached the backend");
    Ok(())
}

async fn request_out_of_bounds_refused(lab: &Lab<'_>) -> Verdict {
    let env = lab.env().await;
    let bounds = env.policy.bounds.clone();
    let ctx = env.ctx("tenant-a", "node-1");
    let create = env.create_req();
    let too_large = "policy_denied.request_too_large";

    let many = tool(
        (0..=bounds.max_argv_count)
            .map(|i| format!("a{i}"))
            .collect(),
    );
    refused(&env, &ctx, &create, &many, too_large, "too many arguments").await?;

    let half = usize::try_from(bounds.max_argv_bytes / 2).unwrap_or(0) + 1;
    let wide = tool(vec!["x".repeat(half), "y".repeat(half)]);
    refused(
        &env,
        &ctx,
        &create,
        &wide,
        too_large,
        "arguments of too many bytes",
    )
    .await?;

    let stdin_size = usize::try_from(bounds.max_stdin_bytes).unwrap_or(0) + 1;
    let heavy = ExecRequest {
        stdin: Some(vec![0; stdin_size]),
        ..tool(vec!["tool".to_owned()])
    };
    refused(
        &env,
        &ctx,
        &create,
        &heavy,
        too_large,
        "a standard input past its bound",
    )
    .await?;

    let plain = tool(vec!["tool".to_owned()]);
    let mut slow = env.create_req();
    slow.ttl_ms = bounds.max_ttl_ms + 1;
    refused(&env, &ctx, &slow, &plain, too_large, "a ttl past its bound").await?;
    let mut instant = env.create_req();
    instant.ttl_ms = 0;
    refused(
        &env,
        &ctx,
        &instant,
        &plain,
        "policy_denied.ttl_zero",
        "a zero ttl",
    )
    .await?;

    let mut crowded = env.create_req();
    for i in 0..=bounds.max_input_files as usize {
        crowded.inputs.extend(input(i, &format!("in/file-{i}")));
    }
    refused(
        &env,
        &ctx,
        &crowded,
        &plain,
        "policy_denied.input_too_large",
        "too many inputs",
    )
    .await?;

    // The shape of the exec request.
    refused(
        &env,
        &ctx,
        &create,
        &tool(vec![]),
        "policy_denied.empty_argv",
        "an empty argv",
    )
    .await?;
    let nul = tool(vec!["tool".to_owned(), "a\0b".to_owned()]);
    refused(
        &env,
        &ctx,
        &create,
        &nul,
        "policy_denied.nul_in_argument",
        "a NUL byte in an argument",
    )
    .await?;
    let nameless = tool(vec![String::new()]);
    refused(
        &env,
        &ctx,
        &create,
        &nameless,
        "policy_denied.empty_program",
        "an empty program",
    )
    .await?;

    // The bounds of a collect, on a sandbox that has run.
    let exec = Env::success(lab)?;
    let (handle, claim) = env.ready(&ctx, env.create_req(), &exec).await?;
    env.run(&ctx, &handle, exec, &claim).await.must("exec")?;
    let calls = env.tap.total();
    let base = env.collect_req();
    let past = [
        (
            CollectRequest {
                patterns: (0..=bounds.max_collect_patterns)
                    .map(|i| format!("out/{i}"))
                    .collect(),
                ..base.clone()
            },
            "too many patterns",
        ),
        (
            CollectRequest {
                max_files: bounds.max_collect_files + 1,
                ..base.clone()
            },
            "too many files",
        ),
        (
            CollectRequest {
                max_bytes: bounds.max_collect_bytes + 1,
                ..base
            },
            "too many bytes",
        ),
    ];
    for (request, what) in past {
        let result = env
            .guard
            .collect(&ctx, &handle, request, env.run_started(), &Env::never())
            .await;
        expect_code(&result, too_large, &format!("a collect with {what}"))?;
    }
    ensure!(
        env.tap.total() == calls,
        "a collect past its bounds reached the backend"
    );
    Ok(())
}

async fn malformed_requests_refused(lab: &Lab<'_>) -> Verdict {
    let env = lab.env().await;
    let ctx = env.ctx("tenant-a", "node-1");
    let plain = tool(vec!["tool".to_owned()]);
    let with_network = |rules: Vec<HostRule>| CreateRequest {
        network: NetworkPolicy::Allow(rules),
        ..env.create_req()
    };
    let host = |name: &str| HostRule {
        host: name.to_owned(),
        port: 443,
    };
    let rule = "policy_denied.network_rule";
    let checks: Vec<(CreateRequest, &str, &str)> = vec![
        (
            with_network(vec![]),
            "policy_denied.network_allow_empty",
            "an allow list with no rule",
        ),
        (
            with_network(vec![host("Api.Example.com")]),
            rule,
            "a host in capitals",
        ),
        (
            with_network(vec![host("93.184.216.34")]),
            rule,
            "an IPv4 literal",
        ),
        (
            with_network(vec![host("[2001:db8::1]")]),
            rule,
            "an IPv6 literal",
        ),
        (with_network(vec![host("localhost")]), rule, "localhost"),
        (
            with_network(vec![host("*.example.com")]),
            rule,
            "a wildcard",
        ),
        (
            with_network(vec![HostRule {
                port: 0,
                ..host("api.example.com")
            }]),
            rule,
            "port zero",
        ),
        // Well formed, but the deployment's list is empty.
        (
            with_network(vec![host("api.example.com")]),
            rule,
            "a rule outside the deployment's list",
        ),
        (
            CreateRequest {
                limits: Limits {
                    wall_time_ms: 0,
                    ..Limits::default()
                },
                ..env.create_req()
            },
            "policy_denied.limit_zero",
            "a zero limit",
        ),
        (
            CreateRequest {
                profile: String::new(),
                ..env.create_req()
            },
            "policy_denied.empty_field",
            "an empty profile",
        ),
        (
            CreateRequest {
                env: vec![EnvGrant {
                    name: "bad-name".to_owned(),
                    secret_ref: SecretRef("r".to_owned()),
                }],
                ..env.create_req()
            },
            "policy_denied.invalid_env_name",
            "an environment name with a hyphen",
        ),
        (
            CreateRequest {
                env: vec![
                    EnvGrant {
                        name: "TOKEN".to_owned(),
                        secret_ref: SecretRef("r1".to_owned()),
                    },
                    EnvGrant {
                        name: "TOKEN".to_owned(),
                        secret_ref: SecretRef("r2".to_owned()),
                    },
                ],
                ..env.create_req()
            },
            "policy_denied.invalid_env_name",
            "an environment name granted twice",
        ),
        (
            CreateRequest {
                env: vec![EnvGrant {
                    name: "TOKEN".to_owned(),
                    secret_ref: SecretRef(String::new()),
                }],
                ..env.create_req()
            },
            "policy_denied.empty_field",
            "an empty secret reference",
        ),
    ];
    for (create, code, what) in checks {
        refused(&env, &ctx, &create, &plain, code, what).await?;
    }
    // Destinations that are the same, or one inside the other.
    for (first, second, what) in [
        ("in/a", "in/a", "the same destination twice"),
        ("in/a", "in/a/b", "a file where a directory is needed"),
    ] {
        let mut conflicting = env.create_req();
        conflicting.inputs.extend(input(0, first));
        conflicting.inputs.extend(input(1, second));
        refused(
            &env,
            &ctx,
            &conflicting,
            &plain,
            "policy_denied.input_path_conflict",
            what,
        )
        .await?;
    }
    Ok(())
}
