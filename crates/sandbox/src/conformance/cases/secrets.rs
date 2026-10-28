//! Secrets and privileges: the environment starts empty, a secret enters only through a grant
//! covered by the approval, and its value is nowhere but in the program that needs it. The guard
//! redacts what the backend hands back, in every form it knows, and fails closed when the
//! backend hands back less than it was granted.

use super::{case, CaseDef};
use crate::collected::{
    base64_encode, hex_encode, percent_encode, RawFile, ResolvedSecrets, REDACTION_MARKER,
};
use crate::conformance::support::{ensure, expect_code, skip, Env, Lab, Must, Stop, Verdict};
use crate::conformance::target::ProbeKind;
use crate::digest::sha256_hex;
use crate::error::SandboxError;
use crate::ids::SandboxStep;
use crate::path::LogicalPath;
use crate::request::{CreateRequest, EnvGrant, SecretRef};
use crate::secret::ResolvedSecret;

pub(super) fn cases() -> Vec<CaseDef> {
    vec![
        case!(
            Secrets,
            "secrets.empty_environment",
            RealBackend,
            "a program starts with the environment its profile declares and nothing inherited",
            empty_environment
        ),
        case!(
            Secrets,
            "secrets.granted_secret_reaches_program_only",
            RealBackend,
            "a granted secret reaches the program and appears nowhere the caller or a log can see",
            granted_secret_reaches_program_only
        ),
        case!(
            Secrets,
            "secrets.uncovered_grant_refused",
            Guard,
            "a secret grant the approval does not cover is refused before anything is created",
            uncovered_grant_refused
        ),
        case!(
            Secrets,
            "secrets.foreign_reference_refused",
            RealBackend,
            "another tenant's secret reference runs nothing and is a final generic error",
            foreign_reference_refused
        ),
        case!(
            Secrets,
            "secrets.fewer_than_granted_fails_closed",
            Guard,
            "a backend that hands back fewer secrets than were granted gets no output returned",
            fewer_than_granted_fails_closed
        ),
        case!(
            Secrets,
            "secrets.encoded_forms_redacted",
            Guard,
            "the value of a secret is redacted in its hex, base64 and URL-encoded forms",
            encoded_forms_redacted
        ),
        case!(
            Secrets,
            "secrets.collected_paths_redacted",
            Guard,
            "a collected file name that holds a secret value is redacted",
            collected_paths_redacted
        ),
        case!(
            Secrets,
            "secrets.reresolved_on_replay",
            RealBackend,
            "a replayed collect resolves the secrets again, so its output is redacted too",
            reresolved_on_replay
        ),
    ]
}

/// The value the suite plants. It is path-safe, so it can be planted in a file name too.
const SECRET: &str = "cnf-s3cr3t-7f3a9c41";
const REFERENCE: &str = "conformance/secret";
const NAME: &str = "CONF_TOKEN";

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

fn with_grant(env: &Env) -> CreateRequest {
    CreateRequest {
        env: vec![EnvGrant {
            name: NAME.to_owned(),
            secret_ref: SecretRef(REFERENCE.to_owned()),
        }],
        ..env.create_req()
    }
}

/// Make the secret resolvable for `tenant`, or the case is not applicable.
async fn provide(env: &Env, tenant: &str) -> Result<(), Stop> {
    if !env
        .hooks
        .provide_secret(&crate::ids::TenantId::new(tenant), REFERENCE, SECRET)
        .await
    {
        skip!("the target cannot make a secret resolvable");
    }
    Ok(())
}

/// The variables a profile may reasonably start a program with. A profile that declares
/// anything else is declaring something that looks like a secret or a host variable.
fn plausible_base_variable(name: &str) -> bool {
    matches!(
        name,
        "LANG"
            | "LANGUAGE"
            | "PATH"
            | "TZ"
            | "PWD"
            | "HOME"
            | "USER"
            | "LOGNAME"
            | "SHLVL"
            | "TMPDIR"
            | "TERM"
            | "HOSTNAME"
    ) || name.starts_with("LC_")
}

