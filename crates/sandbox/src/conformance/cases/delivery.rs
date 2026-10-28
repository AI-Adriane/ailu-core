//! Delivery: what a backend hands back to the guard at collect, and what the guard checks on
//! it. A backend declares digests that match the bytes it delivers, delivers only what was
//! asked for and cuts at the bounds; the guard recomputes every digest and refuses a delivery
//! that is out of bounds, outside the patterns, duplicated or inconsistent.

use super::{case, CaseDef};
use crate::collected::{glob_match, RawCollected, RawFile};
use crate::conformance::support::{ensure, expect_code, Env, Lab, Must, Verdict};
use crate::digest::{sha256_hex, Canon};
use crate::limits::Limits;
use crate::path::LogicalPath;
use crate::request::CollectRequest;
use std::collections::HashSet;

pub(super) fn cases() -> Vec<CaseDef> {
    vec![
        case!(
            Delivery,
            "delivery.guard_refuses_bad_deliveries",
            Guard,
            "a delivery with a stale digest, a stray or duplicated path or past a bound is refused and can be collected again",
            guard_refuses_bad_deliveries
        ),
        case!(
            Delivery,
            "delivery.digests_recomputed",
            Guard,
            "the digests of what is returned are recomputed by the guard over the bytes it holds",
            digests_recomputed
        ),
        case!(
            Delivery,
            "delivery.backend_digests_match",
            RealBackend,
            "the digests a backend declares are those of the bytes it delivers",
            backend_digests_match
        ),
        case!(
            Delivery,
            "delivery.backend_paths_match_patterns",
            RealBackend,
            "a backend delivers only files that match the requested patterns, each once",
            backend_paths_match_patterns
        ),
        case!(
            Delivery,
            "delivery.backend_truncates_at_bounds",
            RealBackend,
            "a backend cuts its delivery at the bounds of the request and of the output limit, and says so",
            backend_truncates_at_bounds
        ),
    ]
}

fn file(path: &str, bytes: &[u8]) -> Option<RawFile> {
    Some(RawFile {
        path: LogicalPath::new(path).ok()?,
        bytes: bytes.to_vec(),
        declared_sha256: sha256_hex(bytes),
    })
}

type Tamper = Box<dyn FnOnce(RawCollected) -> RawCollected + Send>;

async fn guard_refuses_bad_deliveries(lab: &Lab<'_>) -> Verdict {
    let env = lab.env().await;
    let exec = Env::success(lab)?;
    let tight = CollectRequest {
        patterns: vec!["out/*".to_owned()],
        max_files: 1,
        max_bytes: 1,
    };
    let liars: Vec<(&str, CollectRequest, Tamper)> = vec![
        (
            "a stale digest",
            env.collect_req(),
            Box::new(|mut raw| {
                raw.stdout.push(b'!');
                raw
            }),
        ),
        (
            "a path outside the patterns",
            env.collect_req(),
            Box::new(|mut raw| {
                raw.files.extend(file("never-requested/anything", b"y"));
                raw
            }),
        ),
        (
            "a duplicated path",
            env.collect_req(),
            Box::new(|mut raw| {
                raw.files.clear();
                raw.files.extend(file("out/twice.txt", b"one"));
                raw.files.extend(file("out/twice.txt", b"two"));
                raw
            }),
        ),
        (
            "an output past its bound",
            env.collect_req(),
            Box::new(|mut raw| {
                let size = usize::try_from(Limits::default().output_bytes).unwrap_or(0) + 1;
                let long = vec![b'x'; size];
                raw.declared_stdout_sha256 = sha256_hex(&long);
                raw.stdout = long;
                raw
            }),
        ),
        (
            "more files than asked for",
            tight.clone(),
            Box::new(|mut raw| {
                raw.files.clear();
                raw.files.extend(file("out/a", b"1"));
                raw.files.extend(file("out/b", b"2"));
                raw
            }),
        ),
        (
            "more bytes than asked for",
            tight,
            Box::new(|mut raw| {
                raw.files.clear();
                raw.files.extend(file("out/a", b"12"));
                raw
            }),
        ),
    ];
    for (n, (what, request, tamper)) in liars.into_iter().enumerate() {
        // A sandbox of its own for each lie.
        let ctx = env.ctx("tenant-a", &format!("node-{n}"));
        let (handle, claim) = env.ready(&ctx, env.create_req(), &exec).await?;
        env.run(&ctx, &handle, exec.clone(), &claim)
            .await
            .must("exec")?;
        env.tap
            .rewrite_collect(Box::new(move |result| result.map(tamper)));
        let refused = env
            .guard
            .collect(
                &ctx,
                &handle,
                request.clone(),
                env.run_started(),
                &Env::never(),
            )
            .await;
        expect_code(&refused, "backend_error", what)?;
        // Nothing was recorded: the honest delivery, asked again, is accepted.
        env.guard
            .collect(&ctx, &handle, request, env.run_started(), &Env::never())
            .await
            .must(&format!("the honest delivery after {what}"))?;
        env.guard.destroy(&ctx, &handle).await.must("destroy")?;
    }
    Ok(())
}

