//! The JavaScript string semantics the browser SSE port depends on.

/// Whether a regular expression `\s` matches `ch` in JavaScript: the ECMAScript white space and line terminators.
pub(crate) fn is_js_whitespace(ch: char) -> bool {
    matches!(
        ch,
        '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}'
    )
}

#[cfg(test)]
#[path = "js_text_tests.rs"]
mod tests;
