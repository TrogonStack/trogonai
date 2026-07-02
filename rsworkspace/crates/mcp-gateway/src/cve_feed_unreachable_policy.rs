/// Whether an unreachable or unparseable CVE feed response should block
/// dispatch. Fail-closed (`Deny`) is the only safe default; fail-open is a
/// deliberate, explicit opt-in a caller must construct on purpose, never
/// something the gate falls into implicitly.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CveFeedUnreachablePolicy {
    #[default]
    Deny,
    Allow,
}

impl CveFeedUnreachablePolicy {
    pub fn is_fail_closed(self) -> bool {
        matches!(self, Self::Deny)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_fail_closed() {
        assert_eq!(CveFeedUnreachablePolicy::default(), CveFeedUnreachablePolicy::Deny);
        assert!(CveFeedUnreachablePolicy::default().is_fail_closed());
    }

    #[test]
    fn allow_is_explicit_and_not_fail_closed() {
        assert!(!CveFeedUnreachablePolicy::Allow.is_fail_closed());
    }
}
