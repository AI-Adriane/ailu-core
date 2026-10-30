//! Host authorization for graph-supplied model destinations. The host owns both this
//! allowlist and DNS/network routing for its entries; graphs cannot grant themselves access.

use url::Url;

pub const MODEL_ENDPOINT_ALLOWLIST_VAR: &str = "AILU_MODEL_ENDPOINT_ALLOWLIST";
pub const MODEL_ENDPOINT_NOT_ALLOWED: &str = "AILU_MODEL_ENDPOINT_NOT_ALLOWED";

// Use the same URL parser as reqwest and pass the canonical result to the transport.
// No query/fragment/userinfo: the transport appends `/chat/completions` to this base.
fn canonical_base(raw: &str) -> Result<String, ()> {
    if raw.chars().any(|c| c.is_control()) {
        return Err(());
    }
    let raw = raw.trim();
    if raw.is_empty() || raw.contains(['\\', '*']) {
        return Err(());
    }
    let url = Url::parse(raw).map_err(|_| ())?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(());
    }
    Ok(url.as_str().trim_end_matches('/').to_owned())
}

fn authorize(base_url: &str, allowlist: Option<&str>) -> Result<String, String> {
    let refused = || {
        format!(
            "{MODEL_ENDPOINT_NOT_ALLOWED}: custom baseURL requires an exact HTTP(S) base URL in \
         the host's {MODEL_ENDPOINT_ALLOWLIST_VAR}; credentials, queries, fragments and \
         wildcards are not supported"
        )
    };
    let base = canonical_base(base_url).map_err(|_| refused())?;
    let mut allowed = false;
    for entry in allowlist.unwrap_or("").split(',').map(str::trim) {
        if entry.is_empty() {
            continue;
        }
        // An invalid entry invalidates the entire policy, even after a matching entry.
        let entry = canonical_base(entry).map_err(|_| refused())?;
        allowed |= entry == base;
    }
    if allowed {
        Ok(base)
    } else {
        Err(refused())
    }
}

pub(crate) fn authorize_from_process(base_url: &str) -> Result<String, String> {
    let allowlist = std::env::var(MODEL_ENDPOINT_ALLOWLIST_VAR).map_err(|_| {
        format!("{MODEL_ENDPOINT_NOT_ALLOWED}: {MODEL_ENDPOINT_ALLOWLIST_VAR} is unset or invalid")
    })?;
    authorize(base_url, Some(&allowlist))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_destinations_are_denied_without_host_authorization() {
        for policy in [
            None,
            Some(""),
            Some(" , "),
            Some("https://models.example/v1"),
        ] {
            for base in [
                "http://localhost:1234/v1",
                "http://127.0.0.1/v1",
                "http://2130706433/v1",
                "http://0x7f000001/v1",
                "http://10.0.0.1/v1",
                "http://172.16.0.1/v1",
                "http://192.168.0.1/v1",
                "http://169.254.169.254/latest/meta-data",
                "http://[::1]/v1",
                "http://[::ffff:127.0.0.1]/v1",
                "http://[fc00::1]/v1",
                "http://[fe80::1]/v1",
                "https://unlisted.example/v1",
            ] {
                assert!(authorize(base, policy).is_err(), "{base} {policy:?}");
            }
        }
    }

    #[test]
    fn only_the_exact_canonical_endpoint_is_authorized() {
        let policy = Some(" https://MODELS.example:443/v1/ , http://127.0.0.1:1234/v1 , ");
        assert_eq!(
            authorize(" https://models.example/v1/// ", policy),
            Ok("https://models.example/v1".into())
        );
        assert_eq!(
            authorize("http://127.0.0.1:1234/v1", policy),
            Ok("http://127.0.0.1:1234/v1".into())
        );
        for base in [
            "http://models.example/v1",
            "https://models.example:444/v1",
            "https://models.example/v2",
            "https://models.example/v1/other",
            "https://models.example.evil/v1",
            "https://evilmodels.example/v1",
            "https://models.example@evil.example/v1",
            "https://user:secret@models.example/v1",
            "https://models.example/v1?target=internal",
            "https://models.example/v1#fragment",
            "http://127.0.0.1:1235/v1",
        ] {
            assert!(authorize(base, policy).is_err(), "{base}");
        }
    }

    #[test]
    fn invalid_urls_and_invalid_policies_fail_closed() {
        for base in [
            "",
            "relative/v1",
            "file:///etc/passwd",
            "ftp://models.example/v1",
            "https://*.example/v1",
            "https://models.example/v1?",
            "https://models.example/v1#",
            "https://models.example\\@evil.example/v1",
            "https://mod\nels.example/v1",
        ] {
            assert!(authorize(base, Some(base)).is_err(), "{base:?}");
        }
        assert!(authorize(
            "https://models.example/v1",
            Some("https://models.example/v1,invalid")
        )
        .is_err());
    }

    #[test]
    fn credentials_in_policy_entries_invalidate_the_whole_policy() {
        for entry in [
            "https://user@models.example/v1",
            "https://user:secret@models.example/v1",
            "https://:secret@models.example/v1",
        ] {
            assert!(canonical_base(entry).is_err());
            assert!(authorize(entry, Some(entry)).is_err());
            let policy = format!("https://models.example/v1,{entry}");
            assert!(authorize("https://models.example/v1", Some(&policy)).is_err());
        }
    }
}
