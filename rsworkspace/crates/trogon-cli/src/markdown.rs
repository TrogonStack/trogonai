/// Render a markdown string to ANSI-escaped terminal output.
///
/// Handles: headers (# / ## / ###), bold (**), italic (*), inline code (`),
/// fenced code blocks (```), and horizontal rules (---).
pub fn render(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 256);
    let mut in_code_block = false;

    let lines: Vec<&str> = text.split('\n').collect();
    let last = lines.len().saturating_sub(1);

    for (i, line) in lines.iter().enumerate() {
        if line.starts_with("```") {
            in_code_block = !in_code_block;
            // skip the fence line itself (no newline either — avoids blank lines at fence positions)
            continue;
        }

        if i > 0 {
            out.push('\n');
        }

        if in_code_block {
            out.push_str(DIM);
            out.push_str(line);
            out.push_str(RESET);
            continue;
        }

        if let Some(rest) = line.strip_prefix("### ") {
            out.push_str(BOLD);
            out.push_str(&render_inline(rest));
            out.push_str(RESET);
        } else if let Some(rest) = line.strip_prefix("## ") {
            out.push_str(BOLD_UNDERLINE);
            out.push_str(&render_inline(rest));
            out.push_str(RESET);
        } else if let Some(rest) = line.strip_prefix("# ") {
            out.push_str(BOLD_CYAN);
            out.push_str(&render_inline(rest));
            out.push_str(RESET);
        } else if *line == "---" || *line == "***" || *line == "___" {
            // horizontal rule
            out.push_str(DIM);
            out.push_str("────────────────────────────────────────");
            out.push_str(RESET);
        } else {
            out.push_str(&render_inline(line));
        }

        let _ = last; // suppress unused warning when iterating
    }

    out
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

fn render_inline(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 64);
    let mut rest = text;

    while !rest.is_empty() {
        // Inline code: `...`  (but not ```)
        if rest.starts_with('`') && !rest.starts_with("```")
            && let Some(end) = rest[1..].find('`') {
                out.push_str(YELLOW);
                out.push_str(&rest[1..1 + end]);
                out.push_str(RESET);
                rest = &rest[1 + end + 1..];
                continue;
            }
        // Bold: **...**
        if rest.starts_with("**")
            && let Some(end) = rest[2..].find("**") {
                out.push_str(BOLD);
                out.push_str(&render_inline(&rest[2..2 + end]));
                out.push_str(RESET);
                rest = &rest[2 + end + 2..];
                continue;
            }
        // Italic: *...* (not preceded by another *)
        if rest.starts_with('*') && !rest.starts_with("**")
            && let Some(end) = rest[1..].find('*')
            && end > 0 && !rest[1..1 + end].starts_with('*') {
                out.push_str(ITALIC);
                out.push_str(&rest[1..1 + end]);
                out.push_str(RESET);
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

    #[test]
    fn bold_renders_with_ansi() {
        let out = render("hello **world** there");
        assert!(out.contains(BOLD), "missing bold escape");
        assert!(out.contains(RESET));
        assert_eq!(strip_ansi(&out), "hello world there");
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
