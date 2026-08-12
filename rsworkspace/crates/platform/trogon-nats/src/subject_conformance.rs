//! Whole-subject conformance checks for ADR#0055.
//!
//! [`subject_token_violation`](crate::subject_token_violation) validates a
//! single token. Nothing validated a whole subject, so ADR#0055's token-count,
//! byte, and layout limits had no home. This is that home.
//!
//! Two entry points, because the rules differ by role.
//!
//! A subject a binding *publishes* to is a concrete address: no wildcards, and
//! its dynamic positions hold opaque values (session ids, task ids, agent
//! names) that the grammar does not get to spell. So it is checked for shape
//! and limits, not for lower_snake.
//!
//! A subject used as a subscription or JetStream `filter_subject` is a
//! *pattern*: `*` occupies each dynamic position and `>` may occupy the final
//! one, which means every remaining literal token is grammar. That is exactly
//! where ADR#0055's "tokens are lower_snake and case-consistent" is checkable,
//! so patterns get the stricter pass.

use crate::subject_token_violation::SubjectTokenViolationError;

/// ADR#0055 Limits: at most 16 tokens per subject.
///
/// The hard ceiling before NATS escapes to the heap is 32; 16 is the budget
/// this profile spends, leaving room for method arity and durable suffixes.
pub const MAX_SUBJECT_TOKENS: usize = 16;

/// ADR#0055 Limits: at most 256 bytes per subject.
pub const MAX_SUBJECT_BYTES: usize = 256;

/// The reserved token introducing ADR#0055's method-to-terminal escape encoding
/// (`custom.{base64url}`). The token following it is the sole exemption from
/// lower_snake; the wildcard, token-count, and byte limits still bind it.
pub const ESCAPE_TOKEN: &str = "custom";

/// Why a subject failed ADR#0055 conformance.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum SubjectViolationError {
    #[error("subject is empty")]
    Empty,
    #[error("subject is flat; ADR#0055 requires a hierarchical subject")]
    Flat,
    #[error("subject has {count} tokens, exceeding the {MAX_SUBJECT_TOKENS}-token budget")]
    TooManyTokens { count: usize },
    #[error("subject is {bytes} bytes, exceeding the {MAX_SUBJECT_BYTES}-byte budget")]
    TooLong { bytes: usize },
    #[error("subject has a leading, trailing, or doubled dot")]
    MalformedDots,
    #[error("token {index} ({token:?}) is invalid: {source}")]
    Token {
        index: usize,
        token: String,
        source: SubjectTokenViolationError,
    },
    #[error("token {index} ({token:?}) is not lower_snake")]
    NotLowerSnake { index: usize, token: String },
    #[error("token {index} is a wildcard; a published subject must be a concrete address")]
    WildcardInPublishedSubject { index: usize },
    #[error("token {index} ({token:?}) mixes a wildcard character with other characters")]
    PartialWildcard { index: usize, token: String },
    #[error("the `>` wildcard is only valid as the final token, found at token {index}")]
    TrailingWildcardNotLast { index: usize },
    #[error("token {index} ({token:?}) looks like a request id; ADR#0055 keeps correlation in headers")]
    RequestIdToken { index: usize, token: String },
    #[error("expected a `v{{major}}` binding-version token after the prefix, found {found:?}")]
    MissingBindingVersion { found: Option<String> },
    #[error("subject does not start with the expected prefix {prefix:?}")]
    PrefixMismatch { prefix: String },
}

/// Validates a concrete subject a binding publishes to.
///
/// Checks ADR#0055's shape and limits. Wildcards are rejected: a published
/// subject is an address, not a pattern. lower_snake is deliberately not
/// enforced, because the dynamic positions carry opaque identifiers.
pub fn validate_published_subject(subject: &str) -> Result<(), SubjectViolationError> {
    validate(subject, Role::Published)
}

/// Validates a subscription or `filter_subject` pattern.
///
/// `*` may occupy a whole token and `>` may occupy the final token. Every other
/// token is grammar and must be lower_snake, except the token following
/// [`ESCAPE_TOKEN`].
pub fn validate_subject_pattern(subject: &str) -> Result<(), SubjectViolationError> {
    validate(subject, Role::Pattern)
}

/// Asserts the token immediately following `prefix` is a `v{major}` binding
/// version, per ADR#0055's versioning posture.
///
/// The prefix may itself be dotted (`AcpPrefix` permits `my.multi.part`), so the
/// version position is derived from the prefix rather than assumed to be index 1.
pub fn validate_binding_version(subject: &str, prefix: &str) -> Result<(), SubjectViolationError> {
    let rest = subject
        .strip_prefix(prefix)
        .and_then(|rest| rest.strip_prefix('.'))
        .ok_or_else(|| SubjectViolationError::PrefixMismatch {
            prefix: prefix.to_owned(),
        })?;

    match rest.split('.').next() {
        Some(token) if is_binding_version(token) => Ok(()),
        other => Err(SubjectViolationError::MissingBindingVersion {
            found: other.map(str::to_owned),
        }),
    }
}