async fn empty_environment(lab: &Lab<'_>) -> Verdict {
    let env = lab.env().await;
    let exec = Env::exec_of(&lab.probe(&ProbeKind::ListEnvironment)?);
    let Some(declared) = env.hooks.base_environment().await else {
        skip!("the target does not declare the environment its profile starts with");
    };
    ensure!(
        declared.iter().all(|name| plausible_base_variable(name)),
        "the profile declares a variable that is not part of a minimal environment"
    );
    let ctx = env.ctx("tenant-a", "node-1");
    let ran = env.cycle(&ctx, env.create_req(), &exec).await?;
    let listed = String::from_utf8_lossy(&ran.collected.stdout).into_owned();
    let listed: std::collections::BTreeSet<&str> =
        listed.lines().filter(|name| !name.is_empty()).collect();
    ensure!(!listed.is_empty(), "the program listed no variable at all");
    let declared: std::collections::BTreeSet<&str> = declared.iter().map(String::as_str).collect();
    let extra = listed.difference(&declared).count();
    ensure!(
        extra == 0,
        "{extra} environment variables are not declared by the profile"
    );
    let missing = declared.difference(&listed).count();
    ensure!(
        missing == 0,
        "{missing} variables the profile declares were not in the environment"
    );
    Ok(())
}

async fn granted_secret_reaches_program_only(lab: &Lab<'_>) -> Verdict {
    let env = lab.env().await;
    let exec = Env::exec_of(&lab.probe(&ProbeKind::ReadSecret {
        name: NAME.to_owned(),
    })?);
    provide(&env, "tenant-a").await?;
    let ctx = env.ctx("tenant-a", "node-1");
    let ran = env.cycle(&ctx, with_grant(&env), &exec).await?;
    let collected = &ran.collected;
    ensure!(
        contains(&collected.stdout, REDACTION_MARKER.as_bytes()),
        "the program did not receive the granted secret"
    );
    let leaked = contains(&collected.stdout, SECRET.as_bytes())
        || contains(&collected.stderr, SECRET.as_bytes())
        || collected
            .files
            .iter()
            .any(|file| contains(&file.bytes, SECRET.as_bytes()));
    ensure!(
        !leaked,
        "the value of the secret is in the collected output"
    );
    let seen = format!(
        "{} {:?} {:?} {:?}",
        serde_json::to_string(&env.journal.events()).unwrap_or_default(),
        collected,
        ran.outcome,
        ran.handle
    );
    ensure!(
        !seen.contains(SECRET),
        "the value of the secret is in the journal or in a debug output"
    );
    Ok(())
}

async fn uncovered_grant_refused(lab: &Lab<'_>) -> Verdict {
    let env = lab.env().await;
    let exec = Env::success(lab)?;
    let ctx = env.ctx("tenant-a", "node-1");
    // The approver saw a call with no secret.
    let claim = env
        .approve(&ctx, &env.create_req(), &exec)
        .must("approval request")?;
    let created = env
        .guard
        .create(
            &ctx,
            with_grant(&env),
            &exec,
            Some(&claim),
            env.run_started(),
            &Env::never(),
        )
        .await;
    expect_code(
        &created,
        "approval_required",
        "a create that adds a secret grant to an approved call",
    )?;
    ensure!(
        env.tap.count(SandboxStep::Create) == 0,
        "an uncovered grant reached the backend"
    );
    Ok(())
}

async fn foreign_reference_refused(lab: &Lab<'_>) -> Verdict {
    let env = lab.env().await;
    let exec = Env::exec_of(&lab.probe(&ProbeKind::ReadSecret {
        name: NAME.to_owned(),
    })?);
    provide(&env, "tenant-a").await?;
    let before = env.hooks.executions().await;
    // Tenant b names tenant a's reference.
    let ctx = env.ctx("tenant-b", "node-1");
    let create = with_grant(&env);
    let claim = env.approve(&ctx, &create, &exec).must("approval request")?;
    let created = env
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
    let refused = match created {
        // Resolved at creation: the refusal comes here.
        Err(error) => Err::<(), SandboxError>(error),
        Ok(handle) => env
            .run(&ctx, &handle, exec.clone(), &claim)
            .await
            .map(|_| ()),
    };
    expect_code(
        &refused,
        "backend_error",
        "another tenant's secret reference",
    )?;
    if let (Some(before), Some(after)) = (before, env.hooks.executions().await) {
        ensure!(
            after == before,
            "a program ran with another tenant's secret"
        );
    }
    Ok(())
}

async fn fewer_than_granted_fails_closed(lab: &Lab<'_>) -> Verdict {
    let env = lab.env().await;
    let exec = Env::exec_of(&lab.probe(&ProbeKind::ReadSecret {
        name: NAME.to_owned(),
    })?);
    provide(&env, "tenant-a").await?;
    let ctx = env.ctx("tenant-a", "node-1");
    let (handle, claim) = env.ready(&ctx, with_grant(&env), &exec).await?;
    env.run(&ctx, &handle, exec, &claim).await.must("exec")?;
    // The backend "forgets" to hand back the granted secret.
    env.tap.rewrite_collect(Box::new(|result| {
        result.map(|mut raw| {
            raw.secrets = ResolvedSecrets::new();
            raw
        })
    }));
    let refused = env.gather(&ctx, &handle).await;
    expect_code(
        &refused,
        "backend_error",
        "a delivery with fewer secrets than grants",
    )?;
    // Nothing was returned and nothing is recorded: the honest delivery still works.
    let collected = env.gather(&ctx, &handle).await.must("collect again")?;
    ensure!(
        !contains(&collected.stdout, SECRET.as_bytes()),
        "the value of the secret is in the collected output"
    );
    Ok(())
}

