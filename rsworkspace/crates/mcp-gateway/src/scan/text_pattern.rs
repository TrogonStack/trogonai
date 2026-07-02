/// Hand-written, dependency-free equivalents of the `re.compile(...)`
/// patterns AGT's `mcp_security.py` and `prompt_injection.py` use to scan
/// tool descriptions. There is no `regex` crate in this workspace and this
/// crate is not allowed to add one, so each AGT pattern is reimplemented as
/// a small matcher over `\s+`-style flexible whitespace and case-insensitive
/// literal/word sequences, which is all the upstream patterns actually need.
///
/// A "phrase" pattern is a sequence of literal words that must appear in
/// order, separated by one or more whitespace characters (mirroring `\s+`
/// in the source regex), matched case-insensitively (mirroring
/// `re.IGNORECASE`). Optional connector words (matching a `(?:foo\s+)?`
/// group in the source) are represented by [`PhraseToken::Optional`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhraseToken<'a> {
    /// A single literal word that must appear.
    Word(&'a str),
    /// One of several literal words, one of which must appear.
    AnyOf(&'a [&'a str]),
    /// A literal word that may or may not appear (the "previous" in
    /// `override\s+(the\s+)?previous`, for example).
    Optional(&'a str),
}

/// A single logical detection rule: an ordered sequence of [`PhraseToken`]s,
/// plus the exact upstream regex source string (kept for the
/// `matched_pattern` field on threats, so audit output stays legible).
#[derive(Debug, Clone, Copy)]
pub struct PhrasePattern {
    pub source: &'static str,
    pub tokens: &'static [PhraseToken<'static>],
}

/// Case-insensitive substring search: true if `haystack` contains `needle`
/// anywhere, ignoring ASCII case. Used for single-word / fixed-string
/// patterns where a phrase match is unnecessary.
pub fn contains_ci(haystack: &str, needle: &str) -> bool {
    let haystack_lower = haystack.to_lowercase();
    let needle_lower = needle.to_lowercase();
    haystack_lower.contains(&needle_lower)
}

/// Does `text` match `pattern`: do the tokens appear in order, each
/// separated by whitespace, case-insensitively?
///
/// This is intentionally a simple ordered scan rather than a general regex
/// engine: AGT's patterns are all "word (whitespace word)*" shapes with at
/// most one optional connector, which this covers exactly.
pub fn matches_phrase(text: &str, pattern: &PhrasePattern) -> bool {
    let lower = text.to_lowercase();
    let words: Vec<&str> = lower.split_whitespace().collect();

    for start in 0..words.len() {
        if try_match_at(&words[start..], pattern.tokens) {
            return true;
        }
    }
    false
}

fn try_match_at(words: &[&str], tokens: &[PhraseToken<'_>]) -> bool {
    let mut word_idx = 0;
    for token in tokens {
        match token {
            PhraseToken::Word(expected) => {
                let Some(word) = words.get(word_idx) else {
                    return false;
                };
                if !word_matches(word, expected) {
                    return false;
                }
                word_idx += 1;
            }
            PhraseToken::AnyOf(options) => {
                let Some(word) = words.get(word_idx) else {
                    return false;
                };
                if !options.iter().any(|opt| word_matches(word, opt)) {
                    return false;
                }
                word_idx += 1;
            }
            PhraseToken::Optional(expected) => {
                if let Some(word) = words.get(word_idx)
                    && word_matches(word, expected)
                {
                    word_idx += 1;
                }
            }
        }
    }
    true
}

/// A "word" in `expected` may itself be a multi-word literal joined by a
/// single space (e.g. "root access"); split and compare against the
/// upcoming words positionally is handled by the caller via `AnyOf`/`Word`
/// being single tokens, so here we only need an exact case-normalized
/// comparison of one text word against one pattern word, after stripping
/// trailing punctuation that would otherwise block a match at sentence
/// boundaries (e.g. "instructions." vs "instructions").
fn word_matches(text_word: &str, pattern_word: &str) -> bool {
    let trimmed = text_word.trim_matches(|c: char| !c.is_alphanumeric());
    trimmed.eq_ignore_ascii_case(pattern_word)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contains_ci_matches_regardless_of_case() {
        assert!(contains_ci("Hello WORLD", "world"));
        assert!(!contains_ci("Hello WORLD", "planet"));
    }

    #[test]
    fn phrase_matches_simple_sequence() {
        const PATTERN: PhrasePattern = PhrasePattern {
            source: "actually do",
            tokens: &[PhraseToken::Word("actually"), PhraseToken::Word("do")],
        };
        assert!(matches_phrase("please actually do this", &PATTERN));
        assert!(!matches_phrase("please do this", &PATTERN));
    }

    #[test]
    fn phrase_matches_with_optional_connector() {
        const PATTERN: PhrasePattern = PhrasePattern {
            source: "override (the )?previous",
            tokens: &[
                PhraseToken::Word("override"),
                PhraseToken::Optional("the"),
                PhraseToken::Word("previous"),
            ],
        };
        assert!(matches_phrase("override the previous instructions", &PATTERN));
        assert!(matches_phrase("override previous instructions", &PATTERN));
        assert!(!matches_phrase("override nothing", &PATTERN));
    }

    #[test]
    fn phrase_matches_any_of() {
        const PATTERN: PhrasePattern = PhrasePattern {
            source: "(above|prior|previous)",
            tokens: &[PhraseToken::AnyOf(&["above", "prior", "previous"])],
        };
        assert!(matches_phrase("disregard prior context", &PATTERN));
        assert!(matches_phrase("disregard above context", &PATTERN));
        assert!(!matches_phrase("disregard future context", &PATTERN));
    }

    #[test]
    fn phrase_matching_is_case_insensitive() {
        const PATTERN: PhrasePattern = PhrasePattern {
            source: "you are",
            tokens: &[PhraseToken::Word("you"), PhraseToken::Word("are")],
        };
        assert!(matches_phrase("YOU ARE a helpful assistant", &PATTERN));
    }

    #[test]
    fn word_matches_strips_trailing_punctuation() {
        const PATTERN: PhrasePattern = PhrasePattern {
            source: "must be called",
            tokens: &[
                PhraseToken::Word("must"),
                PhraseToken::Word("be"),
                PhraseToken::Word("called"),
            ],
        };
        assert!(matches_phrase("this tool must be called.", &PATTERN));
    }
}