/// Rejects any token shaped like a per-request correlation value.
///
/// ADR#0055 keeps correlation in headers, not in the subject, because a subject
/// token that varies per request cannot be a stable consumer filter and forces
/// a consumer per request.
///
/// This is opt-in per subject family, not a blanket rule, because shape alone
/// cannot separate a per-request id from a durable resource id: an A2A task id
/// is UUID-shaped and legitimately routes `a2a.v1.tasks.{task_id}.events`. Apply
/// it only to families whose dynamic positions are known to be resource
/// identities, where a UUID-shaped token is therefore evidence of a leak.
pub fn validate_no_request_id_tokens(subject: &str) -> Result<(), SubjectViolationError> {
    match subject.split('.').enumerate().find(|(_, t)| looks_like_request_id(t)) {
        Some((index, token)) => Err(SubjectViolationError::RequestIdToken {
            index,
            token: token.to_owned(),
        }),
        None => Ok(()),
    }
}

/// True if a token has the shape of a generated request id.
///
/// Covers both forms this workspace mints: the hyphenated UUID `acp-nats`
/// produces via `Uuid::now_v7().to_string()`, and the 32-hex simple form
/// `a2a-nats` produces via `Uuid::now_v7().simple().to_string()`.
pub fn looks_like_request_id(token: &str) -> bool {
    is_hyphenated_uuid(token) || is_simple_uuid(token)
}

fn is_hex(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|b| b.is_ascii_hexdigit())
}

fn is_hyphenated_uuid(token: &str) -> bool {
    let groups: Vec<&str> = token.split('-').collect();
    groups.len() == 5 && groups.iter().map(|g| g.len()).eq([8, 4, 4, 4, 12]) && groups.iter().all(|g| is_hex(g))
}

fn is_simple_uuid(token: &str) -> bool {
    token.len() == 32 && is_hex(token)
}

fn is_binding_version(token: &str) -> bool {
    token
        .strip_prefix('v')
        .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()))
}

/// Grammar tokens are lowercase ASCII, digits, and `_`. No `-`: ADR#0055 says
/// lower_snake, and allowing `-` would let a hyphenated id pass as grammar.
fn is_lower_snake(token: &str) -> bool {
    token
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    Published,
    Pattern,
}

fn validate(subject: &str, role: Role) -> Result<(), SubjectViolationError> {
    if subject.is_empty() {
        return Err(SubjectViolationError::Empty);
    }
    if crate::token::has_consecutive_or_boundary_dots(subject) {
        return Err(SubjectViolationError::MalformedDots);
    }
    if subject.len() > MAX_SUBJECT_BYTES {
        return Err(SubjectViolationError::TooLong { bytes: subject.len() });
    }

    let tokens: Vec<&str> = subject.split('.').collect();
    if tokens.len() < 2 {
        return Err(SubjectViolationError::Flat);
    }
    if tokens.len() > MAX_SUBJECT_TOKENS {
        return Err(SubjectViolationError::TooManyTokens { count: tokens.len() });
    }

    let last = tokens.len() - 1;
    for (index, token) in tokens.iter().enumerate() {
        let preceded_by_escape = index > 0 && tokens[index - 1] == ESCAPE_TOKEN;
        validate_token(token, index, last, role, preceded_by_escape)?;
    }
    Ok(())
}

fn validate_token(
    token: &str,
    index: usize,
    last: usize,
    role: Role,
    preceded_by_escape: bool,
) -> Result<(), SubjectViolationError> {
    if token.is_empty() {
        return Err(SubjectViolationError::Token {
            index,
            token: token.to_owned(),
            source: SubjectTokenViolationError::Empty,
        });
    }

    if token == "*" || token == ">" {
        return match role {
            Role::Published => Err(SubjectViolationError::WildcardInPublishedSubject { index }),
            Role::Pattern if token == ">" && index != last => {
                Err(SubjectViolationError::TrailingWildcardNotLast { index })
            }
            Role::Pattern => Ok(()),
        };
    }

    if let Some(ch) = crate::token::has_wildcards_or_whitespace(token) {
        return Err(if ch == '*' || ch == '>' {
            SubjectViolationError::PartialWildcard {
                index,
                token: token.to_owned(),
            }
        } else {
            SubjectViolationError::Token {
                index,
                token: token.to_owned(),
                source: SubjectTokenViolationError::InvalidCharacter(ch),
            }
        });
    }

    if role == Role::Pattern && !preceded_by_escape && !is_lower_snake(token) {
        return Err(SubjectViolationError::NotLowerSnake {
            index,
            token: token.to_owned(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests;