/// Every form of `value` the guard knows how to find.
fn forms(value: &str) -> Vec<Vec<u8>> {
    let raw = value.as_bytes();
    let mut all = vec![
        raw.to_vec(),
        hex_encode(raw, false),
        hex_encode(raw, true),
        percent_encode(raw),
    ];
    for url_safe in [false, true] {
        for pad in [false, true] {
            all.push(base64_encode(raw, url_safe, pad));
        }
    }
    all
}

async fn encoded_forms_redacted(lab: &Lab<'_>) -> Verdict {
    let env = lab.env().await;
    let exec = Env::success(lab)?;
    let ctx = env.ctx("tenant-a", "node-1");
    let (handle, claim) = env.ready(&ctx, env.create_req(), &exec).await?;
    env.run(&ctx, &handle, exec, &claim).await.must("exec")?;
    let forms = forms(SECRET);
    let planted = forms.clone();
    env.tap.rewrite_collect(Box::new(move |result| {
        result.map(|mut raw| {
            let mut stdout = Vec::new();
            for form in &planted {
                stdout.extend_from_slice(b"[");
                stdout.extend_from_slice(form);
                stdout.extend_from_slice(b"]");
            }
            raw.declared_stdout_sha256 = sha256_hex(&stdout);
            raw.stdout = stdout;
            raw.secrets.push(ResolvedSecret::new(SECRET));
            raw
        })
    }));
    let collected = env.gather(&ctx, &handle).await.must("collect")?;
    let left = forms
        .iter()
        .filter(|form| contains(&collected.stdout, form))
        .count();
    ensure!(
        left == 0,
        "{left} encoded forms of the secret were not redacted"
    );
    ensure!(
        contains(&collected.stdout, REDACTION_MARKER.as_bytes()),
        "the redaction left no marker"
    );
    Ok(())
}

async fn collected_paths_redacted(lab: &Lab<'_>) -> Verdict {
    let env = lab.env().await;
    let exec = Env::success(lab)?;
    let ctx = env.ctx("tenant-a", "node-1");
    let (handle, claim) = env.ready(&ctx, env.create_req(), &exec).await?;
    env.run(&ctx, &handle, exec, &claim).await.must("exec")?;
    env.tap.rewrite_collect(Box::new(|result| {
        result.map(|mut raw| {
            let bytes = b"content".to_vec();
            if let Ok(path) = LogicalPath::new(&format!("out/{SECRET}.txt")) {
                raw.files.push(RawFile {
                    path,
                    declared_sha256: sha256_hex(&bytes),
                    bytes,
                });
            }
            raw.secrets.push(ResolvedSecret::new(SECRET));
            raw
        })
    }));
    let collected = env.gather(&ctx, &handle).await.must("collect")?;
    ensure!(
        !collected
            .files
            .iter()
            .any(|file| file.path.as_str().contains(SECRET)),
        "a collected file name holds the value of a secret"
    );
    ensure!(
        collected
            .files
            .iter()
            .any(|file| file.path.as_str().contains(REDACTION_MARKER)),
        "the planted file is missing from the delivery"
    );
    Ok(())
}

async fn reresolved_on_replay(lab: &Lab<'_>) -> Verdict {
    let env = lab.env().await;
    let exec = Env::exec_of(&lab.probe(&ProbeKind::ReadSecret {
        name: NAME.to_owned(),
    })?);
    provide(&env, "tenant-a").await?;
    let ctx = env.ctx("tenant-a", "node-1");
    let ran = env.cycle(&ctx, with_grant(&env), &exec).await?;
    // The host restarts and replays the node: what the backend kept holds no secret value.
    let guard = env.restarted();
    let (_, collected) = env
        .replay(&guard, &ctx, with_grant(&env), &exec, &ran.claim)
        .await?;
    ensure!(
        collected == ran.collected,
        "the replayed collect returned another result"
    );
    ensure!(
        !contains(&collected.stdout, SECRET.as_bytes()),
        "the replayed output holds the value of the secret"
    );
    Ok(())
}
