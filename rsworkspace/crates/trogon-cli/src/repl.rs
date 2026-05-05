use crate::session::{StreamEvent, TrogonSession};
use async_nats::Client;
use rustyline::completion::{Completer, Pair};
use rustyline::error::ReadlineError;
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::validate::Validator;
use rustyline::{Context, Editor, Helper};
use std::io::Write;
use std::path::{Path, PathBuf};

const HISTORY_PATH: &str = "~/.local/share/trogon/history";

// ── FileAtHelper ──────────────────────────────────────────────────────────────

/// rustyline helper that tab-completes `@<path>` tokens.
struct FileAtHelper {
    cwd: PathBuf,
}

impl Default for FileAtHelper {
    fn default() -> Self {
        Self {
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        }
    }
}

impl Helper for FileAtHelper {}

impl Completer for FileAtHelper {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Pair>)> {
        let before = &line[..pos];
        let Some(at_pos) = before.rfind('@') else {
            return Ok((pos, vec![]));
        };
        let partial = &before[at_pos + 1..];
        if partial.contains(' ') {
            return Ok((pos, vec![]));
        }
        let (dir, prefix) = if let Some(slash) = partial.rfind('/') {
            (self.cwd.join(&partial[..=slash]), &partial[slash + 1..])
        } else {
            (self.cwd.clone(), partial)
        };
        let mut pairs: Vec<Pair> = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with(prefix) {
                    let suffix = if entry.path().is_dir() { "/" } else { "" };
                    let replacement = format!("{name}{suffix}");
                    pairs.push(Pair { display: replacement.clone(), replacement });
                }
            }
        }
        pairs.sort_by(|a, b| a.display.cmp(&b.display));
        Ok((at_pos + 1, pairs))
    }
}

impl Hinter for FileAtHelper {
    type Hint = String;
    fn hint(&self, _line: &str, _pos: usize, _ctx: &Context<'_>) -> Option<String> {
        None
    }
}

impl Highlighter for FileAtHelper {}

impl Validator for FileAtHelper {}

// ── @mention expansion ────────────────────────────────────────────────────────

/// Scans `text` for `@<path>` tokens and replaces each with a fenced code block
/// containing the file's contents, resolved relative to `cwd`.
/// Unresolvable paths are left as-is with a warning on stderr.
fn expand_mentions(text: &str, cwd: &Path) -> String {
    let mut result = String::with_capacity(text.len());
    let mut chars = text.char_indices().peekable();
    while let Some((i, ch)) = chars.next() {
        if ch == '@' {
            let start = i + 1;
            let mut end = start;
            while let Some(&(j, c)) = chars.peek() {
                if c.is_whitespace() {
                    break;
                }
                end = j + c.len_utf8();
                chars.next();
            }
            let path_str = &text[start..end];
            if path_str.is_empty() {
                result.push('@');
            } else {
                match std::fs::read_to_string(cwd.join(path_str)) {
                    Ok(content) => {
                        result.push_str(&format!("`{path_str}`:\n```\n{content}\n```"));
                    }
                    Err(_) => {
                        eprintln!("warning: @{path_str}: file not found or not readable");
                        result.push('@');
                        result.push_str(path_str);
                    }
                }
            }
        } else {
            result.push(ch);
        }
    }
    result
}

// ── REPL entry point ──────────────────────────────────────────────────────────

pub async fn run(nats: Client, prefix: &str, cwd: PathBuf) -> anyhow::Result<()> {
    let session = TrogonSession::new(nats, prefix, cwd.clone()).await?;

    let history_path = expand_tilde(HISTORY_PATH);
    if let Some(dir) = history_path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }

    let mut rl: Editor<FileAtHelper, _> = Editor::new()?;
    rl.set_helper(Some(FileAtHelper { cwd: cwd.clone() }));
    let _ = rl.load_history(&history_path);

    eprintln!("trogon — session {} (Ctrl+D to quit)", session.session_id);

    loop {
        match rl.readline("> ") {
            Ok(line) => {
                let line = line.trim().to_string();
                if line.is_empty() {
                    continue;
                }

                if let Some(output) = handle_slash_command(&line) {
                    println!("{output}");
                    continue;
                }

                let _ = rl.add_history_entry(&line);

                let expanded = expand_mentions(&line, &cwd);
                match session.prompt(&expanded).await {
                    Err(e) => eprintln!("error: {e}"),
                    Ok(mut rx) => {
                        let mut stdout = std::io::stdout();
                        loop {
                            match rx.recv().await {
                                None => break,
                                Some(StreamEvent::Text(text)) => {
                                    print!("{text}");
                                    let _ = stdout.flush();
                                }
                                Some(StreamEvent::Thinking) => {}
                                Some(StreamEvent::ToolCall(name)) => {
                                    eprintln!("\n[tool: {name}]");
                                }
                                Some(StreamEvent::Done(reason)) => {
                                    if reason == "cancelled" {
                                        eprintln!("\n[cancelled]");
                                    } else {
                                        println!();
                                    }
                                    break;
                                }
                            }
                        }
                    }
                }
            }
            Err(ReadlineError::Interrupted) => {
                session.cancel().await;
                eprintln!("(Ctrl+C)");
            }
            Err(ReadlineError::Eof) => {
                eprintln!("bye");
                break;
            }
            Err(e) => {
                eprintln!("readline error: {e}");
                break;
            }
        }
    }

    let _ = rl.save_history(&history_path);
    Ok(())
}

