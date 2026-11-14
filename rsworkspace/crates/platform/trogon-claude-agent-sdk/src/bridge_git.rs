use std::future::Future;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Value, json};

use crate::bridge_js::js_trim;

static SCP: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"^git@([^:/@]+):([^/]+)/([^/]+?)(?:\.git)?/?$").ok());
static URL: LazyLock<Option<Regex>> = LazyLock::new(|| {
    Regex::new(r"^(https?|ssh|git)://(?:[^@/?#]*@)?([^/:?#@]+(?::[0-9]+)?)/([^/]+)/([^/]+?)(?:\.git)?/?$").ok()
});

/// What the caller knows about the repository a code session works in.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CodeSessionGitContext {
    pub git_repo_url: String,
    pub branch: Option<String>,
    pub default_branch: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum DropLogKey {
    Unset,
    Cleared,
    Key(String),
}

/// Remembers the last "branch dropped" log line so a loop creating sessions does not repeat it.
#[derive(Clone, Debug)]
pub struct BranchDropLogDedup {
    last_key: DropLogKey,
}

impl BranchDropLogDedup {
    pub fn new() -> Self {
        Self {
            last_key: DropLogKey::Unset,
        }
    }

    /// A dedup that already logged the drop identified by `key`.
    pub fn with_last_key(key: impl Into<String>) -> Self {
        Self {
            last_key: DropLogKey::Key(key.into()),
        }
    }

    pub fn last_key(&self) -> Option<&str> {
        match &self.last_key {
            DropLogKey::Key(key) => Some(key),
            _ => None,
        }
    }
}

impl Default for BranchDropLogDedup {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitRepository {
    pub host: String,
    pub owner: String,
    pub name: String,
}

/// Reads what the session creation needs to know about a repository that is not in the request itself.
pub trait GitContextProbe {
    fn parse_repository(&self, url: &str) -> Option<GitRepository>;

    /// The remote's default branch when the caller did not supply one.
    fn guess_default_branch(&self) -> impl Future<Output = Option<String>> + Send;
}

/// Parses repository addresses the way the upstream bridge does and never guesses a default branch.
#[derive(Clone, Copy, Debug, Default)]
pub struct DefaultGitProbe;

impl GitContextProbe for DefaultGitProbe {
    fn parse_repository(&self, url: &str) -> Option<GitRepository> {
        parse_repository(url)
    }

    async fn guess_default_branch(&self) -> Option<String> {
        None
    }
}

fn valid_host(host: &str) -> bool {
    !host.is_empty()
        && !host.starts_with('-')
        && host.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
        && host.contains('.')
        && host
            .rsplit('.')
            .next()
            .is_some_and(|label| !label.is_empty() && label.chars().all(|c| c.is_ascii_alphabetic()))
}

fn valid_segment(segment: &str) -> bool {
    !segment.is_empty()
        && !segment.starts_with('-')
        && segment != "."
        && segment != ".."
        && segment
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

fn repository(host: &str, owner: &str, name: &str) -> Option<GitRepository> {
    (valid_segment(owner) && valid_segment(name)).then(|| GitRepository {
        host: host.to_owned(),
        owner: owner.to_owned(),
        name: name.to_owned(),
    })
}

pub(crate) fn parse_repository(input: &str) -> Option<GitRepository> {
    let text = js_trim(input);
    if let Some(captures) = SCP.as_ref().and_then(|scp| scp.captures(text)) {
        let host = &captures[1];
        return valid_host(host)
            .then(|| repository(host, &captures[2], &captures[3]))
            .flatten();
    }
    if let Some(captures) = URL.as_ref().and_then(|url| url.captures(text)) {
        let authority = &captures[2];
        let hostname = authority.split(':').next().unwrap_or(authority);
        if !valid_host(hostname) {
            return None;
        }
        let host = if matches!(&captures[1], "http" | "https") {
            authority
        } else {
            hostname
        };
        return repository(host, &captures[3], &captures[4]);
    }
    if text.contains("://") || text.contains('@') {
        return None;
    }
    let parts: Vec<&str> = text.split('/').collect();
    let [owner, name] = parts[..] else {
        return None;
    };
    if owner.is_empty() || name.is_empty() {
        return None;
    }
    repository("github.com", owner, name.strip_suffix(".git").unwrap_or(name))
}

/// The source and outcome entries a git context adds to a session creation request, if any.
pub(crate) async fn git_entries<G: GitContextProbe>(
    probe: &G,
    context: &CodeSessionGitContext,
    dedup: Option<&mut BranchDropLogDedup>,
) -> Option<(Value, Value)> {
    let mut dedup = dedup;
    let Some(repo) = (!context.git_repo_url.is_empty())
        .then(|| probe.parse_repository(&context.git_repo_url))
        .flatten()
    else {
        if let Some(dedup) = dedup.as_deref_mut() {
            dedup.last_key = DropLogKey::Cleared;
        }
        return None;
    };
    let branch = context
        .branch
        .as_deref()
        .filter(|branch| *branch != "HEAD")
        .unwrap_or("");
    let default_branch = context.default_branch.as_deref().filter(|default| !default.is_empty());
    let guessed = if branch.is_empty() && default_branch.is_none() {
        probe.guess_default_branch().await
    } else {
        None
    };
    let revision = [Some(branch), default_branch, guessed.as_deref()]
        .into_iter()
        .flatten()
        .find(|candidate| !candidate.is_empty());
    let branches: Vec<&str> = match default_branch {
        Some(default) if !branch.is_empty() && branch != default => vec![branch],
        _ => Vec::new(),
    };
    match (branch.is_empty(), branches.is_empty()) {
        (false, true) => {
            let reason = if default_branch.is_some() {
                "is_default"
            } else {
                "no_evidence"
            };
            let key = format!("{reason}:{}:{branch}", context.git_repo_url);
            if let Some(dedup) = dedup {
                if dedup.last_key != DropLogKey::Key(key.clone()) {
                    dedup.last_key = DropLogKey::Key(key);
                    log_drop(reason, &context.git_repo_url, branch);
                }
            } else {
                log_drop(reason, &context.git_repo_url, branch);
            }
        }
        _ => {
            if let Some(dedup) = dedup {
                dedup.last_key = DropLogKey::Cleared;
            }
        }
    }
    let slug = format!("{}/{}", repo.owner, repo.name);
    let mut source = json!({
        "type": "git_repository",
        "url": format!("https://{}/{}/{}", repo.host, repo.owner, repo.name),
    });
    if let (Some(revision), Some(object)) = (revision, source.as_object_mut()) {
        object.insert("revision".to_owned(), Value::String(revision.to_owned()));
    }
    let outcome = json!({
        "type": "git_repository",
        "git_info": {"type": "github", "repo": slug, "branches": branches},
    });
    Some((source, outcome))
}

fn log_drop(reason: &str, url: &str, branch: &str) {
    tracing::debug!(
        reason,
        url,
        branch,
        "bridge: current branch not sent as an outcome branch"
    );
}

#[cfg(test)]
#[path = "bridge_git_tests.rs"]
mod tests;
