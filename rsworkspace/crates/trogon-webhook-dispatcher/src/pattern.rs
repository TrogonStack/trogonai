/// Returns `true` if `subject` matches `pattern` using NATS wildcard rules.
///
/// - `*` matches exactly one dot-separated token.
/// - `>` matches one or more tokens and must appear at the end of the pattern.
pub fn matches(pattern: &str, subject: &str) -> bool {
    let pp: Vec<&str> = pattern.split('.').collect();
    let sp: Vec<&str> = subject.split('.').collect();

    let mut pi = 0;
    let mut si = 0;

    while pi < pp.len() {
        match pp[pi] {
            ">" => return si < sp.len(),
            "*" => {
                if si >= sp.len() {
                    return false;
                }
            }
            tok => {
                if si >= sp.len() || tok != sp[si] {
                    return false;
                }
            }
        }
        pi += 1;
        si += 1;
    }

    pi == pp.len() && si == sp.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_match() {
        assert!(matches("a.b.c", "a.b.c"));
    }

    #[test]
    fn exact_mismatch() {
        assert!(!matches("a.b.c", "a.b.d"));
    }

    #[test]
    fn star_matches_single_token() {
        assert!(matches("a.*.c", "a.b.c"));
    }

    #[test]
    fn star_does_not_match_multiple_tokens() {
        assert!(!matches("a.*", "a.b.c"));
    }

    #[test]
    fn gt_matches_one_token() {
        assert!(matches("a.>", "a.b"));
    }

    #[test]
    fn gt_matches_multiple_tokens() {
        assert!(matches("a.>", "a.b.c.d"));
    }

    #[test]
    fn gt_requires_at_least_one_token() {
        assert!(!matches("a.>", "a"));
    }

    #[test]
    fn gt_at_root_matches_any() {
        assert!(matches(">", "a"));
        assert!(matches(">", "a.b.c"));
    }

    #[test]
    fn transcripts_wildcard() {
        assert!(matches(
            "transcripts.>",
            "transcripts.pr.owner.repo.456.sess-123"
        ));
        assert!(!matches("transcripts.>", "github.push"));
    }

    #[test]
    fn longer_subject_does_not_match_without_gt() {
        assert!(!matches("a.b", "a.b.c"));
    }

    #[test]
    fn shorter_subject_does_not_match() {
        assert!(!matches("a.b.c", "a.b"));
    }

    #[test]
    fn star_in_middle() {
        assert!(matches("a.*.*.d", "a.b.c.d"));
        assert!(!matches("a.*.*.d", "a.b.d"));
    }
}