fn handle_slash_command(line: &str) -> Option<String> {
    if !line.starts_with('/') {
        return None;
    }
    let parts: Vec<&str> = line.splitn(2, ' ').collect();
    match parts[0] {
        "/help" => Some(
            "Commands: /help /clear /cost /model <id>\n\
             Ctrl+D  quit"
                .to_string(),
        ),
        "/clear" => Some("[/clear not yet implemented — PR 9]".to_string()),
        "/cost" => Some("[/cost not yet implemented — PR 9]".to_string()),
        _ => Some(format!("unknown command: {}", parts[0])),
    }
}

fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = std::env::var("HOME").ok().map(PathBuf::from) {
            return home.join(rest);
        }
    }
    PathBuf::from(path)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_mentions_no_at_sign_is_unchanged() {
        let result = expand_mentions("hello world", Path::new("/tmp"));
        assert_eq!(result, "hello world");
    }

    #[test]
    fn expand_mentions_lone_at_is_preserved() {
        let result = expand_mentions("price @ discount", Path::new("/tmp"));
        assert_eq!(result, "price @ discount");
    }

    #[test]
    fn expand_mentions_missing_file_leaves_token_and_warns() {
        let result = expand_mentions("see @nonexistent.txt please", Path::new("/tmp"));
        assert!(result.contains("@nonexistent.txt"));
        assert!(!result.contains("```"));
    }

    #[test]
    fn expand_mentions_existing_file_inserts_code_block() {
        let dir = std::env::temp_dir();
        let file_path = dir.join("trogon_test_mention.txt");
        {
            let mut f = std::fs::File::create(&file_path).unwrap();
            f.write_all(b"hello from file").unwrap();
        }
        let filename = file_path.file_name().unwrap().to_str().unwrap();
        let input = format!("look at @{filename} now");
        let result = expand_mentions(&input, &dir);
        assert!(result.contains("hello from file"), "got: {result}");
        assert!(result.contains("```"), "expected fenced block, got: {result}");
        let _ = std::fs::remove_file(&file_path);
    }

    #[test]
    fn expand_mentions_multiple_mentions() {
        let dir = std::env::temp_dir();
        let f1 = dir.join("trogon_m1.txt");
        let f2 = dir.join("trogon_m2.txt");
        std::fs::write(&f1, "file1").unwrap();
        std::fs::write(&f2, "file2").unwrap();
        let input = "a @trogon_m1.txt b @trogon_m2.txt c";
        let result = expand_mentions(input, &dir);
        assert!(result.contains("file1"), "got: {result}");
        assert!(result.contains("file2"), "got: {result}");
        let _ = std::fs::remove_file(&f1);
        let _ = std::fs::remove_file(&f2);
    }

    #[test]
    fn file_at_helper_complete_no_at_returns_empty() {
        let helper = FileAtHelper { cwd: std::env::temp_dir() };
        let history = rustyline::history::DefaultHistory::new();
        let ctx = Context::new(&history);
        let (_, pairs) = helper.complete("hello world", 5, &ctx).unwrap();
        assert!(pairs.is_empty());
    }

    #[test]
    fn file_at_helper_complete_at_with_space_returns_empty() {
        let helper = FileAtHelper { cwd: std::env::temp_dir() };
        let history = rustyline::history::DefaultHistory::new();
        let ctx = Context::new(&history);
        let (_, pairs) = helper.complete("@ hello", 7, &ctx).unwrap();
        assert!(pairs.is_empty());
    }
}
