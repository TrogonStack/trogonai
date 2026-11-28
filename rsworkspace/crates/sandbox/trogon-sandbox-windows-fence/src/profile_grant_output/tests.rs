use super::*;
use crate::caller_reads::CallerReads;
use crate::grant_entry::GrantEntry;

#[test]
fn args_in_order() {
    let a = profile_grant_args(&["a".to_string(), "b".to_string()], true);
    assert_eq!(
        a,
        [
            "profile",
            "grant",
            "--json",
            "--exclude",
            "a",
            "--exclude",
            "b",
            "--replace-excludes"
        ]
    );
    assert_eq!(profile_grant_args(&[], false), ["profile", "grant", "--json"]);
}

#[test]
fn parses_result_and_item_details() -> Result<(), Box<dyn std::error::Error>> {
    let json = r#"{"profile":"C:\\Users\\u","containers":9,"granted":8,"files":100,"denied":3,
      "placeholders":1,"writes":4,"unchanged":2,"removed":0,
      "skipped":[{"path":"j","reason":"junction"}],
      "unverifiable":[{"path":"u","reason":"r","caller_reads":"unknown","cause":"acl_unreadable"},
                      {"path":"v","caller_reads":true,"entry":"inherited","protected_exclusion":false}],
      "elapsed_ms":1234}"#;
    let r = parse_profile_grant(&profile_grant_args(&[], false), json)?;
    assert_eq!((r.granted, r.denied, r.elapsed_ms), (8, 3, 1234));
    assert_eq!(r.skipped.len(), 1);
    assert_eq!(r.unverifiable.len(), 2);
    assert_eq!(r.unverifiable[0].caller_reads, Some(CallerReads::Unknown));
    assert_eq!(r.unverifiable[0].cause.as_deref(), Some("acl_unreadable"));
    assert_eq!(r.unverifiable[1].entry, Some(GrantEntry::Inherited));
    assert_eq!(r.unverifiable[1].protected_exclusion, Some(false));
    assert!(r.stale_allow.is_empty() && r.not_granted.is_empty());
    Ok(())
}

#[test]
fn bad_json_is_typed() -> Result<(), Box<dyn std::error::Error>> {
    let Err(e) = parse_profile_grant(&profile_grant_args(&[], false), "x") else {
        return Err("expected an error".into());
    };
    assert_eq!(e.code(), Code::FenceHelperBadJson);
    assert_eq!(e.subcommand(), Some("profile"));
    Ok(())
}

#[test]
fn a_malformed_item_list_is_refused() {
    for field in ["skipped", "not_granted", "stale_allow"] {
        for bad in [r#""not a list""#, r#"[{"reason":"no path"}]"#] {
            let json = format!(
                r#"{{"profile":"p","containers":0,"granted":0,"files":0,"denied":0,"placeholders":0,
                "writes":0,"unchanged":0,"removed":0,"{field}":{bad},"elapsed_ms":1}}"#
            );
            let result = parse_profile_grant(&profile_grant_args(&[], false), &json);
            assert_eq!(
                result.err().map(|e| e.code()),
                Some(Code::FenceHelperBadJson),
                "{field} {bad}"
            );
        }
    }
}