async fn digests_recomputed(lab: &Lab<'_>) -> Verdict {
    let env = lab.env().await;
    let exec = Env::success(lab)?;
    let ctx = env.ctx("tenant-a", "node-1");
    let ran = env.cycle(&ctx, env.create_req(), &exec).await?;
    let collected = &ran.collected;
    ensure!(
        collected.stdout_sha256 == sha256_hex(&collected.stdout)
            && collected.stderr_sha256 == sha256_hex(&collected.stderr),
        "a stream digest is not the digest of the returned bytes"
    );
    let mut outputs = Canon::new("sandbox.outputs.v1");
    for file in &collected.files {
        ensure!(
            file.sha256 == sha256_hex(&file.bytes) && file.size == file.bytes.len() as u64,
            "a file digest or size is not that of the returned bytes"
        );
        outputs.str("path", file.path.as_str());
        outputs.str("sha256", &file.sha256);
    }
    ensure!(
        collected
            .files
            .windows(2)
            .all(|pair| pair[0].path.as_str() < pair[1].path.as_str()),
        "the files are not returned in the order of their paths"
    );
    ensure!(
        collected.outputs_digest == outputs.finish(),
        "the outputs digest is not the digest of the returned files"
    );
    Ok(())
}

async fn delivered(
    env: &Env,
    lab: &Lab<'_>,
    limits: Limits,
    request: CollectRequest,
) -> Result<RawCollected, crate::conformance::support::Stop> {
    let exec = Env::success(lab)?;
    let ctx = env.ctx("tenant-a", "node-1");
    let mut create = env.create_req();
    create.limits = limits;
    let (handle, _) = env.b_create(&ctx, create).await.must("create")?;
    env.b_exec(&ctx, &handle, exec).await.must("exec")?;
    env.b_collect_with(&ctx, &handle, request)
        .await
        .must("collect")
}

async fn backend_digests_match(lab: &Lab<'_>) -> Verdict {
    let env = lab.env().await;
    let raw = delivered(&env, lab, Limits::default(), env.collect_req()).await?;
    let same = |declared: &str, bytes: &[u8]| declared.eq_ignore_ascii_case(&sha256_hex(bytes));
    ensure!(
        same(&raw.declared_stdout_sha256, &raw.stdout)
            && same(&raw.declared_stderr_sha256, &raw.stderr),
        "a declared stream digest is not the digest of the delivered bytes"
    );
    ensure!(
        raw.files
            .iter()
            .all(|file| same(&file.declared_sha256, &file.bytes)),
        "a declared file digest is not the digest of the delivered bytes"
    );
    Ok(())
}

async fn backend_paths_match_patterns(lab: &Lab<'_>) -> Verdict {
    let env = lab.env().await;
    let request = env.collect_req();
    let raw = delivered(&env, lab, Limits::default(), request.clone()).await?;
    ensure!(
        raw.files.iter().all(|file| request
            .patterns
            .iter()
            .any(|p| glob_match(p, file.path.as_str()))),
        "a delivered file does not match a requested pattern"
    );
    let paths: HashSet<_> = raw.files.iter().map(|file| file.path.as_str()).collect();
    ensure!(paths.len() == raw.files.len(), "a path was delivered twice");
    Ok(())
}

async fn backend_truncates_at_bounds(lab: &Lab<'_>) -> Verdict {
    let env = lab.env().await;
    // One byte of output per stream, one file of one byte at most.
    let limits = Limits {
        output_bytes: 1,
        ..Limits::default()
    };
    let request = CollectRequest {
        patterns: vec!["out/*".to_owned()],
        max_files: 1,
        max_bytes: 1,
    };
    let raw = delivered(&env, lab, limits, request).await?;
    ensure!(
        raw.stdout.len() <= 1 && raw.stderr.len() <= 1,
        "a stream is longer than the output limit"
    );
    ensure!(
        raw.files.len() <= 1,
        "more files were delivered than asked for"
    );
    let bytes: usize = raw.files.iter().map(|file| file.bytes.len()).sum();
    ensure!(bytes <= 1, "more bytes were delivered than asked for");
    ensure!(
        raw.declared_truncated
            || (raw.stdout.is_empty() && raw.stderr.is_empty() && raw.files.is_empty()),
        "a delivery cut at a bound is not marked as truncated"
    );
    Ok(())
}
