/// Render a markdown string to ANSI-escaped terminal output.
///
/// Handles: headers (# / ## / ###), bold (**), italic (*), inline code (`),
/// fenced code blocks (```), and horizontal rules (---).
pub fn render(text: &str) -> String {
    render_with_ansi(text, true)
}

/// Render markdown to plain text (no ANSI escapes). Markdown structure is
/// normalized the same way as [`render`], but without terminal styling.
pub fn render_plain(text: &str) -> String {
    render_with_ansi(text, false)
}

fn render_with_ansi(text: &str, use_ansi: bool) -> String {
    let mut out = String::with_capacity(text.len() + 256);
    let mut in_code_block = false;
    let mut first = true;

    for line in text.split('\n') {
        if let Some(rendered) = render_line(line, use_ansi, &mut in_code_block) {
            if !first {
                out.push('\n');
            }
            out.push_str(&rendered);
            first = false;
        }
    }

    out
}

/// Render a single markdown line, updating `in_code_block` (block state the
/// caller owns so it persists across calls). Returns `None` for a code-fence
/// line (which produces no output), else the styled line. Inline spans (bold,
/// italic, inline code) are resolved per line — this lets the streaming REPL
/// render markdown line-by-line as text arrives, not just in buffered mode.
pub fn render_line(line: &str, use_ansi: bool, in_code_block: &mut bool) -> Option<String> {
    if line.starts_with("```") {
        *in_code_block = !*in_code_block;
        // The fence line itself produces no output.
        return None;
    }

    let mut out = String::new();
    if *in_code_block {
        push_style(&mut out, Style::Dim, use_ansi);
        out.push_str(line);
        push_reset(&mut out, use_ansi);
        return Some(out);
    }

    if let Some(rest) = line.strip_prefix("### ") {
        push_style(&mut out, Style::Bold, use_ansi);
        out.push_str(&render_inline(rest, use_ansi));
        push_reset(&mut out, use_ansi);
    } else if let Some(rest) = line.strip_prefix("## ") {
        push_style(&mut out, Style::BoldUnderline, use_ansi);
        out.push_str(&render_inline(rest, use_ansi));
        push_reset(&mut out, use_ansi);
    } else if let Some(rest) = line.strip_prefix("# ") {
        push_style(&mut out, Style::BoldCyan, use_ansi);
        out.push_str(&render_inline(rest, use_ansi));
        push_reset(&mut out, use_ansi);
    } else if line == "---" || line == "***" || line == "___" {
        // horizontal rule
        push_style(&mut out, Style::Dim, use_ansi);
        out.push_str("────────────────────────────────────────");
        push_reset(&mut out, use_ansi);
    } else {
        out.push_str(&render_inline(line, use_ansi));
    }

    Some(out)
}

enum Style {
    Bold,
    Italic,
    Dim,
    Yellow,
    BoldCyan,
    BoldUnderline,
}

fn push_style(out: &mut String, style: Style, use_ansi: bool) {
    if use_ansi {
        out.push_str(match style {
            Style::Bold => BOLD,
            Style::Italic => ITALIC,
            Style::Dim => DIM,
            Style::Yellow => YELLOW,
            Style::BoldCyan => BOLD_CYAN,
            Style::BoldUnderline => BOLD_UNDERLINE,
        });
    }
}

fn push_reset(out: &mut String, use_ansi: bool) {
    if use_ansi {
        out.push_str(RESET);
    }
}

// ── ANSI codes ─────────────────────────────────────────────────────────────────

const BOLD: &str = "\x1b[1m";
const ITALIC: &str = "\x1b[3m";
const DIM: &str = "\x1b[2m";
const YELLOW: &str = "\x1b[33m";
const RESET: &str = "\x1b[0m";
const BOLD_CYAN: &str = "\x1b[1;36m";
const BOLD_UNDERLINE: &str = "\x1b[1;4m";

// ── Inline rendering ───────────────────────────────────────────────────────────

