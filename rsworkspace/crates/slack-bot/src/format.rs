/// Maximum characters per message chunk. OpenClaw default is 4 000; Slack's
/// hard limit on `text` fields is higher, but 4 000 leaves room for mrkdwn
/// expansion.
pub const SLACK_TEXT_LIMIT: usize = 4000;

/// Escape HTML entities (`&`, `<`, `>`) for Slack's mrkdwn text fields.
fn escape_entities_to(output: &mut String, text: &str) {
    for c in text.chars() {
        match c {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            _ => output.push(c),
        }
    }
}

/// Convert a subset of Markdown to Slack mrkdwn format.
///
/// Handles:
/// - `**bold**` → `*bold*`
/// - `*italic*` → `_italic_`
/// - `~~strikethrough~~` → `~strikethrough~`
/// - `` `code` `` → `` `code` `` (unchanged, no entity escaping)
/// - ```` ```block``` ```` → ```` ```block``` ```` (unchanged, no entity escaping)
/// - `[text](url)` → `<url|text>` (text is escaped, url is not)
/// - `# Heading` / `## Heading` etc. → `*Heading*`
/// - `- item` / `* item` preserved
/// - `&`, `<`, `>` in plain text are escaped to `&amp;`, `&lt;`, `&gt;`
pub fn markdown_to_mrkdwn(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        // Fenced code block: ```...```
        if i + 2 < len && chars[i] == '`' && chars[i + 1] == '`' && chars[i + 2] == '`' {
            // Find closing ```
            let start = i;
            i += 3;
            while i + 2 < len {
                if chars[i] == '`' && chars[i + 1] == '`' && chars[i + 2] == '`' {
                    i += 3;
                    break;
                }
                i += 1;
            }
            // Code blocks are shown verbatim — do NOT escape entities.
            output.extend(&chars[start..i]);
            continue;
        }

        // Inline code: `...`
        if chars[i] == '`' {
            let start = i;
            i += 1;
            while i < len && chars[i] != '`' {
                i += 1;
            }
            if i < len {
                i += 1; // consume closing `
            }
            // Inline code is shown verbatim — do NOT escape entities.
            output.extend(&chars[start..i]);
            continue;
        }

        // Strikethrough: ~~text~~
        if i + 1 < len
            && chars[i] == '~'
            && chars[i + 1] == '~'
            && let Some(end) = find_closing(&chars, i + 2, "~~")
        {
            output.push('~');
            escape_entities_to(&mut output, &chars[i + 2..end].iter().collect::<String>());
            output.push('~');
            i = end + 2;
            continue;
        }

        // Bold: **text**
        if i + 1 < len
            && chars[i] == '*'
            && chars[i + 1] == '*'
            && let Some(end) = find_closing(&chars, i + 2, "**")
        {
            output.push('*');
            escape_entities_to(&mut output, &chars[i + 2..end].iter().collect::<String>());
            output.push('*');
            i = end + 2;
            continue;
        }

        // Italic: *text* (but not **)
        if chars[i] == '*'
            && (i + 1 >= len || chars[i + 1] != '*')
            && let Some(end) = find_closing_char(&chars, i + 1, '*')
        {
            output.push('_');
            escape_entities_to(&mut output, &chars[i + 1..end].iter().collect::<String>());
            output.push('_');
            i = end + 1;
            continue;
        }

        // Markdown link: [text](url)
        if chars[i] == '['
            && let Some(bracket_end) = find_closing_char(&chars, i + 1, ']')
            && bracket_end + 1 < len
            && chars[bracket_end + 1] == '('
            && let Some(paren_end) = find_closing_char(&chars, bracket_end + 2, ')')
        {
            let link_text: String = chars[i + 1..bracket_end].iter().collect();
            let url: String = chars[bracket_end + 2..paren_end].iter().collect();
            output.push('<');
            output.push_str(&url); // URL unchanged — do NOT escape entities
            output.push('|');
            escape_entities_to(&mut output, &link_text); // link text is escaped
            output.push('>');
            i = paren_end + 1;
            continue;
        }

        // Blockquote: > at the start of a line → preserve as Slack mrkdwn blockquote.
        if (i == 0 || chars[i - 1] == '\n') && chars[i] == '>' {
            output.push('>');
            i += 1;
            continue;
        }

        // Headings at start of line: # / ## / ### etc.
        if (i == 0 || chars[i - 1] == '\n') && chars[i] == '#' {
            // Count leading #
            let mut j = i;
            while j < len && chars[j] == '#' {
                j += 1;
            }
            // Skip space after #
            if j < len && chars[j] == ' ' {
                j += 1;
            }
            // Find end of line
            let line_start = j;
            let mut line_end = j;
            while line_end < len && chars[line_end] != '\n' {
                line_end += 1;
            }
            let heading: String = chars[line_start..line_end].iter().collect();
            output.push('*');
            escape_entities_to(&mut output, &heading);
            output.push('*');
            i = line_end;
            continue;
        }

        // Fallthrough: escape entities in plain text characters.
        match chars[i] {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            _ => output.push(chars[i]),
        }
        i += 1;
    }

    output
}

