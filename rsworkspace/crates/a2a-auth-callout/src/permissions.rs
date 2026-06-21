use std::fmt;

use serde::{Deserialize, Serialize};

use crate::jwt::CallerId;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SubjectPattern(String);

#[derive(Debug)]
pub enum SubjectPatternError {
    Empty,
    Whitespace,
}

impl fmt::Display for SubjectPatternError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("subject pattern must be non-empty"),
            Self::Whitespace => f.write_str("subject pattern must not contain whitespace"),
        }
    }
}

impl std::error::Error for SubjectPatternError {}

impl SubjectPattern {
    pub fn new(pattern: impl Into<String>) -> Result<Self, SubjectPatternError> {
        let s = pattern.into();
        if s.is_empty() {
            return Err(SubjectPatternError::Empty);
        }
        if s.chars().any(char::is_whitespace) {
            return Err(SubjectPatternError::Whitespace);
        }
        Ok(Self(s))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssuedPermissions {
    pub publish_allow: Vec<SubjectPattern>,
    pub subscribe_allow: Vec<SubjectPattern>,
}

impl IssuedPermissions {
    /// Caller subject ACL: publish into the gateway namespace, receive replies
    /// on its own `_INBOX`, and read push deliveries on its own caller-scoped
    /// push subject. Mirrors `scripts/acl-templates/caller.acl`.
    // Inputs are either static literals or strings derived from a previously
    // validated `CallerId`; failure here would mean an upstream invariant
    // is broken, not a runtime condition we can recover from. The `expect`s
    // are documented and intentional — clippy denies them by default.
    #[allow(clippy::expect_used)]
    pub fn default_for_caller(caller_id: &CallerId) -> Self {
        let inbox = format!("_INBOX.{}.>", caller_id.as_str());
        let push = format!("a2a.push.{}.>", caller_id.as_str());
        Self {
            publish_allow: vec![SubjectPattern::new("a2a.gateway.>").expect("static literal")],
            subscribe_allow: vec![
                SubjectPattern::new(inbox).expect("derived from validated caller_id"),
                SubjectPattern::new(push).expect("derived from validated caller_id"),
            ],
        }
    }
}

/// Subject ACL template materializer.
///
/// A `SubjectAclTemplate` is a pair of `publish` / `subscribe` patterns with
/// `{placeholder}` substitution tokens. `materialize` resolves the placeholders
/// from a [`SubjectAclContext`] and produces an [`IssuedPermissions`] with
/// validated [`SubjectPattern`]s.
///
/// Supported placeholders:
/// - `{caller}` — the NATS caller id
/// - `{aud}`    — the resolved audience / tenant account
/// - `{sub}`    — the JWT subject claim
/// - `{iss}`    — the JWT issuer claim
///
/// Unknown placeholders or unfilled context fields fail closed so an operator
/// misconfiguration cannot silently broaden a caller's NATS surface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubjectAclTemplate {
    pub publish: Vec<String>,
    pub subscribe: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct SubjectAclContext<'a> {
    pub caller: Option<&'a str>,
    pub aud: Option<&'a str>,
    pub sub: Option<&'a str>,
    pub iss: Option<&'a str>,
}

#[derive(Debug)]
pub enum TemplateError {
    UnknownPlaceholder(String),
    MissingValue(&'static str),
    UnclosedPlaceholder,
    InvalidSubject(SubjectPatternError),
}

impl fmt::Display for TemplateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownPlaceholder(name) => write!(f, "unknown ACL template placeholder: {{{name}}}"),
            Self::MissingValue(name) => write!(f, "ACL template context missing required field: {name}"),
            Self::UnclosedPlaceholder => f.write_str("ACL template has an unclosed '{' placeholder"),
            Self::InvalidSubject(err) => write!(f, "materialized subject is invalid: {err}"),
        }
    }
}

impl std::error::Error for TemplateError {}

impl SubjectAclTemplate {
    #[must_use]
    pub fn new(publish: Vec<String>, subscribe: Vec<String>) -> Self {
        Self { publish, subscribe }
    }

    pub fn materialize(&self, ctx: &SubjectAclContext<'_>) -> Result<IssuedPermissions, TemplateError> {
        let publish_allow = materialize_list(&self.publish, ctx)?;
        let subscribe_allow = materialize_list(&self.subscribe, ctx)?;
        Ok(IssuedPermissions {
            publish_allow,
            subscribe_allow,
        })
    }
}

fn materialize_list(templates: &[String], ctx: &SubjectAclContext<'_>) -> Result<Vec<SubjectPattern>, TemplateError> {
    let mut out = Vec::with_capacity(templates.len());
    for tmpl in templates {
        let rendered = substitute_placeholders(tmpl, ctx)?;
        out.push(SubjectPattern::new(rendered).map_err(TemplateError::InvalidSubject)?);
    }
    Ok(out)
}

fn substitute_placeholders(template: &str, ctx: &SubjectAclContext<'_>) -> Result<String, TemplateError> {
    let mut out = String::with_capacity(template.len());
    let mut iter = template.chars().peekable();
    while let Some(c) = iter.next() {
        if c != '{' {
            out.push(c);
            continue;
        }
        let mut name = String::new();
        let mut closed = false;
        for next in iter.by_ref() {
            if next == '}' {
                closed = true;
                break;
            }
            name.push(next);
        }
        if !closed {
            return Err(TemplateError::UnclosedPlaceholder);
        }
        let value = match name.as_str() {
            "caller" => ctx.caller.ok_or(TemplateError::MissingValue("caller"))?,
            "aud" => ctx.aud.ok_or(TemplateError::MissingValue("aud"))?,
            "sub" => ctx.sub.ok_or(TemplateError::MissingValue("sub"))?,
            "iss" => ctx.iss.ok_or(TemplateError::MissingValue("iss"))?,
            other => return Err(TemplateError::UnknownPlaceholder(other.to_string())),
        };
        out.push_str(value);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
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
        assert!(matches!(
            err,
            TemplateError::InvalidSubject(SubjectPatternError::Whitespace)
        ));
    }
}
