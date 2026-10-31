mod admission_route;
mod caller_grant;

use serde::{Deserialize, Serialize};

pub use admission_route::AdmissionRoute;
pub use caller_grant::CallerGrant;

use crate::jwt::CallerId;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct SubjectPattern(String);

// Custom Deserialize so JSON-sourced patterns (e.g. IssuedPermissions carried
// in UserJwtClaims) run through SubjectPattern::new. Derived transparent
// Deserialize would let a permission patternserialize and mint into the
// embedded NATS user permissions without ever seeing the validator.
impl<'de> Deserialize<'de> for SubjectPattern {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(d)?;
        Self::new(raw).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SubjectPatternError {
    #[error("subject pattern must be non-empty")]
    Empty,
    #[error("subject pattern must not contain whitespace")]
    Whitespace,
}

impl SubjectPattern {
    pub fn new(pattern: impl Into<String>) -> Result<Self, SubjectPatternError> {
        let s = pattern.into();
        match Self::violation(&s) {
            Some(violation) => Err(violation),
            None => Ok(Self(s)),
        }
    }

    /// A pattern rendered only from validated subject tokens, which are never
    /// empty and never hold whitespace.
    fn from_rendered(pattern: String) -> Self {
        debug_assert!(
            Self::violation(&pattern).is_none(),
            "{pattern:?} is not a subject pattern"
        );
        Self(pattern)
    }

    fn violation(pattern: &str) -> Option<SubjectPatternError> {
        if pattern.is_empty() {
            Some(SubjectPatternError::Empty)
        } else if pattern.chars().any(char::is_whitespace) {
            Some(SubjectPatternError::Whitespace)
        } else {
            None
        }
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
    /// No subject at all, so the minted user may neither publish nor subscribe.
    pub fn none() -> Self {
        Self {
            publish_allow: Vec::new(),
            subscribe_allow: Vec::new(),
        }
    }

    /// Publish anywhere in the gateway namespace, receive replies on the
    /// caller's own `_INBOX`, and read push deliveries on its own push subject.
    /// The mint path grants a [`CallerGrant`] instead, which names the caller's
    /// principal.
    pub fn default_for_caller(caller_id: &CallerId) -> Self {
        let caller = caller_id.as_str();
        Self {
            publish_allow: vec![SubjectPattern::from_rendered("a2a.v1.gateway.>".to_owned())],
            subscribe_allow: vec![
                SubjectPattern::from_rendered(format!("_INBOX.{caller}.>")),
                SubjectPattern::from_rendered(format!("a2a.v1.push.{caller}.>")),
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

#[derive(Debug, thiserror::Error)]
pub enum TemplateError {
    #[error("unknown ACL template placeholder: {{{0}}}")]
    UnknownPlaceholder(String),
    #[error("ACL template context missing required field: {0}")]
    MissingValue(&'static str),
    #[error("ACL template has an unclosed '{{' placeholder")]
    UnclosedPlaceholder,
    #[error("materialized subject is invalid: {0}")]
    InvalidSubject(SubjectPatternError),
    /// A placeholder value contained characters that would break out of the
    /// rendered subject segment (`.`, `*`, `>`, whitespace, or an empty
    /// string).
    #[error("ACL template placeholder {{{0}}} value would escape its subject segment")]
    InvalidPlaceholderValue(String),
}

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
        let raw = match name.as_str() {
            "caller" => ctx.caller.ok_or(TemplateError::MissingValue("caller"))?,
            "aud" => ctx.aud.ok_or(TemplateError::MissingValue("aud"))?,
            "sub" => ctx.sub.ok_or(TemplateError::MissingValue("sub"))?,
            "iss" => ctx.iss.ok_or(TemplateError::MissingValue("iss"))?,
            other => return Err(TemplateError::UnknownPlaceholder(other.to_string())),
        };
        // Placeholder values are interpolated into NATS subject patterns. Any
        // `.`, `*`, `>`, whitespace or empty value would expand the rendered
        // pattern's scope or produce an unparseable subject — fail closed
        // before the rendered string ever reaches SubjectPattern::new.
        validate_placeholder_value(name.as_str(), raw)?;
        out.push_str(raw);
    }
    Ok(out)
}

fn validate_placeholder_value(name: &str, value: &str) -> Result<(), TemplateError> {
    if value.is_empty()
        || value.contains('.')
        || value.contains('*')
        || value.contains('>')
        || value.chars().any(char::is_whitespace)
    {
        return Err(TemplateError::InvalidPlaceholderValue(name.to_string()));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
