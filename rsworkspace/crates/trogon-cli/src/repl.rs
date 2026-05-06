use crate::fs::Fs;
use crate::nats::NatsClient;
use crate::session::{Session, StreamEvent, TrogonSession};
use rustyline::completion::{Completer, Pair};
use rustyline::error::ReadlineError;
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::validate::{ValidationContext, ValidationResult, Validator};
use rustyline::{Context, Editor, Helper};
use std::io::Write;
use std::path::{Path, PathBuf};

const HISTORY_PATH: &str = "~/.local/share/trogon/history";

// ── FileAtHelper ──────────────────────────────────────────────────────────────

struct FileAtHelper {
    cwd: PathBuf,
}

impl Default for FileAtHelper {
    fn default() -> Self {
        Self { cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")) }
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

impl Validator for FileAtHelper {
    fn validate(&self, ctx: &mut ValidationContext<'_>) -> rustyline::Result<ValidationResult> {
        if input_needs_continuation(ctx.input()) {
            Ok(ValidationResult::Incomplete)
        } else {
            Ok(ValidationResult::Valid(None))
        }
    }
}

fn input_needs_continuation(input: &str) -> bool {
    input.ends_with('\\')
}

// ── @mention expansion ────────────────────────────────────────────────────────

fn expand_mentions<F: Fs>(text: &str, cwd: &Path, fs: &F) -> String {
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
                match fs.read_to_string(&cwd.join(path_str)) {
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

// ── Multiline join ────────────────────────────────────────────────────────────

fn join_continuation(s: &str) -> String {
    s.replace("\\\n", " ")
}

// ── REPL entry point ──────────────────────────────────────────────────────────

pub async fn run<N: NatsClient + Clone, F: Fs>(
    nats: N,
    prefix: &str,
    cwd: PathBuf,
    fs: F,
) -> anyhow::Result<()> {
    let prefix = prefix.to_string();
    let mut session = TrogonSession::new(nats.clone(), &prefix, cwd.clone()).await?;

    let history_path = expand_tilde(HISTORY_PATH);
    if let Some(dir) = history_path.parent() {
        let _ = fs.create_dir_all(dir);
    }

    let mut rl: Editor<FileAtHelper, _> = Editor::new()?;
    rl.set_helper(Some(FileAtHelper { cwd: cwd.clone() }));
    let _ = rl.load_history(&history_path);

    eprintln!("trogon — session {} (Ctrl+D to quit)", session.session_id());

    let mut session_used_tokens: u64 = 0;
    let mut session_context_size: u64 = 0;

    loop {
        match rl.readline("> ") {
            Ok(raw_line) => {
                let line = join_continuation(&raw_line).trim().to_string();
                if line.is_empty() {
                    continue;
                }

                if line.starts_with('/') {
                    let mut parts = line.splitn(2, ' ');
                    let cmd = parts.next().unwrap_or("");
                    let arg = parts.next().unwrap_or("");
                    if cmd == "/clear" {
                        match TrogonSession::new(nats.clone(), &prefix, cwd.clone()).await {
                            Ok(s) => {
                                session = s;
                                session_used_tokens = 0;
                                session_context_size = 0;
                                eprintln!("session cleared — new session {}", session.session_id());
                            }
                            Err(e) => eprintln!("error: {e}"),
                        }
                    } else {
                        println!(
                            "{}",
                            handle_slash_command(
                                cmd,
                                arg,
                                session_used_tokens,
                                session_context_size,
                                &cwd,
                                &fs,
                            )
                        );
                    }
                    continue;
                }

                let _ = rl.add_history_entry(&raw_line);

                let expanded = expand_mentions(&line, &cwd, &fs);
                match session.prompt(&expanded).await {
                    Err(e) => eprintln!("error: {e}"),
                    Ok(mut rx) => {
                        let mut stdout = std::io::stdout();
                        let ctrl_c = tokio::signal::ctrl_c();
                        tokio::pin!(ctrl_c);

                        loop {
                            tokio::select! {
                                biased;
                                _ = &mut ctrl_c => {
                                    session.cancel().await;
                                    eprintln!("\n[cancelled]");
                                    break;
                                }
                                event = rx.recv() => {
                                    match event {
                                        None => break,
                                        Some(StreamEvent::Text(text)) => {
                                            print!("{text}");
                                            let _ = stdout.flush();
                                        }
                                        Some(StreamEvent::Thinking) => {}
                                        Some(StreamEvent::ToolCall(name)) => {
                                            eprintln!("\n[tool: {name}]");
                                        }
                                        Some(StreamEvent::Diff(diff)) => {
                                            eprintln!("{diff}");
                                        }
                                        Some(StreamEvent::Usage { used_tokens, context_size }) => {
                                            session_used_tokens = used_tokens;
                                            session_context_size = context_size;
                                        }
                                        Some(StreamEvent::Done(reason)) => {
                                            if reason == "cancelled" {
                                                eprintln!("\n[cancelled]");
                                            } else {
                                                println!();
                                                if session_context_size > 0 {
                                                    let pct = session_used_tokens * 100 / session_context_size;
                                                    eprintln!(
                                                        "\x1b[2m[context: {}/{} tokens ({}%)]\x1b[0m",
                                                        fmt_tokens(session_used_tokens),
                                                        fmt_tokens(session_context_size),
                                                        pct,
                                                    );
                                                }
                                            }
                                            break;
                                        }
                                    }
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

fn handle_slash_command<F: Fs>(
    cmd: &str,
    arg: &str,
    used_tokens: u64,
    context_size: u64,
    cwd: &Path,
    fs: &F,
) -> String {
    match cmd {
        "/help" => "\
Commands:
  /help               show this help
  /cost               show token usage for this session
  /clear              start a new session (clears conversation history)
  /compact            show context usage and compaction status
  /config             show config  |  /config set <key> <value>
  /model              show current model  |  /model <id> change model
  /init               generate TROGON.md for this project

Multiline: end a line with \\ to continue on the next line
Ctrl+C    cancel active response
Ctrl+D    quit"
            .to_string(),

        "/cost" => {
            if context_size == 0 {
                "no usage data yet — send a message first".to_string()
            } else {
                let pct = used_tokens * 100 / context_size;
                let cost = estimate_cost(used_tokens, fs);
                format!(
                    "context: {}/{} tokens ({}%)  |  ~${cost}",
                    fmt_tokens(used_tokens),
                    fmt_tokens(context_size),
                    pct,
                )
            }
        }

        "/compact" => {
            if context_size == 0 {
                "no context data yet — send a message first to see usage".to_string()
            } else {
                let pct = used_tokens * 100 / context_size;
                let advice = if pct >= 85 {
                    "context is nearly full — compaction will trigger automatically before the next message"
                } else {
                    "use /clear to start a fresh session if you need immediate relief"
                };
                format!(
                    "context: {}/{} tokens ({}%)\nauto-compaction triggers at 85%\n{advice}",
                    fmt_tokens(used_tokens),
                    fmt_tokens(context_size),
                    pct,
                )
            }
        }

        "/config" => handle_config_cmd(arg, fs),

        "/model" => {
            if arg.is_empty() {
                let model = read_config(fs)
                    .get("model")
                    .and_then(|v| v.as_str())
                    .unwrap_or("claude-sonnet-4-6")
                    .to_string();
                format!("current model: {model}\nchange with: /model <model-id>")
            } else {
                let model = arg.trim();
                let mut cfg = read_config(fs);
                cfg["model"] = serde_json::Value::String(model.to_string());
                match write_config(&cfg, fs) {
                    Ok(()) => format!(
                        "model set to: {model}\n\
                         cost estimates updated  |  restart runner with AGENT_MODEL={model} to apply"
                    ),
                    Err(e) => format!("error saving model: {e}"),
                }
            }
        }

        "/init" => handle_init_cmd(cwd, fs),

        other => format!("unknown command: {other}  (type /help for a list)"),
    }
}

// ── /config ───────────────────────────────────────────────────────────────────

fn config_path() -> PathBuf {
    expand_tilde("~/.config/trogon/config.json")
}

fn read_config<F: Fs>(fs: &F) -> serde_json::Value {
    let path = config_path();
    fs.read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(serde_json::json!({}))
}

fn write_config<F: Fs>(config: &serde_json::Value, fs: &F) -> Result<(), String> {
    let path = config_path();
    if let Some(dir) = path.parent() {
        fs.create_dir_all(dir).map_err(|e| format!("cannot create config dir: {e}"))?;
    }
    let content = serde_json::to_string_pretty(config).map_err(|e| e.to_string())?;
    fs.write(&path, content.as_bytes()).map_err(|e| format!("cannot write config: {e}"))
}

fn handle_config_cmd<F: Fs>(arg: &str, fs: &F) -> String {
    let mut parts = arg.splitn(3, ' ');
    match parts.next().unwrap_or("") {
        "" => {
            let cfg = read_config(fs);
            let path = config_path();
            format!(
                "config: {}\n{}",
                path.display(),
                serde_json::to_string_pretty(&cfg).unwrap_or_default()
            )
        }
        "get" => {
            let key = parts.next().unwrap_or("");
            if key.is_empty() {
                return "usage: /config get <key>".to_string();
            }
            let cfg = read_config(fs);
            match cfg.get(key) {
                Some(v) => format!("{key} = {v}"),
                None => format!("{key} is not set"),
            }
        }
        "set" => {
            let key = parts.next().unwrap_or("");
            let value = parts.next().unwrap_or("");
            if key.is_empty() {
                return "usage: /config set <key> <value>".to_string();
            }
            let mut cfg = read_config(fs);
            cfg[key] = serde_json::Value::String(value.to_string());
            match write_config(&cfg, fs) {
                Ok(()) => format!("{key} = {value}"),
                Err(e) => format!("error: {e}"),
            }
        }
        other => format!("unknown config subcommand: {other}  (use get, set, or no args)"),
    }
}

// ── /init ─────────────────────────────────────────────────────────────────────

fn handle_init_cmd<F: Fs>(cwd: &Path, fs: &F) -> String {
    let root = find_git_root(cwd).unwrap_or_else(|| cwd.to_path_buf());
    let dest = root.join("TROGON.md");

    if fs.read_to_string(&dest).is_ok() {
        return format!(
            "TROGON.md already exists at {}\nEdit it to update project context.",
            dest.display()
        );
    }

    let project_name = root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "project".to_string());

    let langs = detect_languages(&root);
    let content = generate_trogon_md(&project_name, &langs);

    match fs.write(&dest, content.as_bytes()) {
        Ok(()) => format!(
            "created {}\nEdit it to give Claude context about this project.",
            dest.display()
        ),
        Err(e) => format!("error creating TROGON.md: {e}"),
    }
}

fn find_git_root(start: &Path) -> Option<PathBuf> {
    let mut dir = start.to_path_buf();
    loop {
        if dir.join(".git").exists() {
            return Some(dir);
        }
        if !dir.pop() {
            return None;
        }
    }
}

fn detect_languages(root: &Path) -> Vec<&'static str> {
    const INDICATORS: &[(&str, &str)] = &[
        ("Cargo.toml", "Rust"),
        ("package.json", "JavaScript/TypeScript"),
        ("go.mod", "Go"),
        ("pyproject.toml", "Python"),
        ("setup.py", "Python"),
        ("build.gradle", "Kotlin/Java"),
        ("build.gradle.kts", "Kotlin"),
        ("pom.xml", "Java"),
        ("Gemfile", "Ruby"),
        ("mix.exs", "Elixir"),
        ("pubspec.yaml", "Dart/Flutter"),
        ("CMakeLists.txt", "C/C++"),
    ];
    let mut found: Vec<&'static str> = Vec::new();
    for (file, lang) in INDICATORS {
        if root.join(file).exists() && !found.contains(lang) {
            found.push(lang);
        }
    }
    found
}

fn generate_trogon_md(project_name: &str, langs: &[&str]) -> String {
    let lang_line = if langs.is_empty() {
        String::new()
    } else {
        format!("\nLanguages/frameworks: {}\n", langs.join(", "))
    };
    format!(
        "# {project_name}\n\
         {lang_line}\
         \n## Overview\n\
         <!-- Describe the project purpose and goals -->\n\
         \n## Architecture\n\
         <!-- Key components and their interactions -->\n\
         \n## Development\n\
         <!-- Build, test, and run commands -->\n\
         \n## Notes\n\
         <!-- Project-specific context for Claude -->\n"
    )
}

// ── cost estimation ───────────────────────────────────────────────────────────

fn blended_rate_per_mtoken(model: &str) -> f64 {
    if model.contains("opus") {
        28.5
    } else if model.contains("haiku") {
        1.6
    } else {
        6.0
    }
}

fn estimate_cost<F: Fs>(tokens: u64, fs: &F) -> String {
    let model = read_config(fs)
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("claude-sonnet-4-6")
        .to_string();
    let rate = blended_rate_per_mtoken(&model);
    let cost = tokens as f64 * rate / 1_000_000.0;
    if cost < 0.01 {
        format!("{cost:.4}")
    } else {
        format!("{cost:.2}")
    }
}

// ── token formatting ──────────────────────────────────────────────────────────

fn fmt_tokens(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out.chars().rev().collect()
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
    use crate::fs::mock::MockFs;
    use crate::fs::RealFs;

    // ── expand_mentions with MockFs ───────────────────────────────────────────

    #[test]
    fn expand_mentions_no_at_sign_is_unchanged() {
        let fs = MockFs::new();
        assert_eq!(expand_mentions("hello world", Path::new("/tmp"), &fs), "hello world");
    }

    #[test]
    fn expand_mentions_lone_at_is_preserved() {
        let fs = MockFs::new();
        assert_eq!(
            expand_mentions("price @ discount", Path::new("/tmp"), &fs),
            "price @ discount"
        );
    }

    #[test]
    fn expand_mentions_missing_file_leaves_token_and_warns() {
        let fs = MockFs::new();
        let result = expand_mentions("see @nonexistent.txt please", Path::new("/tmp"), &fs);
        assert!(result.contains("@nonexistent.txt"));
        assert!(!result.contains("```"));
    }

    #[test]
    fn expand_mentions_existing_file_inserts_code_block() {
        let fs = MockFs::new();
        fs.add_file("/tmp/myfile.txt", "hello from file");
        let result = expand_mentions("look at @myfile.txt now", Path::new("/tmp"), &fs);
        assert!(result.contains("hello from file"), "got: {result}");
        assert!(result.contains("```"), "expected fenced block, got: {result}");
    }

    #[test]
    fn expand_mentions_multiple_mentions() {
        let fs = MockFs::new();
        fs.add_file("/tmp/f1.txt", "file1");
        fs.add_file("/tmp/f2.txt", "file2");
        let result = expand_mentions("a @f1.txt b @f2.txt c", Path::new("/tmp"), &fs);
        assert!(result.contains("file1"), "got: {result}");
        assert!(result.contains("file2"), "got: {result}");
    }

    #[test]
    fn expand_mentions_at_end_of_string_is_lone_at() {
        let fs = MockFs::new();
        assert_eq!(expand_mentions("end @", Path::new("/tmp"), &fs), "end @");
    }

    #[test]
    fn expand_mentions_code_block_includes_filename_in_header() {
        let fs = MockFs::new();
        fs.add_file("/tmp/header_test.txt", "content");
        let result = expand_mentions("@header_test.txt", Path::new("/tmp"), &fs);
        assert!(result.contains("`header_test.txt`"), "filename missing: {result}");
        assert!(result.contains("```"), "missing fenced block: {result}");
    }

    // ── expand_mentions with RealFs (temp files) ──────────────────────────────

    #[test]
    fn expand_mentions_real_fs_existing_file() {
        let dir = std::env::temp_dir();
        let path = dir.join("trogon_real_mention.txt");
        std::fs::write(&path, "real content").unwrap();
        let fname = path.file_name().unwrap().to_str().unwrap();
        let input = format!("look at @{fname} now");
        let result = expand_mentions(&input, &dir, &RealFs);
        assert!(result.contains("real content"), "got: {result}");
        let _ = std::fs::remove_file(&path);
    }

    // ── FileAtHelper tab-completion (uses real fs directly — no Fs trait) ─────

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

    #[test]
    fn file_at_helper_complete_filters_by_prefix() {
        let dir = std::env::temp_dir().join("trogon_compl_prefix");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("alpha.txt"), "").unwrap();
        std::fs::write(dir.join("alpha2.txt"), "").unwrap();
        std::fs::write(dir.join("beta.txt"), "").unwrap();

        let helper = FileAtHelper { cwd: dir.clone() };
        let history = rustyline::history::DefaultHistory::new();
        let ctx = Context::new(&history);
        let (start, pairs) = helper.complete("@alpha", 6, &ctx).unwrap();

        assert_eq!(start, 1);
        let names: Vec<&str> = pairs.iter().map(|p| p.display.as_str()).collect();
        assert!(names.contains(&"alpha.txt"));
        assert!(names.contains(&"alpha2.txt"));
        assert!(!names.contains(&"beta.txt"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_at_helper_complete_directory_gets_slash_suffix() {
        let dir = std::env::temp_dir().join("trogon_compl_dir");
        let sub = dir.join("mysubdir");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&sub).unwrap();

        let helper = FileAtHelper { cwd: dir.clone() };
        let history = rustyline::history::DefaultHistory::new();
        let ctx = Context::new(&history);
        let (_, pairs) = helper.complete("@my", 3, &ctx).unwrap();

        let names: Vec<&str> = pairs.iter().map(|p| p.display.as_str()).collect();
        assert!(names.contains(&"mysubdir/"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_at_helper_complete_results_are_sorted() {
        let dir = std::env::temp_dir().join("trogon_compl_sorted");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["zzz.txt", "aaa.txt", "mmm.txt"] {
            std::fs::write(dir.join(name), "").unwrap();
        }

        let helper = FileAtHelper { cwd: dir.clone() };
        let history = rustyline::history::DefaultHistory::new();
        let ctx = Context::new(&history);
        let (_, pairs) = helper.complete("@", 1, &ctx).unwrap();

        let names: Vec<&str> = pairs.iter().map(|p| p.display.as_str()).collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── multiline join ────────────────────────────────────────────────────────

    #[test]
    fn join_continuation_single_line_unchanged() {
        assert_eq!(join_continuation("hello world"), "hello world");
    }

    #[test]
    fn join_continuation_backslash_newline_becomes_space() {
        assert_eq!(
            join_continuation("line one\\\nline two\\\nline three"),
            "line one line two line three"
        );
    }

    #[test]
    fn join_continuation_no_backslash_newlines_unchanged() {
        assert_eq!(join_continuation("line one\nline two"), "line one\nline two");
    }

    // ── input_needs_continuation ──────────────────────────────────────────────

    #[test]
    fn continuation_true_when_ends_with_backslash() {
        assert!(input_needs_continuation("hello\\"));
        assert!(input_needs_continuation("\\"));
    }

    #[test]
    fn continuation_false_when_no_trailing_backslash() {
        assert!(!input_needs_continuation("hello world"));
        assert!(!input_needs_continuation(""));
        assert!(!input_needs_continuation("has\\backslash in middle"));
    }

    // ── fmt_tokens ────────────────────────────────────────────────────────────

    #[test]
    fn fmt_tokens_small_number() {
        assert_eq!(fmt_tokens(42), "42");
    }

    #[test]
    fn fmt_tokens_thousands() {
        assert_eq!(fmt_tokens(1_234), "1,234");
    }

    #[test]
    fn fmt_tokens_millions() {
        assert_eq!(fmt_tokens(1_234_567), "1,234,567");
    }

    // ── handle_slash_command with MockFs ──────────────────────────────────────

    #[test]
    fn slash_help_lists_all_commands() {
        let fs = MockFs::new();
        let out = handle_slash_command("/help", "", 0, 0, Path::new("/tmp"), &fs);
        assert!(out.contains("/help"));
        assert!(out.contains("/cost"));
        assert!(out.contains("/clear"));
        assert!(out.contains("/compact"));
        assert!(out.contains("/config"));
        assert!(out.contains("/model"));
        assert!(out.contains("/init"));
    }

    #[test]
    fn slash_cost_no_data_yet() {
        let fs = MockFs::new();
        let out = handle_slash_command("/cost", "", 0, 0, Path::new("/tmp"), &fs);
        assert!(out.contains("no usage data"), "got: {out}");
    }

    #[test]
    fn slash_cost_shows_percentage_and_estimated_cost() {
        let fs = MockFs::new();
        let out = handle_slash_command("/cost", "", 50_000, 200_000, Path::new("/tmp"), &fs);
        assert!(out.contains("25%"), "got: {out}");
        assert!(out.contains("50,000"), "got: {out}");
        assert!(out.contains("200,000"), "got: {out}");
        assert!(out.contains("~$"), "got: {out}");
    }

    // ── blended_rate / estimate_cost ──────────────────────────────────────────

    #[test]
    fn blended_rate_sonnet_default() {
        assert_eq!(blended_rate_per_mtoken("claude-sonnet-4-6"), 6.0);
    }

    #[test]
    fn blended_rate_opus() {
        assert_eq!(blended_rate_per_mtoken("claude-opus-4-7"), 28.5);
    }

    #[test]
    fn blended_rate_haiku() {
        assert_eq!(blended_rate_per_mtoken("claude-haiku-4-5"), 1.6);
    }

    #[test]
    fn blended_rate_unknown_model_falls_back_to_sonnet() {
        assert_eq!(blended_rate_per_mtoken("unknown-model"), 6.0);
    }

    #[test]
    fn estimate_cost_zero_tokens_is_zero() {
        let fs = MockFs::new();
        assert_eq!(estimate_cost(0, &fs), "0.0000");
    }

    #[test]
    fn estimate_cost_1m_tokens_sonnet_is_6_dollars() {
        let fs = MockFs::new(); // no model set → sonnet default
        assert_eq!(estimate_cost(1_000_000, &fs), "6.00");
    }

    #[test]
    fn estimate_cost_uses_model_from_config() {
        let fs = MockFs::new();
        handle_config_cmd("set model claude-opus-4-7", &fs);
        assert_eq!(estimate_cost(1_000_000, &fs), "28.50");
    }

    #[test]
    fn estimate_cost_small_amount_shows_four_decimals() {
        let fs = MockFs::new();
        let cost = estimate_cost(1_000, &fs);
        assert!(cost.starts_with("0.00"), "expected 4-decimal format, got: {cost}");
    }

    // ── handle_config_cmd with MockFs ─────────────────────────────────────────

    #[test]
    fn config_set_and_get_roundtrip() {
        let fs = MockFs::new();
        let set_out = handle_config_cmd("set mykey myvalue", &fs);
        assert!(set_out.contains("mykey") && set_out.contains("myvalue"), "got: {set_out}");
        let get_out = handle_config_cmd("get mykey", &fs);
        assert!(get_out.contains("myvalue"), "got: {get_out}");
    }

    #[test]
    fn config_get_missing_key_says_not_set() {
        let fs = MockFs::new();
        let out = handle_config_cmd("get nonexistent", &fs);
        assert!(out.contains("not set"), "got: {out}");
    }

    #[test]
    fn config_no_args_shows_path() {
        let fs = MockFs::new();
        let out = handle_config_cmd("", &fs);
        assert!(out.contains("config"), "got: {out}");
    }

    #[test]
    fn config_set_missing_key_shows_usage() {
        let fs = MockFs::new();
        assert!(handle_config_cmd("set", &fs).contains("usage"));
    }

    #[test]
    fn config_get_missing_key_arg_shows_usage() {
        let fs = MockFs::new();
        assert!(handle_config_cmd("get", &fs).contains("usage"));
    }

    #[test]
    fn config_unknown_subcommand_returns_error() {
        let fs = MockFs::new();
        assert!(handle_config_cmd("delete mykey", &fs).contains("unknown config subcommand"));
    }

    #[test]
    fn config_set_overwrites_existing_value() {
        let fs = MockFs::new();
        handle_config_cmd("set model claude-sonnet-4-6", &fs);
        let out = handle_config_cmd("set model claude-opus-4-7", &fs);
        assert!(out.contains("claude-opus-4-7"), "got: {out}");
        let get = handle_config_cmd("get model", &fs);
        assert!(get.contains("claude-opus-4-7") && !get.contains("claude-sonnet-4-6"), "got: {get}");
    }

    #[test]
    fn config_set_value_with_spaces_preserved() {
        let fs = MockFs::new();
        handle_config_cmd("set description hello world foo", &fs);
        let out = handle_config_cmd("get description", &fs);
        assert!(out.contains("hello world foo"), "got: {out}");
    }

    #[test]
    fn config_show_reflects_set_values() {
        let fs = MockFs::new();
        handle_config_cmd("set theme dark", &fs);
        let show = handle_config_cmd("", &fs);
        assert!(show.contains("dark") && show.contains("theme"), "got: {show}");
    }

    #[test]
    fn read_config_returns_empty_object_when_file_missing() {
        let fs = MockFs::new();
        let cfg = read_config(&fs);
        assert!(cfg.is_object() && cfg.as_object().unwrap().is_empty());
    }

    #[test]
    fn write_config_then_read_roundtrips() {
        let fs = MockFs::new();
        let cfg = serde_json::json!({"x": "y"});
        write_config(&cfg, &fs).unwrap();
        let read_back = read_config(&fs);
        assert_eq!(read_back["x"].as_str().unwrap(), "y");
    }

    // ── slash commands ────────────────────────────────────────────────────────

    #[test]
    fn slash_compact_no_data_explains() {
        let fs = MockFs::new();
        let out = handle_slash_command("/compact", "", 0, 0, Path::new("/tmp"), &fs);
        assert!(out.contains("no context data"), "got: {out}");
    }

    #[test]
    fn slash_compact_shows_usage_stats() {
        let fs = MockFs::new();
        let out = handle_slash_command("/compact", "", 100_000, 200_000, Path::new("/tmp"), &fs);
        assert!(out.contains("50%"), "got: {out}");
        assert!(out.contains("100,000"), "got: {out}");
        assert!(out.contains("85%"), "auto-compaction threshold missing: {out}");
    }

    #[test]
    fn slash_compact_nearly_full_shows_warning() {
        let fs = MockFs::new();
        let out = handle_slash_command("/compact", "", 175_000, 200_000, Path::new("/tmp"), &fs);
        assert!(out.contains("nearly full") || out.contains("87%"), "got: {out}");
    }

    #[test]
    fn slash_unknown_suggests_help() {
        let fs = MockFs::new();
        let out = handle_slash_command("/nope", "", 0, 0, Path::new("/tmp"), &fs);
        assert!(out.contains("unknown command") && out.contains("/help"), "got: {out}");
    }

    #[test]
    fn slash_model_without_arg_shows_current_model() {
        let fs = MockFs::new();
        let out = handle_slash_command("/model", "", 0, 0, Path::new("/tmp"), &fs);
        assert!(out.contains("current model"), "got: {out}");
        assert!(out.contains("claude-sonnet-4-6"), "expected default model: {out}");
    }

    #[test]
    fn slash_model_with_arg_saves_and_confirms() {
        let fs = MockFs::new();
        let out = handle_slash_command("/model", "claude-opus-4-7", 0, 0, Path::new("/tmp"), &fs);
        assert!(out.contains("claude-opus-4-7"), "got: {out}");
        assert!(!out.contains("not yet implemented"), "should be implemented: {out}");
        // Verify it was actually persisted.
        let check = handle_slash_command("/model", "", 0, 0, Path::new("/tmp"), &fs);
        assert!(check.contains("claude-opus-4-7"), "model not persisted: {check}");
    }

    #[test]
    fn slash_model_updates_cost_estimates() {
        let fs = MockFs::new();
        handle_slash_command("/model", "claude-haiku-4-5", 0, 0, Path::new("/tmp"), &fs);
        let cost_out = handle_slash_command("/cost", "", 1_000_000, 2_000_000, Path::new("/tmp"), &fs);
        // haiku rate is 1.6 $/Mtok → 1M tokens ≈ $1.60
        assert!(cost_out.contains("1.60") || cost_out.contains("1.6"), "got: {cost_out}");
    }

    #[test]
    fn slash_init_creates_trogon_md() {
        let fs = MockFs::new();
        let out = handle_slash_command("/init", "", 0, 0, Path::new("/tmp"), &fs);
        assert!(out.contains("TROGON.md"), "got: {out}");
        assert!(!out.contains("not yet implemented"), "should be implemented: {out}");
    }

    #[test]
    fn slash_init_idempotent_if_file_exists() {
        let fs = MockFs::new();
        // First call creates it.
        handle_slash_command("/init", "", 0, 0, Path::new("/tmp"), &fs);
        // Second call should say it already exists.
        let out = handle_slash_command("/init", "", 0, 0, Path::new("/tmp"), &fs);
        assert!(out.contains("already exists"), "got: {out}");
    }

    #[test]
    fn slash_init_content_has_required_sections() {
        let md = generate_trogon_md("my-project", &["Rust", "Python"]);
        assert!(md.contains("# my-project"), "missing title: {md}");
        assert!(md.contains("Rust"), "missing lang: {md}");
        assert!(md.contains("## Overview"), "missing section: {md}");
        assert!(md.contains("## Architecture"), "missing section: {md}");
        assert!(md.contains("## Development"), "missing section: {md}");
        assert!(md.contains("## Notes"), "missing section: {md}");
    }

    #[test]
    fn slash_cost_at_full_context_shows_100_percent() {
        let fs = MockFs::new();
        let out = handle_slash_command("/cost", "", 200_000, 200_000, Path::new("/tmp"), &fs);
        assert!(out.contains("100%"), "got: {out}");
    }

    #[test]
    fn slash_cost_rounds_down() {
        let fs = MockFs::new();
        let out = handle_slash_command("/cost", "", 1, 3, Path::new("/tmp"), &fs);
        assert!(out.contains("33%"), "got: {out}");
    }

    // ── expand_tilde ──────────────────────────────────────────────────────────

    #[test]
    fn expand_tilde_without_tilde_is_unchanged() {
        assert_eq!(expand_tilde("/absolute/path"), PathBuf::from("/absolute/path"));
    }

    #[test]
    fn expand_tilde_bare_tilde_without_slash_is_unchanged() {
        assert_eq!(expand_tilde("~nodot"), PathBuf::from("~nodot"));
    }
}
