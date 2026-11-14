use super::*;

struct Guessing(Option<&'static str>);

impl GitContextProbe for Guessing {
    fn parse_repository(&self, url: &str) -> Option<GitRepository> {
        parse_repository(url)
    }

    async fn guess_default_branch(&self) -> Option<String> {
        self.0.map(str::to_owned)
    }
}

fn context(branch: Option<&str>, default_branch: Option<&str>) -> CodeSessionGitContext {
    CodeSessionGitContext {
        git_repo_url: "https://github.com/acme/widgets".to_owned(),
        branch: branch.map(str::to_owned),
        default_branch: default_branch.map(str::to_owned),
    }
}

#[test]
fn parses_repository_addresses() {
    let repo = |host: &str, owner: &str, name: &str| {
        Some(GitRepository {
            host: host.to_owned(),
            owner: owner.to_owned(),
            name: name.to_owned(),
        })
    };
    let cases = [
        (
            " git@github.com:acme/widgets.git/ ",
            repo("github.com", "acme", "widgets"),
        ),
        (
            "https://user@gitlab.example.com:8443/acme/widgets.git",
            repo("gitlab.example.com:8443", "acme", "widgets"),
        ),
        (
            "ssh://git@gitlab.example.com:22/acme/widgets",
            repo("gitlab.example.com", "acme", "widgets"),
        ),
        ("git://host.org/acme/widgets/", repo("host.org", "acme", "widgets")),
        (
            "https://my-host.org/acme/widgets",
            repo("my-host.org", "acme", "widgets"),
        ),
        ("acme/widgets.git", repo("github.com", "acme", "widgets")),
        ("acme/widgets", repo("github.com", "acme", "widgets")),
        ("git@nodot:acme/widgets", None),
        ("git@host.org:-acme/widgets", None),
        ("git@host.org:acme/..", None),
        ("git@host.org:a/b/c", None),
        ("https://host.123/acme/widgets", None),
        ("https://-host.org/acme/widgets", None),
        ("https://host.org/acme/wid gets", None),
        ("ftp://host.org/acme/widgets", None),
        ("a@b/c", None),
        ("acme", None),
        ("acme/widgets/extra", None),
        ("/widgets", None),
        ("acme/", None),
        ("", None),
    ];
    for (input, expected) in cases {
        assert_eq!(parse_repository(input), expected, "{input}");
        assert_eq!(DefaultGitProbe.parse_repository(input), expected, "{input}");
    }
}

#[tokio::test]
async fn the_default_probe_never_guesses() {
    assert_eq!(DefaultGitProbe.guess_default_branch().await, None);
}

#[tokio::test]
async fn guesses_the_revision_when_nothing_else_names_one() {
    let (source, outcome) = git_entries(&Guessing(Some("trunk")), &context(None, None), None)
        .await
        .unwrap_or_default();
    assert_eq!(source["revision"], "trunk");
    assert_eq!(outcome["git_info"]["branches"], json!([]));
    let (source, _) = git_entries(&Guessing(None), &context(None, None), None)
        .await
        .unwrap_or_default();
    assert!(source.get("revision").is_none());
    let (source, _) = git_entries(&Guessing(Some("trunk")), &context(Some("HEAD"), None), None)
        .await
        .unwrap_or_default();
    assert_eq!(source["revision"], "trunk");
}

#[tokio::test]
async fn sends_a_branch_that_differs_from_the_default() {
    let (source, outcome) = git_entries(&DefaultGitProbe, &context(Some("feat"), Some("main")), None)
        .await
        .unwrap_or_default();
    assert_eq!(source["revision"], "feat");
    assert_eq!(outcome["git_info"]["branches"], json!(["feat"]));
}

#[tokio::test]
async fn drops_log_lines_that_repeat() {
    let mut dedup = BranchDropLogDedup::default();
    assert_eq!(dedup.last_key(), None);
    assert_eq!(BranchDropLogDedup::with_last_key("k").last_key(), Some("k"));
    let same = context(Some("main"), Some("main"));
    git_entries(&DefaultGitProbe, &same, Some(&mut dedup)).await;
    assert_eq!(
        dedup.last_key(),
        Some("is_default:https://github.com/acme/widgets:main")
    );
    git_entries(&DefaultGitProbe, &same, Some(&mut dedup)).await;
    assert_eq!(
        dedup.last_key(),
        Some("is_default:https://github.com/acme/widgets:main")
    );
    git_entries(&DefaultGitProbe, &context(Some("feat"), None), Some(&mut dedup)).await;
    assert_eq!(
        dedup.last_key(),
        Some("no_evidence:https://github.com/acme/widgets:feat")
    );
    git_entries(&DefaultGitProbe, &context(Some("feat"), Some("main")), Some(&mut dedup)).await;
    assert_eq!(dedup.last_key(), None);
    git_entries(&DefaultGitProbe, &same, None).await;
}

#[tokio::test]
async fn clears_the_dedup_when_the_address_is_unusable() {
    let mut dedup = BranchDropLogDedup::new();
    git_entries(&DefaultGitProbe, &context(Some("main"), Some("main")), Some(&mut dedup)).await;
    let mut empty = context(Some("main"), Some("main"));
    empty.git_repo_url.clear();
    assert!(git_entries(&DefaultGitProbe, &empty, Some(&mut dedup)).await.is_none());
    assert_eq!(dedup.last_key(), None);
    let mut bad = empty.clone();
    bad.git_repo_url = "nonsense".to_owned();
    assert!(git_entries(&DefaultGitProbe, &bad, None).await.is_none());
}