fn find_closing(chars: &[char], start: usize, pattern: &str) -> Option<usize> {
    let pat: Vec<char> = pattern.chars().collect();
    let plen = pat.len();
    let mut i = start;
    while i + plen <= chars.len() {
        if &chars[i..i + plen] == pat.as_slice() {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn find_closing_char(chars: &[char], start: usize, target: char) -> Option<usize> {
    chars[start..]
        .iter()
        .position(|&c| c == target)
        .map(|p| p + start)
}

/// Split `text` into chunks of at most `limit` characters, preferring to break
/// on paragraph boundaries (`\n\n`) before falling back to single newlines.
pub fn chunk_text(text: &str, limit: usize) -> Vec<String> {
    if limit == 0 {
        return vec![];
    }
    let mut chunks = Vec::new();
    let mut remaining = text;

    while remaining.len() > limit {
        let slice = &remaining[..limit];
        // Try to break on a paragraph boundary (\n\n) first.
        let break_pos = if let Some(p) = slice.rfind("\n\n") {
            p + 2
        } else if let Some(p) = slice.rfind('\n') {
            p + 1
        } else {
            limit
        };
        chunks.push(remaining[..break_pos].to_string());
        remaining = &remaining[break_pos..];
    }

    if !remaining.is_empty() {
        chunks.push(remaining.to_string());
    }

    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bold_conversion() {
        assert_eq!(markdown_to_mrkdwn("**hello**"), "*hello*");
    }

    #[test]
    fn italic_conversion() {
        assert_eq!(markdown_to_mrkdwn("*hello*"), "_hello_");
    }

    #[test]
    fn link_conversion() {
        assert_eq!(
            markdown_to_mrkdwn("[text](https://example.com)"),
            "<https://example.com|text>"
        );
    }

    #[test]
    fn heading_conversion() {
        assert_eq!(markdown_to_mrkdwn("# Hello"), "*Hello*");
        assert_eq!(markdown_to_mrkdwn("## World"), "*World*");
    }

    #[test]
    fn code_preserved() {
        assert_eq!(markdown_to_mrkdwn("`code`"), "`code`");
        assert_eq!(markdown_to_mrkdwn("```block```"), "```block```");
    }

    #[test]
    fn chunk_text_splits_correctly() {
        let text = "line1\nline2\nline3";
        let chunks = chunk_text(text, 12);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0], "line1\nline2\n");
        assert_eq!(chunks[1], "line3");
    }

    #[test]
    fn chunk_text_short_text() {
        let chunks = chunk_text("hello", 100);
        assert_eq!(chunks, vec!["hello"]);
    }

    #[test]
    fn chunk_text_empty() {
        let chunks = chunk_text("", 100);
        assert!(chunks.is_empty());
    }

    #[test]
    fn bold_and_italic_in_same_string() {
        assert_eq!(
            markdown_to_mrkdwn("**bold** and *italic*"),
            "*bold* and _italic_"
        );
    }

    #[test]
    fn heading_only_at_start_of_line() {
        assert_eq!(
            markdown_to_mrkdwn("text\n## Heading\nmore"),
            "text\n*Heading*\nmore"
        );
    }

    #[test]
    fn heading_h3_converts() {
        assert_eq!(markdown_to_mrkdwn("### Title"), "*Title*");
    }

    #[test]
    fn unmatched_bold_passes_through() {
        assert_eq!(markdown_to_mrkdwn("**no close"), "**no close");
    }

    #[test]
    fn unmatched_italic_passes_through() {
        assert_eq!(markdown_to_mrkdwn("*no close"), "*no close");
    }

    #[test]
    fn code_block_not_converted() {
        assert_eq!(
            markdown_to_mrkdwn("```**bold**```"),
            "```**bold**```"
        );
    }

    #[test]
    fn link_with_bold_text() {
        assert_eq!(
            markdown_to_mrkdwn("[**click**](https://example.com)"),
            "<https://example.com|**click**>"
        );
    }

    #[test]
    fn multiple_links() {
        assert_eq!(
            markdown_to_mrkdwn("[a](https://a.com) and [b](https://b.com)"),
            "<https://a.com|a> and <https://b.com|b>"
        );
    }

    #[test]
    fn empty_string_unchanged() {
        assert_eq!(markdown_to_mrkdwn(""), "");
    }

    #[test]
    fn plain_text_unchanged() {
        assert_eq!(markdown_to_mrkdwn("hello world"), "hello world");
    }

    #[test]
    fn chunk_text_zero_limit_returns_empty() {
        let chunks = chunk_text("hello", 0);
        assert_eq!(chunks, Vec::<String>::new());
    }

    #[test]
    fn chunk_text_no_newline_hard_split() {
        let chunks = chunk_text("abcdefgh", 4);
        assert_eq!(chunks, vec!["abcd", "efgh"]);
    }

    #[test]
    fn chunk_text_prefers_newline_break() {
        // slice is "abc\nde" (6 chars), rfind('\n') = 3, break_pos = 4
        // first chunk = "abc\n", remaining = "defgh"
        let chunks = chunk_text("abc\ndefgh", 6);
        assert_eq!(chunks, vec!["abc\n", "defgh"]);
    }

    #[test]
    fn chunk_text_exact_limit_no_split() {
        let chunks = chunk_text("hello", 5);
        assert_eq!(chunks, vec!["hello"]);
    }

    // --- New tests for Change 1: HTML entity escaping ---

    #[test]
    fn entity_escaping_ampersand() {
        assert_eq!(markdown_to_mrkdwn("a & b"), "a &amp; b");
    }

    #[test]
    fn entity_escaping_less_than() {
        assert_eq!(markdown_to_mrkdwn("a < b"), "a &lt; b");
    }

    #[test]
    fn entity_escaping_greater_than() {
        assert_eq!(markdown_to_mrkdwn("a > b"), "a &gt; b");
    }

    #[test]
    fn entity_escaping_in_bold() {
        assert_eq!(markdown_to_mrkdwn("**a & b**"), "*a &amp; b*");
    }

    #[test]
    fn entity_escaping_in_heading() {
        assert_eq!(markdown_to_mrkdwn("# a > b"), "*a &gt; b*");
    }

    #[test]
    fn code_block_not_escaped() {
        // Raw code should not have entities escaped
        assert_eq!(markdown_to_mrkdwn("```a < b```"), "```a < b```");
    }

    #[test]
    fn inline_code_not_escaped() {
        assert_eq!(markdown_to_mrkdwn("`a < b`"), "`a < b`");
    }

    // --- New tests for Change 2: Strikethrough support ---

    #[test]
    fn strikethrough_conversion() {
        assert_eq!(markdown_to_mrkdwn("~~hello~~"), "~hello~");
    }

    #[test]
    fn strikethrough_with_escaping() {
        assert_eq!(markdown_to_mrkdwn("~~a & b~~"), "~a &amp; b~");
    }

    #[test]
    fn unmatched_strikethrough_passes_through() {
        // ~~ with no closing stays as-is (falls through char by char)
        // This behavior: first char is '~' but not ~~, so it passes through.
        // "~~no close" — starts with ~~, no closing ~~, falls through.
        // The fallthrough will output "~~no close" unchanged (each char separately).
        assert_eq!(markdown_to_mrkdwn("~~no close"), "~~no close");
    }

    // --- New tests for Change 3: Paragraph-aware chunk splitting ---

    #[test]
    fn chunk_text_prefers_paragraph_break() {
        // "para1\n\npara2abc" with limit 14: slice is "para1\n\npara2ab" (14 chars)
        // rfind("\n\n") at pos 5, break_pos = 7
        // chunk[0] = "para1\n\n", remaining = "para2abc"
        let chunks = chunk_text("para1\n\npara2abc", 14);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0], "para1\n\n");
        assert_eq!(chunks[1], "para2abc");
    }

    #[test]
    fn link_url_not_escaped() {
        // URL part of link must NOT have & escaped (it's a URL)
        let result = markdown_to_mrkdwn("[click](https://a.com/q?a=1&b=2)");
        assert_eq!(result, "<https://a.com/q?a=1&b=2|click>");
    }
    #[test]
    fn blockquote_at_line_start_preserved() {
        assert_eq!(markdown_to_mrkdwn("> hello world"), "> hello world");
    }

    #[test]
    fn blockquote_mid_line_still_escaped() {
        // > not at line start must still be escaped
        assert_eq!(markdown_to_mrkdwn("text > more"), "text &gt; more");
    }

    #[test]
    fn blockquote_content_entities_escaped() {
        // Content inside a blockquote gets entity-escaped
        assert_eq!(markdown_to_mrkdwn("> a & b"), "> a &amp; b");
    }

    #[test]
    fn blockquote_after_newline() {
        assert_eq!(markdown_to_mrkdwn("line\n> quote"), "line\n> quote");
    }

    #[test]
    fn blockquote_with_bold_inside() {
        assert_eq!(markdown_to_mrkdwn("> **bold**"), "> *bold*");
    }

}
