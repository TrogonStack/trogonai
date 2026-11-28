//! Shared vocabulary for project-context gathering.

/// The whitespace trimmed off a rule body and off every line of it.
///
/// This is four ASCII bytes, not Unicode whitespace.
pub const TRIM_CHARS: [char; 4] = [' ', '\t', '\r', '\n'];

/// How much of an omitted source is quoted back before it is hashed instead.
pub const OMISSION_SOURCE_PREFIX_BYTES: usize = 256;

/// How many omissions are recorded one by one before the rest become a summary.
pub const RETAINED_OMISSION_COUNT: usize = 32;

/// What the model is told about how project instructions rank against its own.
pub const PROJECT_INSTRUCTION_GUIDANCE: &str = concat!(
    "Direct user instructions take precedence over project instructions. ",
    "When project instructions conflict, follow the narrowest applicable project scope."
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_instruction_guidance_matches_the_contract_fixture() {
        assert!(PROJECT_INSTRUCTION_GUIDANCE.contains("Direct user instructions"));
        assert!(PROJECT_INSTRUCTION_GUIDANCE.contains("narrowest applicable project scope"));
    }

    #[test]
    fn trim_chars_are_the_four_ascii_whitespace_bytes() {
        assert_eq!(TRIM_CHARS, [' ', '\t', '\r', '\n']);
    }

    #[test]
    fn omission_and_retention_limits_match_context_contract() {
        assert_eq!(OMISSION_SOURCE_PREFIX_BYTES, 256);
        assert_eq!(RETAINED_OMISSION_COUNT, 32);
    }
}
