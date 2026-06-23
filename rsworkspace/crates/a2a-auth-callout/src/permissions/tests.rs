use super::*;

#[test]
fn subject_pattern_rejects_empty() {
    assert!(matches!(
        SubjectPattern::new("").unwrap_err(),
        SubjectPatternError::Empty
    ));
}

#[test]
fn subject_pattern_rejects_whitespace() {
    assert!(matches!(
        SubjectPattern::new("a b").unwrap_err(),
        SubjectPatternError::Whitespace
    ));
}

#[test]
fn default_permissions_scope_publish_to_gateway() {
    let caller = CallerId::new("usr1").unwrap();
    let perms = IssuedPermissions::default_for_caller(&caller);
    assert_eq!(perms.publish_allow.len(), 1);
    assert_eq!(perms.publish_allow[0].as_str(), "a2a.gateway.>");
}

#[test]
fn default_permissions_subscribe_to_caller_namespaces() {
    let caller = CallerId::new("usr1").unwrap();
    let perms = IssuedPermissions::default_for_caller(&caller);
    let subjects: Vec<&str> = perms.subscribe_allow.iter().map(SubjectPattern::as_str).collect();
    assert!(subjects.contains(&"_INBOX.usr1.>"));
    assert!(subjects.contains(&"a2a.push.usr1.>"));
}

#[test]
fn template_materializes_all_placeholders() {
    let tmpl = SubjectAclTemplate::new(
        vec!["a2a.tenant-{aud}.agent-{sub}.>".into()],
        vec!["_INBOX.{caller}.>".into()],
    );
    let ctx = SubjectAclContext {
        caller: Some("usr1"),
        aud: Some("acme"),
        sub: Some("agent-7"),
        iss: Some("https://ps.example"),
    };
    let perms = tmpl.materialize(&ctx).expect("materialize");
    assert_eq!(perms.publish_allow[0].as_str(), "a2a.tenant-acme.agent-agent-7.>");
    assert_eq!(perms.subscribe_allow[0].as_str(), "_INBOX.usr1.>");
}

#[test]
fn template_rejects_unknown_placeholder() {
    let tmpl = SubjectAclTemplate::new(vec!["a2a.{nope}.>".into()], vec![]);
    let ctx = SubjectAclContext::default();
    let err = tmpl.materialize(&ctx).unwrap_err();
    assert!(matches!(err, TemplateError::UnknownPlaceholder(ref n) if n == "nope"));
}

#[test]
fn template_rejects_missing_context_value() {
    let tmpl = SubjectAclTemplate::new(vec!["a2a.tenant-{aud}.>".into()], vec![]);
    let ctx = SubjectAclContext {
        caller: Some("usr1"),
        ..Default::default()
    };
    let err = tmpl.materialize(&ctx).unwrap_err();
    assert!(matches!(err, TemplateError::MissingValue("aud")));
}

#[test]
fn template_rejects_unclosed_placeholder() {
    let tmpl = SubjectAclTemplate::new(vec!["a2a.{aud".into()], vec![]);
    let ctx = SubjectAclContext {
        aud: Some("acme"),
        ..Default::default()
    };
    let err = tmpl.materialize(&ctx).unwrap_err();
    assert!(matches!(err, TemplateError::UnclosedPlaceholder));
}

#[test]
fn template_rejects_materialized_subject_with_whitespace() {
    let tmpl = SubjectAclTemplate::new(vec!["a2a.{aud}".into()], vec![]);
    let ctx = SubjectAclContext {
        aud: Some("bad value"),
        ..Default::default()
    };
    let err = tmpl.materialize(&ctx).unwrap_err();
    // Whitespace now fails on the placeholder validator before the
    // rendered subject reaches SubjectPattern::new — same fail-closed
    // intent, earlier catch point.
    assert!(matches!(err, TemplateError::InvalidPlaceholderValue(ref n) if n == "aud"));
}

#[test]
fn template_rejects_wildcard_and_dotted_placeholder_values() {
    let tmpl = SubjectAclTemplate::new(vec![], vec!["_INBOX.{caller}.>".into()]);
    for bad in ["", "*", ">", "alice.evil"] {
        let ctx = SubjectAclContext {
            caller: Some(bad),
            ..Default::default()
        };
        let err = tmpl.materialize(&ctx).unwrap_err();
        assert!(
            matches!(err, TemplateError::InvalidPlaceholderValue(ref n) if n == "caller"),
            "expected InvalidPlaceholderValue for caller={bad:?}"
        );
    }
}
