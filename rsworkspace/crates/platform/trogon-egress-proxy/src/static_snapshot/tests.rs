use std::time::SystemTime;

use super::*;

fn source() -> PolicySource {
    PolicySource::StaticFile {
        path: PathBuf::from("policy.toml"),
    }
}

fn load(text: &str) -> Result<StaticSnapshot, SnapshotError> {
    StaticSnapshot::from_toml(source(), text)
}

fn agent(raw: &str) -> AgentId {
    AgentId::new(raw).expect("agent")
}

fn destination(raw: &str) -> Destination {
    raw.parse().expect("destination")
}

fn verdict(snapshot: &StaticSnapshot, agent_id: &str, raw: &str) -> SnapshotVerdict {
    snapshot.lookup(&agent(agent_id), &destination(raw), SystemTime::now())
}

const VALID: &str = r#"
[agents.agent-1]
allow = ["api.example.com:443", "10.0.0.5:8080", "[2001:db8::1]:443"]

[agents."agent.two"]
allow = ["other.example.com:443"]
"#;

#[test]
fn permits_exactly_the_listed_destinations_per_agent() {
    let snapshot = load(VALID).expect("valid policy");
    assert_eq!(snapshot.agent_count(), 2);
    assert_eq!(snapshot.permit_count(), 4);
    assert_eq!(
        verdict(&snapshot, "agent-1", "api.example.com:443"),
        SnapshotVerdict::Permitted
    );
    assert_eq!(
        verdict(&snapshot, "agent-1", "API.example.com:443"),
        SnapshotVerdict::Permitted
    );
    assert_eq!(
        verdict(&snapshot, "agent-1", "10.0.0.5:8080"),
        SnapshotVerdict::Permitted
    );
    assert_eq!(
        verdict(&snapshot, "agent-1", "[2001:db8::1]:443"),
        SnapshotVerdict::Permitted
    );
    assert_eq!(
        verdict(&snapshot, "agent.two", "other.example.com:443"),
        SnapshotVerdict::Permitted
    );
}

#[test]
fn canonical_application_identity_is_preserved_as_the_policy_principal() {
    let id = "did:key:z6MkhaXgBZDvotDkL5257faiztiGi2sfKfzvuU5TVndZqquw";
    let policy = format!("[agents.\"{id}\"]\nallow = [\"api.example.com:443\"]\n");
    let snapshot = load(&policy).expect("canonical application identities are valid policy principals");
    assert_eq!(verdict(&snapshot, id, "api.example.com:443"), SnapshotVerdict::Permitted);
    assert_eq!(verdict(&snapshot, "agent-1", "api.example.com:443"), SnapshotVerdict::NotPermitted);
}

#[test]
fn anything_else_is_not_permitted() {
    let snapshot = load(VALID).expect("valid policy");
    for (who, raw) in [
        ("agent-1", "api.example.com:8443"),
        ("agent-1", "sub.api.example.com:443"),
        ("agent-1", "example.com:443"),
        ("agent-1", "10.0.0.6:8080"),
        ("agent-1", "other.example.com:443"),
        ("agent.two", "api.example.com:443"),
        ("unknown-agent", "api.example.com:443"),
    ] {
        assert_eq!(
            verdict(&snapshot, who, raw),
            SnapshotVerdict::NotPermitted,
            "{who} {raw}"
        );
    }
}

#[test]
fn a_name_permit_does_not_permit_an_address_and_back() {
    let snapshot = load(
        r#"
[agents.a]
allow = ["localhost:443"]
"#,
    )
    .expect("valid");
    assert_eq!(verdict(&snapshot, "a", "127.0.0.1:443"), SnapshotVerdict::NotPermitted);
}

#[test]
fn duplicate_entries_collapse() {
    let snapshot = load(
        r#"
[agents.a]
allow = ["example.com:443", "EXAMPLE.com:443"]
"#,
    )
    .expect("valid");
    assert_eq!(snapshot.permit_count(), 1);
}

#[test]
fn empty_and_whitespace_files_permit_nothing() {
    assert!(matches!(load(""), Err(SnapshotError::NoAgents)));
    assert!(matches!(load("  \n\n# only a comment\n"), Err(SnapshotError::NoAgents)));
    assert!(matches!(load("[agents]\n"), Err(SnapshotError::NoAgents)));
}

#[test]
fn an_agent_with_an_empty_allow_list_is_refused() {
    let error = load("[agents.a]\nallow = []\n").expect_err("empty list");
    assert!(matches!(error, SnapshotError::EmptyPermitList { agent } if agent.as_str() == "a"));
}

#[test]
fn unparsable_files_and_unknown_fields_are_refused() {
    assert!(matches!(
        load("this is = = not toml"),
        Err(SnapshotError::Unparsable { .. })
    ));
    assert!(matches!(
        load("[agents.a]\nallow = [\"example.com:443\"]\ndeny = []\n"),
        Err(SnapshotError::Unparsable { .. })
    ));
    assert!(matches!(
        load("[agents.a]\nallow = [\"example.com:443\"]\n[extra]\nx = 1\n"),
        Err(SnapshotError::Unparsable { .. })
    ));
    assert!(matches!(
        load("[agents.a]\nallow = \"example.com:443\"\n"),
        Err(SnapshotError::Unparsable { .. })
    ));
}

#[test]
fn invalid_destinations_are_refused_with_the_offending_entry() {
    for entry in [
        "*",
        "*:443",
        "*.example.com:443",
        "example.com",
        "example.com:0",
        "example.com:65536",
        "example.com/path:443",
        "exa mple.com:443",
        "https://example.com:443",
        "",
    ] {
        let text = format!("[agents.a]\nallow = [\"{entry}\"]\n");
        match load(&text) {
            Err(SnapshotError::InvalidDestination { entry: reported, .. }) => assert_eq!(reported, entry),
            other => panic!("{entry:?} must be refused, got {other:?}"),
        }
    }
}

#[test]
fn invalid_agent_ids_are_refused() {
    let error = load("[agents.\"a/b\"]\nallow = [\"example.com:443\"]\n").expect_err("bad agent");
    assert!(matches!(error, SnapshotError::InvalidAgent { agent, .. } if agent == "a/b"));
}

#[test]
fn loading_a_missing_file_is_a_typed_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let error = StaticSnapshot::load(&dir.path().join("absent.toml")).expect_err("missing file");
    assert!(matches!(error, SnapshotError::Unreadable { .. }));
}

#[test]
fn loading_a_directory_is_a_typed_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let error = StaticSnapshot::load(dir.path()).expect_err("directory");
    assert!(matches!(error, SnapshotError::Unreadable { .. }));
}

#[test]
fn loading_a_valid_file_records_its_path_as_the_source() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("policy.toml");
    std::fs::write(&path, VALID).expect("write policy");
    let snapshot = StaticSnapshot::load(&path).expect("load");
    assert_eq!(snapshot.source(), &PolicySource::StaticFile { path });
    assert_eq!(snapshot.source().kind(), "static_file");
}

#[test]
fn loading_an_empty_file_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("policy.toml");
    std::fs::write(&path, "").expect("write policy");
    assert!(matches!(StaticSnapshot::load(&path), Err(SnapshotError::NoAgents)));
}
