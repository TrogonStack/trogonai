//! Terminal cursor recovery after permission prompts and inline tool status lines.

use std::io::Write;

/// Reset stdout/stderr to column 0 on a fresh line so streamed text is not jagged.
///
/// Terminal cursor position is shared between stdout and stderr; tool status lines
/// on stderr can leave stdout printing mid-line unless both streams are reset.
pub fn reset_display() {
    let _ = std::io::stderr().write_all(b"\r\x1b[2K\n");
    let _ = std::io::stdout().write_all(b"\r\x1b[2K\n");
    let _ = std::io::stderr().flush();
    let _ = std::io::stdout().flush();
}

/// Strip carriage returns and trailing whitespace so multi-line tool output aligns cleanly.
pub fn sanitize_tool_output(output: &str) -> String {
    if output.is_empty() {
        return String::new();
    }
    output
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .lines()
        .map(|line| line.trim_end())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Print tool output on stderr after resetting the cursor so lines start at column 0.
pub fn print_tool_output(output: &str) {
    let clean = sanitize_tool_output(output);
    if clean.is_empty() {
        return;
    }
    reset_display();
    for line in clean.lines() {
        let _ = writeln!(std::io::stderr(), "\x1b[2K\r{line}");
    }
    let _ = std::io::stderr().flush();
    reset_display();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_tool_output_strips_carriage_returns() {
        assert_eq!(sanitize_tool_output("a\r\nb\rc"), "a\nb\nc");
    }

    #[test]
    fn sanitize_tool_output_trims_trailing_spaces() {
        assert_eq!(sanitize_tool_output("  hello   \n  world  "), "  hello\n  world");
    }

    #[test]
    fn sanitize_tool_output_preserves_intentional_indent() {
        assert_eq!(sanitize_tool_output("  foo\n    bar"), "  foo\n    bar");
    }
}