fn render_inline(text: &str, use_ansi: bool) -> String {
    let mut out = String::with_capacity(text.len() + 64);
    let mut rest = text;

    while !rest.is_empty() {
        // Inline code: `...`  (but not ```)
        if rest.starts_with('`')
            && !rest.starts_with("```")
            && let Some(end) = rest[1..].find('`')
        {
            push_style(&mut out, Style::Yellow, use_ansi);
            out.push_str(&rest[1..1 + end]);
            push_reset(&mut out, use_ansi);
            rest = &rest[1 + end + 1..];
            continue;
        }
        // Bold: **...**
        if rest.starts_with("**")
            && let Some(end) = rest[2..].find("**")
        {
            push_style(&mut out, Style::Bold, use_ansi);
            out.push_str(&render_inline(&rest[2..2 + end], use_ansi));
            push_reset(&mut out, use_ansi);
            rest = &rest[2 + end + 2..];
            continue;
        }
        // Italic: *...* (not preceded by another *)
        if rest.starts_with('*')
            && !rest.starts_with("**")
            && let Some(end) = rest[1..].find('*')
            && end > 0
            && !rest[1..1 + end].starts_with('*')
        {
            push_style(&mut out, Style::Italic, use_ansi);
            out.push_str(&rest[1..1 + end]);
            push_reset(&mut out, use_ansi);
            rest = &rest[1 + end + 1..];
            continue;
        }
        // Consume one UTF-8 character
        let ch_len = rest.chars().next().map(|c| c.len_utf8()).unwrap_or(1);
        out.push_str(&rest[..ch_len]);
        rest = &rest[ch_len..];
    }

    out
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn strip_ansi(s: &str) -> String {
        let mut out = String::new();
        let mut in_escape = false;
        for c in s.chars() {
            if c == '\x1b' {
                in_escape = true;
            } else if in_escape {
                if c == 'm' {
                    in_escape = false;
                }
            } else {
                out.push(c);
            }
        }
        out
    }

    fn has_ansi_escapes(s: &str) -> bool {
        s.contains('\x1b')
    }

    #[test]
    fn bold_renders_with_ansi() {
        let out = render("hello **world** there");
        assert!(out.contains(BOLD), "missing bold escape");
        assert!(out.contains(RESET));
        assert_eq!(strip_ansi(&out), "hello world there");
    }

    #[test]
    fn plain_render_has_no_ansi_escapes() {
        let out = render_plain("hello **world** there");
        assert!(!has_ansi_escapes(&out));
        assert_eq!(out, "hello world there");
    }

    #[test]
    fn plain_and_ansi_render_same_visible_text() {
        let text = "# Title\n\nsay **hi** and `code`";
        assert_eq!(strip_ansi(&render(text)), render_plain(text));
    }

    #[test]
    fn italic_renders_with_ansi() {
        let out = render("say *hi* now");
        assert!(out.contains(ITALIC));
        assert_eq!(strip_ansi(&out), "say hi now");
    }

    #[test]
    fn inline_code_renders_yellow() {
        let out = render("run `cargo test` please");
        assert!(out.contains(YELLOW));
        assert_eq!(strip_ansi(&out), "run cargo test please");
    }

    #[test]
    fn h1_renders_bold_cyan() {
        let out = render("# Title");
        assert!(out.contains(BOLD_CYAN));
        assert_eq!(strip_ansi(&out), "Title");
    }

    #[test]
    fn h2_renders_bold_underline() {
        let out = render("## Section");
        assert!(out.contains(BOLD_UNDERLINE));
        assert_eq!(strip_ansi(&out), "Section");
    }

    #[test]
    fn h3_renders_bold() {
        let out = render("### Sub");
        assert!(out.contains(BOLD));
        assert_eq!(strip_ansi(&out), "Sub");
    }

    #[test]
    fn code_block_renders_dim() {
        let out = render("```\nfn main() {}\n```");
        assert!(out.contains(DIM));
        assert!(strip_ansi(&out).contains("fn main() {}"));
    }

    #[test]
    fn plain_text_unchanged() {
        let out = render("hello world");
        assert_eq!(strip_ansi(&out), "hello world");
    }

    #[test]
    fn multiline_preserves_newlines() {
        let out = render("line one\nline two\nline three");
        assert_eq!(strip_ansi(&out), "line one\nline two\nline three");
    }

    #[test]
    fn horizontal_rule_renders() {
        let out = render("---");
        assert!(strip_ansi(&out).contains("──"));
    }

    #[test]
    fn unmatched_bold_passes_through() {
        let out = render("this ** has no close");
        assert_eq!(strip_ansi(&out), "this ** has no close");
    }

    #[test]
    fn nested_inline_code_not_parsed_inside() {
        let out = render("`no **bold** here`");
        assert_eq!(strip_ansi(&out), "no **bold** here");
    }
}
