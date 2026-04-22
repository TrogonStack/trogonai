/// Sanitize an arbitrary string so it is safe to use as a NATS subject token.
///
/// Rules:
/// - `/` → `.`  (path separators become dots; e.g. `owner/repo/456` → `owner.repo.456`)
/// - Alphanumeric, `-`, `_`, `.` → kept as-is
/// - Anything else → `-`
pub fn sanitize_key(key: &str) -> String {
    key.chars()
        .map(|c| match c {
            '/' => '.',
            c if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' => c,
            _ => '-',
        })
        .collect()
}

/// The subject for one session's transcript entries.
///
/// Pattern: `transcripts.{actor_type}.{actor_key}.{session_id}`
pub fn transcript_subject(actor_type: &str, actor_key: &str, session_id: &str) -> String {
    format!(
        "transcripts.{}.{}.{}",
        sanitize_key(actor_type),
        sanitize_key(actor_key),
        session_id,
    )
}

/// A wildcard subject filter that matches every session for a given entity.
///
/// Pattern: `transcripts.{actor_type}.{actor_key}.>`
pub fn entity_subject_filter(actor_type: &str, actor_key: &str) -> String {
    format!(
        "transcripts.{}.{}.>",
        sanitize_key(actor_type),
        sanitize_key(actor_key),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slashes_become_dots() {
        assert_eq!(sanitize_key("owner/repo/123"), "owner.repo.123");
    }

    #[test]
    fn spaces_become_dashes() {
        assert_eq!(sanitize_key("hello world"), "hello-world");
    }

    #[test]
    fn alphanumeric_and_separators_preserved() {
        assert_eq!(sanitize_key("pr-review_2.0"), "pr-review_2.0");
    }

    #[test]
    fn colons_become_dashes() {
        assert_eq!(sanitize_key("owner:repo"), "owner-repo");
    }

    #[test]
    fn transcript_subject_format() {
        let s = transcript_subject("pr", "owner/repo/456", "sess-123");
        assert_eq!(s, "transcripts.pr.owner.repo.456.sess-123");
    }

    #[test]
    fn entity_subject_filter_format() {
        let s = entity_subject_filter("pr", "owner/repo/456");
        assert_eq!(s, "transcripts.pr.owner.repo.456.>");
    }
}
