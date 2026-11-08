use crate::compile_error::CompileError;
use crate::constants::MAX_EXPRESSION_DEPTH;

/// Refuses source whose nesting could overflow the stack of the parser.
///
/// The parser recurses once per open bracket, per chained prefix operator and
/// per level of conditional, and it has no depth limit. The count is made over
/// raw characters and ignores string literals on purpose: a scan that tried to
/// skip strings could be fooled by an escape it did not model, and a scan that
/// over-counts only turns away a literal holding a long run of brackets.
pub(crate) fn check_nesting(source: &str) -> Result<(), CompileError> {
    let too_deep = CompileError::TooDeep {
        limit: MAX_EXPRESSION_DEPTH,
    };
    let mut open = 0usize;
    let mut prefix_run = 0usize;
    let mut conditionals = 0usize;
    for character in source.chars() {
        match character {
            '(' | '[' | '{' => {
                open += 1;
                prefix_run = 0;
            }
            ')' | ']' | '}' => {
                open = open.saturating_sub(1);
                prefix_run = 0;
            }
            '!' | '-' => prefix_run += 1,
            '?' => {
                conditionals += 1;
                prefix_run = 0;
            }
            c if c.is_whitespace() => {}
            _ => prefix_run = 0,
        }
        if open > MAX_EXPRESSION_DEPTH || prefix_run > MAX_EXPRESSION_DEPTH || conditionals > MAX_EXPRESSION_DEPTH {
            return Err(too_deep);
        }
    }
    Ok(())
}
