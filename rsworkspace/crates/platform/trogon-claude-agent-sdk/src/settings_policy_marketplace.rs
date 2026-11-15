//! Whether a marketplace source in a managed allow or block list can ever be enforced (`sP` upstream).
//!
//! An entry whose pattern does not compile, whose GitHub owner wildcard is malformed, whose Git address hides a
//! wildcard in a GitHub path or whose ref holds a wildcard can never match a source. The allowlist drops such an
//! entry; the blocklist keeps it so the restriction stays active.

use serde_json::Value;

use crate::{
    bridge_url::{
        backslash_authority, bracket_form, canonical_host, constants::GITHUB, fold_host, git_address, hex_pair,
        is_github, rewritten_url,
    },
    settings_schema_regexp::capturing_groups,
};

mod constants;

use constants::SSH_GITHUB;

/// `sP(entry) !== null`.
pub(crate) fn unenforceable(entry: &Value) -> bool {
    let text = |name: &str| entry.get(name).and_then(Value::as_str);
    let source = text("source").unwrap_or_default();
    let pattern = match source {
        "hostPattern" | "pathPattern" => text(source),
        _ => None,
    };
    pattern.is_some_and(|pattern| capturing_groups(pattern).is_none())
        || (source == "github" && text("repo").is_some_and(|repo| repo.contains('*') && owner_wildcard(repo).is_none()))
        || (source == "git" && text("url").is_some_and(github_wildcard))
        || (matches!(source, "github" | "git") && text("ref").is_some_and(|reference| reference.contains('*')))
}

/// `_3`: the owner of an `<owner>/*` repository, when the owner is a plain name.
fn owner_wildcard(repo: &str) -> Option<&str> {
    repo.strip_suffix("/*").filter(|owner| {
        !owner.is_empty()
            && owner
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
            && !owner.starts_with('-')
            && *owner != "."
            && *owner != ".."
    })
}

/// `Ck`: GitHub, or the host GitHub serves SSH on port 443 from.
fn is_github_host(host: &str) -> bool {
    is_github(host) || fold_host(host) == SSH_GITHUB
}

/// `U6`: the canonical host with the SSH alias read as GitHub.
fn github_alias(host: &str) -> String {
    let canonical = canonical_host(host);
    if canonical == SSH_GITHUB {
        GITHUB.to_owned()
    } else {
        canonical
    }
}

/// `y3`: whether a Git address names a GitHub repository through a path holding a wildcard.
fn github_wildcard(address: &str) -> bool {
    if !address.contains("://") {
        return git_address(address, false).is_some_and(|(host, path)| {
            is_github_host(&host) && decode_uri_component(&path).unwrap_or(path).contains('*')
        });
    }
    if backslash_authority(address) || bracket_form(address) {
        return false;
    }
    if git_address(address, false).is_some_and(|(host, _)| is_github_host(&host))
        && rewritten_form(address).contains('*')
    {
        return true;
    }
    // `NJe` only rewrites the host and the credentials, and a host GitHub accepts never holds a wildcard, so the
    // path of the address as parsed decides.
    url::Url::parse(address).is_ok_and(|url| {
        url.host_str()
            .is_some_and(|host| !host.is_empty() && is_github_host(host))
            && url.path().contains('*')
    })
}

/// `LJe(address, {stripDotGit: true})` for an address holding `://`: the address with its GitHub host made
/// canonical, credentials, query and fragment removed and its path decoded and collapsed.
fn rewritten_form(address: &str) -> String {
    let Some(mut url) = rewritten_url(address) else {
        return address.to_owned();
    };
    let host = github_alias(url.host_str().unwrap_or_default());
    let _ = url.set_host(Some(&host));
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.set_query(None);
    url.set_fragment(None);
    if let Some(decoded) = decode_uri_component(url.path()) {
        url.set_path(&decoded);
    }
    let collapsed = collapse_segments(url.path());
    url.set_path(strip_dot_git(&collapsed));
    url.into()
}

/// `DJe`: the path segments with `.` dropped, `..` removing the segment before it and empty segments removed.
fn collapse_segments(path: &str) -> String {
    let mut kept: Vec<&str> = Vec::new();
    for segment in path.split('/') {
        match segment {
            "." => {}
            ".." => {
                kept.pop();
            }
            other => kept.push(other),
        }
    }
    kept.retain(|segment| !segment.is_empty());
    kept.join("/")
}

/// `IJe`: the path without trailing slashes and `.git` suffixes, repeatedly.
fn strip_dot_git(path: &str) -> &str {
    let mut end = path.len();
    loop {
        let trimmed = path[..end].trim_end_matches('/');
        let trimmed = trimmed.strip_suffix(".git").unwrap_or(trimmed);
        if trimmed.len() == end {
            return trimmed;
        }
        end = trimmed.len();
    }
}

/// JavaScript `decodeURIComponent`, or `None` where it throws a `URIError`.
fn decode_uri_component(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while let Some(byte) = bytes.get(at) {
        if *byte == b'%' {
            decoded.push(hex_pair(bytes, at)?);
            at += 3;
        } else {
            decoded.push(*byte);
            at += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

#[cfg(test)]
#[path = "settings_policy_marketplace_tests.rs"]
mod tests;
