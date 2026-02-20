/// Maximum characters per message chunk. OpenClaw default is 4 000; Slack's
/// hard limit on `text` fields is higher, but 4 000 leaves room for mrkdwn
/// expansion.
pub const SLACK_TEXT_LIMIT: usize = 4000;

/// Convert a subset of Markdown to Slack mrkdwn format.
///
/// Handles:
/// - `**bold**` → `*bold*`
/// - `*italic*` → `_italic_`
/// - `` `code` `` → `` `code` `` (unchanged)
/// - ```` ```block``` ```` → ```` ```block``` ```` (unchanged)
/// - `[text](url)` → `<url|text>`
/// - `# Heading` / `## Heading` etc. → `*Heading*`
/// - `- item` / `* item` preserved
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
            output.extend(&chars[start..i]);
            continue;
        }

        // Bold: **text**
        if i + 1 < len
            && chars[i] == '*'
            && chars[i + 1] == '*'
            && let Some(end) = find_closing(&chars, i + 2, "**")
        {
            output.push('*');
            output.extend(&chars[i + 2..end]);
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
            output.extend(&chars[i + 1..end]);
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
            output.push_str(&url);
            output.push('|');
            output.push_str(&link_text);
            output.push('>');
            i = paren_end + 1;
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
            output.push_str(&heading);
            output.push('*');
            i = line_end;
            continue;
        }

        output.push(chars[i]);
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
/// on newline boundaries.
pub fn chunk_text(text: &str, limit: usize) -> Vec<String> {
    if limit == 0 {
        return vec![];
    }
    let mut chunks = Vec::new();
    let mut remaining = text;

    while remaining.len() > limit {
        // Try to break on a newline within the limit.
        let slice = &remaining[..limit];
        let break_pos = slice.rfind('\n').map(|p| p + 1).unwrap_or(limit);
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
}
