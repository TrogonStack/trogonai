use crate::fs::Fs;
use crate::mcp::McpManager;
use crate::session::{CompactResult, Session, SessionFactory, StreamEvent};
use crate::session_store::{SessionIndex, new_session_entry};
use crate::RunnerSwitcher;
use rustyline::completion::{Completer, Pair};
use rustyline::error::ReadlineError;
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::validate::{ValidationContext, ValidationResult, Validator};
use rustyline::{Context, Editor, Helper};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use crate::client_supervisor::AcpClientSupervisor;
use crate::terminal::{print_tool_output, reset_display};
use crate::tui_client::PermissionCoordinator;
use trogon_registry::{Registry, RegistryStore};
use trogon_tools::fs::{resolve_directory_target, resolve_path};

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
    let cwd_str = cwd.to_string_lossy();
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
                match resolve_path(&cwd_str, path_str) {
                    Ok(full_path) => match fs.read_to_string(&full_path) {
                        Ok(content) => {
                            // MED-37: Cap file expansions to 200 KB to protect the context window.
                            const MAX_MENTION_BYTES: usize = 204_800;
                            let body = if content.len() > MAX_MENTION_BYTES {
                                eprintln!(
                                    "warning: @{path_str}: file too large ({} bytes), \
                                     showing first {MAX_MENTION_BYTES} bytes",
                                    content.len()
                                );
                                let boundary = content.floor_char_boundary(MAX_MENTION_BYTES);
                                format!(
                                    "[File truncated: {} bytes, showing first {MAX_MENTION_BYTES}]\n{}",
                                    content.len(),
                                    &content[..boundary]
                                )
                            } else {
                                content
                            };
                            result.push_str(&format!("`{path_str}`:\n```\n{body}\n```"));
                        }
                        Err(_) => {
                            eprintln!("warning: @{path_str}: file not found or not readable");
                            result.push('@');
                            result.push_str(path_str);
                        }
                    },
                    Err(_) => {
                        eprintln!("warning: @{path_str}: path escapes working directory — ignored");
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

#[allow(clippy::too_many_arguments)]
pub async fn run<SF: SessionFactory, F: Fs, SW: RunnerSwitcher, RS: RegistryStore>(
    factory: SF,
    prefix: &str,
    mut cwd: PathBuf,
    fs: F,
    mut switcher: SW,
    registry: Registry<RS>,
    client_supervisor: Option<Rc<AcpClientSupervisor>>,
    permission_coordinator: std::sync::Arc<PermissionCoordinator>,
    stream: bool,
    resume: Option<crate::session_store::SessionEntry>,
) -> anyhow::Result<()> {
    let mut prefix = prefix.to_string();
    let init_prefix = prefix.clone(); // always use the startup runner for /init
    let project_dir = cwd.canonicalize().unwrap_or_else(|_| cwd.clone());

    let mut mcp_manager = McpManager::load(&fs);
    let resumed = resume.is_some();
    let mut session = if let Some(entry) = resume {
        prefix = entry.prefix.clone();
        match activate_session(&factory, &mut mcp_manager, &prefix, &entry.session_id, &cwd).await {
            Ok(s) => {
                eprintln!(
                    "resumed session {} on {prefix}",
                    s.session_id()
                );
                s
            }
            Err(e) => {
                eprintln!(
                    "warning: could not resume {}: {e} — starting fresh",
                    entry.session_id
                );
                start_session(&factory, &mut mcp_manager, &prefix, cwd.clone()).await?
            }
        }
    } else {
        start_session(&factory, &mut mcp_manager, &prefix, cwd.clone()).await?
    };
    if let Some(ref sup) = client_supervisor {
        sup.set_session(session.session_id());
        if resumed
            && let Err(e) = sup.rebind(&prefix, session.session_id()).await
        {
            eprintln!("warning: permission client rebind failed: {e}");
        }
    }

    let history_path = expand_tilde(HISTORY_PATH);
    if let Some(dir) = history_path.parent() {
        let _ = fs.create_dir_all(dir);
    }

    let mut rl: Editor<FileAtHelper, _> = Editor::new()?;
    rl.set_helper(Some(FileAtHelper { cwd: cwd.clone() }));
    let _ = rl.load_history(&history_path);

    eprintln!("trogon — session {} (Ctrl+D to quit)", session.session_id());

    sync_repl_cwd_from_session(&session, &mut cwd).await;
    if let Some(helper) = rl.helper_mut() {
        helper.cwd = cwd.clone();
    }

    let mut session_used_tokens: u64 = 0;
    let mut session_context_size: u64 = 0;
    let mut session_mode =
        std::env::var("TROGON_MODE").unwrap_or_else(|_| "default".into());

    loop {
        permission_coordinator.cancel_pending();
        match rl.readline("> ") {
            Ok(raw_line) => {
                let line = join_continuation(&raw_line).trim().to_string();
                if line.is_empty() {
                    continue;
                }

                // Erase the readline echo line and replace with a styled user block
                eprint!("\x1b[1A\r\x1b[2K\x1b[1;35m┃\x1b[0m {line}\n");

                if line == "cd" || line.starts_with("cd ") {
                    if sync_repl_cwd_from_session(&session, &mut cwd).await
                        && let Some(helper) = rl.helper_mut()
                    {
                        helper.cwd = cwd.clone();
                    }
                    let arg = line.strip_prefix("cd").unwrap_or("").trim();
                    if apply_repl_cd(
                        &session,
                        &fs,
                        &mut cwd,
                        &project_dir,
                        &prefix,
                        &session.current_model(),
                        arg,
                    )
                    .await
                        && let Some(helper) = rl.helper_mut()
                    {
                        helper.cwd = cwd.clone();
                    }
                    continue;
                }

                if line.starts_with('/') {
                    let mut parts = line.splitn(2, ' ');
                    let cmd = parts.next().unwrap_or("");
                    let arg = parts.next().unwrap_or("");
                    if cmd == "/cd" {
                        if sync_repl_cwd_from_session(&session, &mut cwd).await
                            && let Some(helper) = rl.helper_mut()
                        {
                            helper.cwd = cwd.clone();
                        }
                        if apply_repl_cd(
                            &session,
                            &fs,
                            &mut cwd,
                            &project_dir,
                            &prefix,
                            &session.current_model(),
                            arg,
                        )
                        .await
                            && let Some(helper) = rl.helper_mut()
                        {
                            helper.cwd = cwd.clone();
                        }
                        continue;
                    } else if cmd == "/clear" {
                        mcp_manager.shutdown_session(session.session_id()).await;
                        session.close().await;
                        match start_session(&factory, &mut mcp_manager, &prefix, cwd.clone()).await {
                            Ok(s) => {
                                session = s;
                                session_used_tokens = 0;
                                session_context_size = 0;
                                session_mode = std::env::var("TROGON_MODE")
                                    .unwrap_or_else(|_| "default".into());
                                if let Some(ref sup) = client_supervisor {
                                    sup.set_session(session.session_id());
                                }
                                persist_session_index(
                                    &fs,
                                    &project_dir,
                                    &prefix,
                                    session.session_id(),
                                    &session.current_model(),
                                );
                                eprintln!("session cleared — new session {}", session.session_id());
                            }
                            Err(e) => eprintln!("error: {e}"),
                        }
                    } else if cmd == "/resume" {
                        if arg.is_empty() {
                            eprintln!("usage: /resume <session-id>");
                        } else {
                            let target = arg.trim();
                            let old_session_id = session.session_id().to_string();
                            // Shut down old MCP bridges before spawning new ones so that
                            // MCP servers that only allow one instance (e.g. port-binding
                            // servers) do not fail to start for the resumed session.
                            mcp_manager.shutdown_session(&old_session_id).await;
                            match activate_session(&factory, &mut mcp_manager, &prefix, target, &cwd).await {
                                Ok(s) => {
                                    session.close().await;
                                    session = s;
                                    session_used_tokens = 0;
                                    session_context_size = 0;
                                    if let Some(ref sup) = client_supervisor {
                                        sup.set_session(session.session_id());
                                    }
                                    persist_session_index(
                                        &fs,
                                        &project_dir,
                                        &prefix,
                                        session.session_id(),
                                        &session.current_model(),
                                    );
                                    eprintln!("resumed session {}", session.session_id());
                                }
                                Err(e) => {
                                    // MED-3: the old session's MCP bridges were shut down
                                    // before the switch attempt. Since resume failed and we
                                    // remain on the old session, restore its bridges so MCP
                                    // tools keep working instead of silently failing.
                                    eprintln!("error resuming session: {e}");
                                    if let Err(re) =
                                        respawn_session_mcp(&session, &mut mcp_manager, &cwd).await
                                    {
                                        eprintln!(
                                            "warning: could not restore MCP bridges for current session: {re}"
                                        );
                                    }
                                }
                            }
                        }
                    } else if cmd == "/sessions" {
                        match session.list_sessions().await {
                            Ok(list) if list.is_empty() => {
                                println!("no sessions on runner {prefix}");
                            }
                            Ok(list) => {
                                let index = SessionIndex::load(&fs);
                                println!(
                                    "{:<36}  {:<20}  cwd",
                                    "session_id", "updated"
                                );
                                for s in list {
                                    let updated = s.updated_at.as_deref().unwrap_or("-");
                                    let model = index
                                        .get_for_prefix(&project_dir, &prefix)
                                        .filter(|e| e.session_id == s.session_id)
                                        .map(|e| e.model.as_str())
                                        .unwrap_or("-");
                                    let label = s
                                        .title
                                        .as_deref()
                                        .filter(|t| !t.is_empty())
                                        .unwrap_or(&s.cwd);
                                    println!(
                                        "{:<36}  {:<20}  {}  [model: {model}]",
                                        s.session_id, updated, label
                                    );
                                }
                            }
                            Err(e) => eprintln!("error listing sessions: {e}"),
                        }
                    } else if cmd == "/model" && arg.is_empty() {
                        match format_model_catalog(
                            &registry,
                            &prefix,
                            &session.current_model(),
                            session.session_id(),
                        )
                        .await
                        {
                            Ok(text) => println!("{text}"),
                            Err(e) => eprintln!("error listing models: {e}"),
                        }
                    } else if cmd == "/pwd" {
                        if sync_repl_cwd_from_session(&session, &mut cwd).await
                            && let Some(helper) = rl.helper_mut()
                        {
                            helper.cwd = cwd.clone();
                        }
                        println!("{}", cwd.display());
                    } else if cmd == "/status" {
                        let text = format_status(
                            &registry,
                            &prefix,
                            &session.current_model(),
                            session.session_id(),
                            session_used_tokens,
                            session_context_size,
                        )
                        .await;
                        println!("{text}");
                    } else if cmd == "/doctor" {
                        // MED-4: use the URL the CLI actually connected to (which
                        // honors --nats-url / TROGON_NATS_URL) instead of the
                        // unrelated NATS_URL env var that was silently read before.
                        let url = client_supervisor
                            .as_ref()
                            .map(|sup| sup.nats_url().to_string())
                            .or_else(|| std::env::var("TROGON_NATS_URL").ok())
                            .unwrap_or_else(|| "nats://localhost:4222".to_string());
                        crate::doctor::print_checks(&url).await;
                    } else if cmd == "/model" && !arg.is_empty() {
                        let model_id = resolve_model_alias(arg.trim());
                        let cwd_str = cwd
                            .canonicalize()
                            .unwrap_or_else(|_| cwd.clone())
                            .to_string_lossy()
                            .into_owned();
                        match apply_model_switch(
                            &mut switcher,
                            &prefix,
                            session.session_id(),
                            &model_id,
                            &cwd_str,
                        )
                        .await
                        {
                            Ok(outcome) => {
                                if outcome.same_runner {
                                    match session.set_model(&model_id).await {
                                        Ok(()) => {
                                            println!("Model set to {model_id}");
                                            persist_session_index(
                                                &fs,
                                                &project_dir,
                                                &prefix,
                                                session.session_id(),
                                                &model_id,
                                            );
                                        }
                                        Err(e) => eprintln!("Error setting model: {e}"),
                                    }
                                } else {
                                    session.close().await;
                                    session =
                                        factory.attach_session(&outcome.new_prefix, outcome.new_session_id);
                                    prefix = outcome.new_prefix.clone();
                                    session_used_tokens = 0;
                                    session_context_size = 0;
                                    // MED-5: the new runner starts a session with mode
                                    // initialized from TROGON_MODE; keep the REPL's tracked
                                    // mode in sync so /status and permission prompts match.
                                    session_mode = std::env::var("TROGON_MODE")
                                        .unwrap_or_else(|_| "default".into());
                                    if let Some(ref sup) = client_supervisor
                                        && let Err(e) = sup
                                            .rebind(&outcome.new_prefix, session.session_id())
                                            .await
                                    {
                                        eprintln!("error rebinding permission client: {e}");
                                    }
                                    match session.set_model(&model_id).await {
                                        Ok(()) => {
                                            println!("Switched to {model_id}");
                                            persist_session_index(
                                                &fs,
                                                &project_dir,
                                                &prefix,
                                                session.session_id(),
                                                &model_id,
                                            );
                                        }
                                        Err(e) => eprintln!("Error setting model on new runner: {e}"),
                                    }
                                }
                            }
                            Err(e) => eprintln!("Error switching model: {e}"),
                        }
                    } else if cmd == "/mode" {
                        if arg.is_empty() {
                            println!(
                                "current mode: {session_mode}\nchange with: \x1b[35m/mode\x1b[0m \
                                 <default|acceptEdits|plan|dontAsk|bypassPermissions>"
                            );
                        } else {
                            let mode = arg.trim();
                            match session.set_mode(mode).await {
                                Ok(()) => {
                                    session_mode = mode.to_string();
                                    println!("mode set to {mode}");
                                }
                                Err(e) => eprintln!("error setting mode: {e}"),
                            }
                        }
                    } else if cmd == "/compact" {
                        match session.compact().await {
                            Ok(CompactResult { compacted: true, tokens_before, tokens_after }) => {
                                println!(
                                    "compacted: {} → {} tokens",
                                    fmt_tokens(tokens_before as u64),
                                    fmt_tokens(tokens_after as u64),
                                );
                            }
                            Ok(CompactResult { compacted: false, tokens_before, .. }) => {
                                if tokens_before > 0 {
                                    println!(
                                        "no compaction needed ({} tokens)",
                                        fmt_tokens(tokens_before as u64),
                                    );
                                } else {
                                    println!("no messages to compact");
                                }
                            }
                            Err(e) => eprintln!("error triggering compaction: {e}"),
                        }
                    } else if cmd == "/mcp" {
                        handle_mcp_command(
                            arg,
                            &mut mcp_manager,
                            &fs,
                            &session,
                            &cwd,
                        )
                        .await;
                    } else if cmd == "/memory" {
                        handle_memory_command(arg, &cwd, &fs).await;
                    } else if cmd == "/init" {
                        let force = arg == "--force";
                        let root = find_git_root(&cwd).unwrap_or_else(|| cwd.clone());
                        let dest = root.join("TROGON.md");
                        if fs.read_to_string(&dest).is_ok() && !force {
                            println!(
                                "TROGON.md already exists at {}\nRun \x1b[35m/init --force\x1b[0m to overwrite.",
                                dest.display()
                            );
                        } else {
                            eprintln!("analyzing project with AI...");
                            let prompt = build_init_prompt(&root, &fs);
                            match factory.create_session(&init_prefix, cwd.clone(), vec![]).await {
                                Err(e) => eprintln!("error creating init session: {e}"),
                                Ok(init_session) => {
                                    // Bypass approval gates so Claude can write TROGON.md without prompting.
                                    let _ = init_session.set_mode("bypassPermissions").await;
                                    match init_session.prompt(&prompt).await {
                                        Err(e) => eprintln!("error: {e}"),
                                        Ok(mut rx) => {
                                            let mut content = String::new();
                                            let mut stdout = std::io::stdout();
                                            let mut runner_error: Option<String> = None;
                                            loop {
                                                match rx.recv().await {
                                                    None => break,
                                                    Some(StreamEvent::Text(t)) => {
                                                        content.push_str(&t);
                                                        print!("{t}");
                                                        let _ = stdout.flush();
                                                    }
                                                    Some(StreamEvent::Error(msg)) => {
                                                        runner_error = Some(msg);
                                                        break;
                                                    }
                                                    Some(StreamEvent::Done(_)) => {
                                                        println!();
                                                        break;
                                                    }
                                                    _ => {}
                                                }
                                            }
                                            init_session.close().await;
                                            if let Some(err) = runner_error {
                                                eprintln!("error: {err}");
                                            } else {
                                            let trogon_content = strip_code_fence(&content);
                                            if trogon_content.is_empty() {
                                                // Model may have written the file directly via write_file tool.
                                                // If it has content, keep it; otherwise report failure.
                                                if fs.read_to_string(&dest).map(|s| !s.is_empty()).unwrap_or(false) {
                                                    println!("created {}", dest.display());
                                                } else {
                                                    eprintln!("error: model produced no content — try again");
                                                }
                                            } else {
                                                match fs.write(&dest, trogon_content.as_bytes()) {
                                                    Ok(()) => println!("created {}", dest.display()),
                                                    Err(e) => eprintln!("error writing TROGON.md: {e}"),
                                                }
                                            }
                                            } // close runner_error else
                                        }
                                    }
                                }
                            }
                        }
                    } else {
                        println!(
                            "{}",
                            handle_slash_command(
                                cmd,
                                arg,
                                session_used_tokens,
                                session_context_size,
                                &session.current_model(),
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
                        let mut response_buf = String::new();
                        // true while a ┆ tool line is live on stderr (no trailing newline)
                        let mut tool_line_active = false;

                        loop {
                            tokio::select! {
                                biased;
                                _ = &mut ctrl_c => {
                                    session.cancel().await;
                                    if tool_line_active { eprint!("\r\x1b[2K\n"); let _ = std::io::stderr().flush(); }
                                    eprintln!("\n[cancelled]");
                                    break;
                                }
                                event = rx.recv() => {
                                    match event {
                                        None => break,
                                        Some(StreamEvent::Text(text)) => {
                                            if tool_line_active {
                                                // clear tool line before text starts
                                                eprint!("\r\x1b[2K\n");
                                                let _ = std::io::stderr().flush();
                                                tool_line_active = false;
                                                reset_display();
                                            }
                                            reset_display();
                                            if stream {
                                                print!("{text}");
                                                let _ = stdout.flush();
                                            } else {
                                                response_buf.push_str(&text);
                                            }
                                        }
                                        Some(StreamEvent::Thinking) => {}
                                        Some(StreamEvent::ToolCall(name)) => {
                                            if !stream && !response_buf.is_empty() {
                                                reset_display();
                                                print!("{}", crate::markdown::render(&response_buf));
                                                let _ = stdout.flush();
                                                response_buf.clear();
                                            }
                                            if tool_line_active {
                                                // overwrite the previous tool line in place
                                                eprint!("\r\x1b[2K\x1b[2m┆ {name}\x1b[0m");
                                            } else {
                                                eprint!("\n\x1b[2m┆ {name}\x1b[0m");
                                                tool_line_active = true;
                                            }
                                            let _ = std::io::stderr().flush();
                                        }
                                        Some(StreamEvent::Diff(diff)) => {
                                            reset_display();
                                            eprintln!("{diff}");
                                        }
                                        Some(StreamEvent::ToolFinished {
                                            name,
                                            output,
                                            exit_code,
                                            status,
                                        }) => {
                                            if tool_line_active {
                                                eprint!("\r\x1b[2K\n");
                                                let _ = std::io::stderr().flush();
                                                tool_line_active = false;
                                            }
                                            let failed = matches!(status, agent_client_protocol::ToolCallStatus::Failed);
                                            let code_suffix = exit_code
                                                .map(|c| format!(" (exit {c})"))
                                                .unwrap_or_default();
                                            let badge = if failed { " [failed]" } else { "" };
                                            eprintln!(
                                                "\x1b[2m┆ {name}{code_suffix}{badge}\x1b[0m"
                                            );
                                            if !output.is_empty() {
                                                print_tool_output(&output);
                                                if sync_cwd_from_tool(&name, &output, &mut cwd)
                                                    && let Some(helper) = rl.helper_mut()
                                                {
                                                    helper.cwd = cwd.clone();
                                                }
                                            }
                                        }
                                        Some(StreamEvent::Usage { used_tokens, context_size }) => {
                                            session_used_tokens = used_tokens;
                                            session_context_size = context_size;
                                        }
                                        Some(StreamEvent::Error(msg)) => {
                                            if tool_line_active { eprint!("\r\x1b[2K\n"); let _ = std::io::stderr().flush(); }
                                            if !stream && !response_buf.is_empty() {
                                                reset_display();
                                                print!("{}", crate::markdown::render(&response_buf));
                                                response_buf.clear();
                                            }
                                            eprintln!("\n\x1b[31merror: {msg}\x1b[0m");
                                            let _ = stdout.flush();
                                            break;
                                        }
                                        Some(StreamEvent::Done(reason)) => {
                                            if tool_line_active { eprint!("\r\x1b[2K\n"); let _ = std::io::stderr().flush(); }
                                            if !stream && !response_buf.is_empty() {
                                                reset_display();
                                                print!("{}", crate::markdown::render(&response_buf));
                                                response_buf.clear();
                                            }
                                            if reason == "cancelled" {
                                                eprintln!("\n[cancelled]");
                                            } else if reason == "maxTurnRequests" {
                                                eprintln!("\n\x1b[33m[max tool rounds reached — try rephrasing or using \x1b[35m/compact\x1b[33m]\x1b[0m");
                                            } else {
                                                println!();
                                                if session_context_size > 0 {
                                                    let pct = session_used_tokens * 100 / session_context_size;
                                                    eprintln!(
                                                        "\x1b[90m[tokens: {}/{} ({}%)]\x1b[0m",
                                                        fmt_tokens(session_used_tokens),
                                                        fmt_tokens(session_context_size),
                                                        pct,
                                                    );
                                                }
                                                eprintln!();
                                                persist_session_index(
                                                    &fs,
                                                    &project_dir,
                                                    &prefix,
                                                    session.session_id(),
                                                    &session.current_model(),
                                                );
                                                sync_repl_cwd_from_session(&session, &mut cwd).await;
                                                if let Some(helper) = rl.helper_mut() {
                                                    helper.cwd = cwd.clone();
                                                }
                                            }
                                            let _ = stdout.flush();
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
                eprint!("\r\x1b[K");
                mcp_manager.shutdown_session(session.session_id()).await;
                session.close().await;
                mcp_manager.shutdown_all().await;
                break;
            }
            Err(e) => {
                // MED-2: tear down the session and MCP bridges on any readline
                // error, not just EOF, so we don't leak the runner session and
                // MCP child processes when the terminal errors out.
                eprintln!("readline error: {e}");
                mcp_manager.shutdown_session(session.session_id()).await;
                session.close().await;
                mcp_manager.shutdown_all().await;
                break;
            }
        }
    }

    let _ = rl.save_history(&history_path);
    Ok(())
}

async fn start_session<SF: SessionFactory>(
    factory: &SF,
    mcp: &mut McpManager,
    prefix: &str,
    cwd: PathBuf,
) -> anyhow::Result<SF::Sess> {
    let mcp_servers = mcp.spawn_pending().await;
    let session = factory.create_session(prefix, cwd, mcp_servers).await?;
    mcp.commit_pending(session.session_id());
    Ok(session)
}

async fn activate_session<SF: SessionFactory>(
    factory: &SF,
    mcp: &mut McpManager,
    prefix: &str,
    session_id: &str,
    cwd: &Path,
) -> anyhow::Result<SF::Sess> {
    let mcp_servers = mcp.spawn_pending().await;
    let session = factory.attach_session(prefix, session_id.to_string());
    session.load_session(session_id, cwd, mcp_servers).await?;
    mcp.commit_pending(session_id);
    Ok(session)
}

async fn handle_mcp_command<F: Fs, S: Session>(
    arg: &str,
    mcp: &mut McpManager,
    fs: &F,
    session: &S,
    cwd: &Path,
) {
    let mut parts = arg.splitn(2, ' ');
    let sub = parts.next().unwrap_or("list").trim();
    let rest = parts.next().unwrap_or("").trim();

    match sub {
        "list" => {
            println!("configured MCP servers (~/.config/trogon/mcp.json):");
            if mcp.configured_servers().is_empty() {
                println!("  (none)");
            } else {
                for s in mcp.configured_servers() {
                    println!("  {} — {} {}", s.name, s.command, s.args.join(" "));
                }
            }
            let active = mcp.active_for_session(session.session_id());
            if active.is_empty() {
                println!("active bridges: (none)");
            } else {
                println!("active bridges:");
                for (name, url) in active {
                    println!("  {name} → {url}");
                }
            }
        }
        "add" => {
            let mut add_parts = rest.splitn(2, ' ');
            let name = add_parts.next().unwrap_or("").trim();
            let cmd_rest = add_parts.next().unwrap_or("").trim();
            match McpManager::parse_add_args(name, cmd_rest) {
                Err(e) => eprintln!("{e}"),
                Ok(cfg) => {
                    mcp.add_server(cfg);
                    if let Err(e) = mcp.save(fs) {
                        eprintln!("error saving MCP config: {e}");
                    } else {
                        println!("added MCP server `{name}`");
                        if let Err(e) = respawn_session_mcp(session, mcp, cwd).await {
                            eprintln!("warning: could not refresh session MCP: {e}");
                        }
                    }
                }
            }
        }
        "remove" => {
            if rest.is_empty() {
                eprintln!("usage: /mcp remove <name>");
            } else if mcp.remove_server(rest) {
                if let Err(e) = mcp.save(fs) {
                    eprintln!("error saving MCP config: {e}");
                } else {
                    mcp.shutdown_session(session.session_id()).await;
                    println!("removed MCP server `{rest}`");
                    if let Err(e) = respawn_session_mcp(session, mcp, cwd).await {
                        eprintln!("warning: could not refresh session MCP: {e}");
                    }
                }
            } else {
                eprintln!("no MCP server named `{rest}`");
            }
        }
        other => eprintln!("unknown /mcp subcommand `{other}` — try list, add, remove"),
    }
}

async fn respawn_session_mcp<S: Session>(
    session: &S,
    mcp: &mut McpManager,
    cwd: &Path,
) -> anyhow::Result<()> {
    mcp.shutdown_session(session.session_id()).await;
    let servers = mcp.spawn_pending().await;
    mcp.commit_pending(session.session_id());
    session.load_session(session.session_id(), cwd, servers).await
}

async fn sync_repl_cwd_from_session<S: Session>(session: &S, cwd: &mut PathBuf) -> bool {
    if let Ok(Some(runner_cwd)) = session.session_cwd().await {
        let canonical = runner_cwd.canonicalize().unwrap_or(runner_cwd);
        if *cwd != canonical {
            *cwd = canonical.clone();
            return true;
        }
    }
    false
}

fn sync_cwd_from_tool(name: &str, output: &str, cwd: &mut PathBuf) -> bool {
    let prefix = "Working directory is now ";
    let is_cd = name == "change_directory"
        || (name == "bash" && output.starts_with(prefix));
    if !is_cd {
        return false;
    }
    let Some(path) = output.strip_prefix(prefix) else {
        return false;
    };
    let path = path.trim();
    if path.is_empty() {
        return false;
    }
    *cwd = PathBuf::from(path);
    true
}

async fn apply_repl_cd<S: Session, F: Fs>(
    session: &S,
    fs: &F,
    cwd: &mut PathBuf,
    project_dir: &Path,
    prefix: &str,
    model: &str,
    arg: &str,
) -> bool {
    match resolve_directory_target(&cwd.to_string_lossy(), arg) {
        Ok(resolved) => {
            *cwd = resolved.clone();
            match session.set_cwd(&resolved).await {
                Ok(()) => {
                    persist_session_index(fs, project_dir, prefix, session.session_id(), model);
                    reset_display();
                    eprintln!("{}", resolved.display());
                }
                Err(e) => {
                    eprintln!("warning: runner cwd update failed: {e} — local shell only");
                    reset_display();
                    eprintln!("{}", resolved.display());
                }
            }
            true
        }
        Err(e) => {
            eprintln!("{e}");
            false
        }
    }
}

fn persist_session_index<F: Fs>(
    fs: &F,
    project: &Path,
    prefix: &str,
    session_id: &str,
    model: &str,
) {
    let mut index = SessionIndex::load(fs);
    index.record(project, new_session_entry(prefix, session_id, model));
    if let Err(e) = index.save(fs) {
        eprintln!("warning: could not save session index: {e}");
    }
}

// ── /model handler ────────────────────────────────────────────────────────────

#[derive(Debug)]
pub(crate) struct ModelSwitchOutcome {
    pub same_runner: bool,
    pub new_prefix: String,
    pub new_session_id: String,
}

pub fn resolve_model_alias(input: &str) -> String {
    match input {
        "haiku" => "claude-haiku-4-5-20251001".into(),
        "sonnet" => "claude-sonnet-4-6".into(),
        "opus" => "claude-opus-4-7".into(),
        "grok" => "grok-3".into(),
        "grok-mini" => "grok-3-mini".into(),
        "openrouter" => std::env::var("OPENROUTER_DEFAULT_MODEL")
            .unwrap_or_else(|_| "anthropic/claude-sonnet-4".into()),
        "codex" => std::env::var("CODEX_DEFAULT_MODEL").unwrap_or_else(|_| "o4-mini".into()),
        "o3" => "o3".into(),
        "gpt-4o" => "gpt-4o".into(),
        other => other.into(),
    }
}

pub(crate) async fn apply_model_switch<SW: RunnerSwitcher>(
    switcher: &mut SW,
    current_prefix: &str,
    current_session_id: &str,
    model_id: &str,
    cwd: &str,
) -> Result<ModelSwitchOutcome, String> {
    let (new_prefix, new_session_id) =
        switcher.switch_model(current_prefix, current_session_id, model_id, cwd).await?;
    Ok(ModelSwitchOutcome {
        same_runner: new_prefix == current_prefix,
        new_prefix,
        new_session_id,
    })
}

fn handle_slash_command<F: Fs>(
    cmd: &str,
    arg: &str,
    used_tokens: u64,
    context_size: u64,
    session_model: &str,
    _cwd: &Path,
    fs: &F,
) -> String {
    match cmd {
        "/help" => {
            let m = "\x1b[35m";
            let r = "\x1b[0m";
            format!("\
Commands:
  {m}/help{r}               show this help
  {m}/doctor{r}             run health checks (same as `trogon doctor`)
  {m}/status{r}             prefix, model, tokens, registered runners
  {m}/cost{r}               show token usage for this session
  {m}/clear{r}              start a new session (clears conversation history)
  {m}/sessions{r}           list sessions on the current runner
  {m}/resume{r} <id>        resume a session by id on the current runner
  {m}/mcp{r} list|add|remove  manage MCP server bridges
  {m}/compact{r}            force context compaction now
  {m}/config{r}             show config  |  {m}/config{r} set <key> <value>
  {m}/model{r}              show current model  |  {m}/model{r} <id> change model
  {m}/mode{r}               show permission mode |  {m}/mode{r} <name> change mode
  {m}/memory{r} list|show|edit  TROGON.md hierarchy (project memory)
  {m}/init{r}               analyze project with AI and generate TROGON.md
  {m}/init --force{r}       overwrite existing TROGON.md
  {m}cd{r} [path]           change working directory (same as {m}/cd{r})
  {m}/cd{r} [path]          change working directory (~ supported)

Multiline: end a line with \\ to continue on the next line
Ctrl+C    cancel active response
Ctrl+D    quit")
        }

        "/cost" => {
            if context_size == 0 {
                "no usage data yet — send a message first".to_string()
            } else {
                let pct = used_tokens * 100 / context_size;
                let cost = estimate_cost(used_tokens, session_model);
                format!(
                    "context: {}/{} tokens ({}%)  |  ~${cost}",
                    fmt_tokens(used_tokens),
                    fmt_tokens(context_size),
                    pct,
                )
            }
        }

        "/config" => handle_config_cmd(arg, fs),

        "/model" => {
            if arg.is_empty() {
                "use /model with no args in the REPL for the full registry listing\n\
                 (config file model shown here only in unit tests)"
                    .to_string()
            } else {
                let model = arg.trim();
                let mut cfg = read_config(fs);
                cfg["model"] = serde_json::Value::String(model.to_string());
                match write_config(&cfg, fs) {
                    Ok(()) => format!("model set to: {model}\ncost estimates updated"),
                    Err(e) => format!("error saving model: {e}"),
                }
            }
        }

        "/doctor" => "run `trogon doctor` from the shell for full health checks".to_string(),

        "/status" => "use /status in the REPL for live session status".to_string(),

        "/memory" => {
            "use /memory in the REPL to list or show TROGON.md hierarchy".to_string()
        }

        other => format!("unknown command: {other}  (type \x1b[35m/help\x1b[0m for a list)"),
    }
}

async fn handle_memory_command<F: Fs>(arg: &str, cwd: &Path, fs: &F) {
    let arg = arg.trim();
    let layers = trogon_runner_tools::list_trogon_md_hierarchy(
        &cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf()).to_string_lossy(),
    )
    .await;

    if arg.is_empty() || arg == "list" {
        println!("TROGON.md hierarchy (general → specific):");
        for layer in &layers {
            let status = if layer.exists { "present" } else { "missing" };
            println!("  [{status}] {} — {}", layer.label, layer.path.display());
        }
        let project = trogon_runner_tools::project_trogon_md_path(cwd);
        println!("\nProject memory file: {}", project.display());
        println!("  /memory show     — print merged or project file");
        println!("  /memory edit     — print path to open in $EDITOR");
        return;
    }

    if arg == "edit" {
        let project = trogon_runner_tools::project_trogon_md_path(cwd);
        let editor = std::env::var("EDITOR").unwrap_or_else(|_| "nano".into());
        println!("Project memory: {}", project.display());
        println!("Global memory:  ~/.config/trogon/TROGON.md");
        println!("Open with: {editor} {}", project.display());
        if !project.exists() {
            println!("File does not exist yet — run \x1b[35m/init\x1b[0m to generate it.");
        }
        return;
    }

    if arg == "show" || arg.starts_with("show ") {
        let path = if arg == "show" {
            trogon_runner_tools::project_trogon_md_path(cwd)
        } else {
            let raw = PathBuf::from(arg.trim_start_matches("show ").trim());
            // Only allow paths within the current project directory.
            let mut norm = std::path::PathBuf::new();
            for comp in raw.components() {
                match comp {
                    std::path::Component::ParentDir => { norm.pop(); }
                    std::path::Component::CurDir => {}
                    c => norm.push(c),
                }
            }
            let resolved = if norm.is_absolute() { norm } else { cwd.join(norm) };
            let mut norm2 = std::path::PathBuf::new();
            for comp in resolved.components() {
                match comp {
                    std::path::Component::ParentDir => { norm2.pop(); }
                    std::path::Component::CurDir => {}
                    c => norm2.push(c),
                }
            }
            if !norm2.starts_with(cwd) {
                eprintln!("/memory show: path must be within the project directory");
                return;
            }
            norm2
        };
        match fs.read_to_string(&path) {
            Ok(content) => {
                println!("--- {} ---", path.display());
                println!("{content}");
            }
            Err(e) => eprintln!("could not read {}: {e}", path.display()),
        }
        return;
    }

    println!("unknown /memory subcommand: {arg}");
    println!("usage: /memory [list|show|edit]");
}

fn runner_display_name(prefix: &str, agent_type: &str) -> String {
    match prefix {
        "acp.claude" => "Claude".into(),
        "acp.grok" => "Grok".into(),
        "acp.openrouter" => "OpenRouter".into(),
        "acp.codex" => "Codex".into(),
        _ => agent_type.to_string(),
    }
}

pub(crate) async fn format_model_catalog<RS: RegistryStore>(
    registry: &Registry<RS>,
    current_prefix: &str,
    current_model: &str,
    session_id: &str,
) -> Result<String, String> {
    let all = registry.list_all().await.map_err(|e| e.to_string())?;
    let mut by_prefix: std::collections::BTreeMap<String, Vec<(String, String)>> =
        std::collections::BTreeMap::new();

    for cap in all {
        let prefix = cap
            .metadata
            .get("acp_prefix")
            .and_then(|v| v.as_str())
            .unwrap_or(&cap.agent_type)
            .to_string();
        let label = runner_display_name(&prefix, &cap.agent_type);
        let models = cap
            .metadata
            .get("models")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| m.as_str().map(str::to_string))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        by_prefix
            .entry(format!("{label} ({prefix})"))
            .or_default()
            .extend(models.into_iter().map(|id| (id.clone(), id)));
    }

    let mut out = String::new();
    for (group, models) in by_prefix {
        out.push_str(&group);
        out.push('\n');
        for (id, _) in models {
            let alias = match id.as_str() {
                "claude-sonnet-4-6" => Some("sonnet"),
                "claude-opus-4-7" => Some("opus"),
                "claude-haiku-4-5-20251001" => Some("haiku"),
                "grok-3" => Some("grok"),
                "grok-3-mini" => Some("grok-mini"),
                "o4-mini" => Some("o4-mini"),
                "o3" => Some("o3"),
                "gpt-4o" => Some("gpt-4o"),
                _ => None,
            };
            if let Some(a) = alias {
                out.push_str(&format!("  {a:<10} → {id}\n"));
            } else {
                out.push_str(&format!("  {id}\n"));
            }
        }
        out.push('\n');
    }
    out.push_str(&format!(
        "Current session: {current_model} on {current_prefix} ({session_id})"
    ));
    Ok(out)
}

pub(crate) async fn format_status<RS: RegistryStore>(
    registry: &Registry<RS>,
    prefix: &str,
    model: &str,
    session_id: &str,
    used_tokens: u64,
    context_size: u64,
) -> String {
    let runners = registry
        .list_all()
        .await
        .map(|all| {
            all.iter()
                .filter_map(|c| c.metadata.get("acp_prefix").and_then(|v| v.as_str()))
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_else(|_| "unknown".into());

    let tokens = if context_size == 0 {
        format!("{used} tokens used (context size unknown)", used = fmt_tokens(used_tokens))
    } else {
        let pct = used_tokens * 100 / context_size;
        format!(
            "{used}/{ctx} tokens ({pct}%)",
            used = fmt_tokens(used_tokens),
            ctx = fmt_tokens(context_size),
            pct = pct,
        )
    };

    format!(
        "prefix: {prefix}\nmodel:   {model}\nsession: {session_id}\n{tokens}\nrunners: {runners}"
    )
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

// ── /init helpers ─────────────────────────────────────────────────────────────

fn build_init_prompt<F: Fs>(root: &Path, fs: &F) -> String {
    let project_name = root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "project".to_string());

    let langs = detect_languages(root);
    let lang_line = if langs.is_empty() {
        String::new()
    } else {
        format!("Languages: {}\n", langs.join(", "))
    };

    let listing = list_top_level(root);

    let readme_section = ["README.md", "README.rst", "README.txt", "README"]
        .iter()
        .find_map(|name| {
            fs.read_to_string(&root.join(name)).ok().map(|content| {
                let truncated = if content.len() > 3000 {
                    format!("{}…(truncated)", &content[..3000])
                } else {
                    content
                };
                format!("{name}:\n{truncated}\n")
            })
        })
        .unwrap_or_default();

    format!(
        "Analyze this software project and generate the content of a TROGON.md file.\n\
         TROGON.md gives AI coding assistants context about the project.\n\
         \n\
         IMPORTANT: Do NOT call any tools (no read_file, no write_file, no list_dir, etc.).\n\
         Output the TROGON.md content directly as plain text in your response — I will write it to disk.\n\
         \n\
         Project: {project_name}\n\
         {lang_line}\
         Top-level structure:\n{listing}\n\
         \n\
         {readme_section}\
         Write a complete TROGON.md with these sections:\n\
         # {project_name}\n\
         ## Overview — purpose and goals\n\
         ## Architecture — key components and how they interact\n\
         ## Development — build, test, and run commands\n\
         ## Notes — conventions and important context for an AI assistant\n\
         \n\
         Output ONLY the TROGON.md content starting with the # heading. No preamble, no explanation."
    )
}

fn list_top_level(root: &Path) -> String {
    let Ok(entries) = std::fs::read_dir(root) else {
        return "(unable to list directory)".to_string();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                return None;
            }
            let suffix = if e.path().is_dir() { "/" } else { "" };
            Some(format!("{name}{suffix}"))
        })
        .collect();
    names.sort();
    names.join("\n")
}

fn strip_code_fence(s: &str) -> String {
    let s = s.trim();
    let inner = s
        .strip_prefix("```markdown\n")
        .or_else(|| s.strip_prefix("```md\n"))
        .or_else(|| s.strip_prefix("```\n"))
        .map(|inner| inner.strip_suffix("```").unwrap_or(inner).trim_end())
        .unwrap_or(s);
    inner.to_string()
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

// ── cost estimation ───────────────────────────────────────────────────────────

fn blended_rate_per_mtoken(model: &str) -> f64 {
    if model.contains("opus") {
        28.5
    } else if model.contains("haiku") {
        1.6
    } else if model.contains("grok") {
        2.0
    } else {
        6.0
    }
}

fn estimate_cost(tokens: u64, model: &str) -> String {
    let rate = blended_rate_per_mtoken(model);
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
    if let Some(rest) = path.strip_prefix("~/")
        && let Some(home) = std::env::var("HOME").ok().map(PathBuf::from)
    {
        return home.join(rest);
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

    #[test]
    fn expand_mentions_symlink_outside_cwd_is_rejected() {
        let dir = std::env::temp_dir().join("trogon_mention_symlink");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let outside = std::env::temp_dir().join("trogon_mention_outside_secret.txt");
        std::fs::write(&outside, "secret").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            symlink(&outside, dir.join("leak.txt")).unwrap();
        }
        #[cfg(not(unix))]
        {
            return;
        }
        let cwd = dir.canonicalize().unwrap();
        let result = expand_mentions("see @leak.txt", &cwd, &RealFs);
        assert!(result.contains("@leak.txt"), "token should be preserved: {result}");
        assert!(!result.contains("secret"), "must not read through symlink: {result}");
        let _ = std::fs::remove_file(&outside);
        let _ = std::fs::remove_dir_all(&dir);
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
        let out = handle_slash_command("/help", "", 0, 0, "claude-sonnet-4-6", Path::new("/tmp"), &fs);
        assert!(out.contains("/help"));
        assert!(out.contains("/cost"));
        assert!(out.contains("/clear"));
        assert!(out.contains("/sessions"));
        assert!(out.contains("/resume"));
        assert!(out.contains("/compact"));
        assert!(out.contains("/config"));
        assert!(out.contains("/model"));
        assert!(out.contains("/doctor"));
        assert!(out.contains("/status"));
        assert!(out.contains("/init"));
        assert!(out.contains("/memory"));
    }

    #[test]
    fn slash_cost_no_data_yet() {
        let fs = MockFs::new();
        let out = handle_slash_command("/cost", "", 0, 0, "claude-sonnet-4-6", Path::new("/tmp"), &fs);
        assert!(out.contains("no usage data"), "got: {out}");
    }

    #[test]
    fn slash_cost_shows_percentage_and_estimated_cost() {
        let fs = MockFs::new();
        let out = handle_slash_command("/cost", "", 50_000, 200_000, "claude-sonnet-4-6", Path::new("/tmp"), &fs);
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
        assert_eq!(estimate_cost(0, "claude-sonnet-4-6"), "0.0000");
    }

    #[test]
    fn estimate_cost_1m_tokens_sonnet_is_6_dollars() {
        assert_eq!(estimate_cost(1_000_000, "claude-sonnet-4-6"), "6.00");
    }

    #[test]
    fn estimate_cost_uses_model_argument() {
        assert_eq!(estimate_cost(1_000_000, "claude-opus-4-7"), "28.50");
    }

    #[test]
    fn estimate_cost_small_amount_shows_four_decimals() {
        let cost = estimate_cost(1_000, "claude-sonnet-4-6");
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
    fn slash_unknown_suggests_help() {
        let fs = MockFs::new();
        let out = handle_slash_command("/nope", "", 0, 0, "claude-sonnet-4-6", Path::new("/tmp"), &fs);
        assert!(out.contains("unknown command") && out.contains("/help"), "got: {out}");
    }

    #[test]
    fn slash_model_without_arg_shows_registry_hint() {
        let fs = MockFs::new();
        let out = handle_slash_command("/model", "", 0, 0, "claude-sonnet-4-6", Path::new("/tmp"), &fs);
        assert!(out.contains("registry listing"), "got: {out}");
    }

    #[test]
    fn slash_model_with_arg_saves_and_confirms() {
        let fs = MockFs::new();
        let out = handle_slash_command("/model", "claude-opus-4-7", 0, 0, "claude-sonnet-4-6", Path::new("/tmp"), &fs);
        assert!(out.contains("claude-opus-4-7"), "got: {out}");
        assert!(out.contains("cost estimates updated"), "got: {out}");
    }

    #[test]
    fn slash_model_updates_cost_estimates() {
        let fs = MockFs::new();
        handle_slash_command("/model", "claude-haiku-4-5", 0, 0, "claude-sonnet-4-6", Path::new("/tmp"), &fs);
        let cost_out = handle_slash_command("/cost", "", 1_000_000, 2_000_000, "claude-haiku-4-5", Path::new("/tmp"), &fs);
        // haiku rate is 1.6 $/Mtok → 1M tokens ≈ $1.60
        assert!(cost_out.contains("1.60") || cost_out.contains("1.6"), "got: {cost_out}");
    }

    #[test]
    fn slash_cost_at_full_context_shows_100_percent() {
        let fs = MockFs::new();
        let out = handle_slash_command("/cost", "", 200_000, 200_000, "claude-sonnet-4-6", Path::new("/tmp"), &fs);
        assert!(out.contains("100%"), "got: {out}");
    }

    #[test]
    fn slash_cost_rounds_down() {
        let fs = MockFs::new();
        let out = handle_slash_command("/cost", "", 1, 3, "claude-sonnet-4-6", Path::new("/tmp"), &fs);
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

    // ── strip_code_fence ──────────────────────────────────────────────────────

    #[test]
    fn strip_code_fence_plain_fence() {
        assert_eq!(strip_code_fence("```\n# Title\n```"), "# Title");
    }

    #[test]
    fn strip_code_fence_markdown_lang() {
        assert_eq!(strip_code_fence("```markdown\n# Title\n```"), "# Title");
    }

    #[test]
    fn strip_code_fence_md_lang() {
        assert_eq!(strip_code_fence("```md\n# Title\n```"), "# Title");
    }

    #[test]
    fn strip_code_fence_no_fence_passthrough() {
        assert_eq!(strip_code_fence("# Title\nsome text"), "# Title\nsome text");
    }

    #[test]
    fn strip_code_fence_trims_trailing_whitespace() {
        let input = "```\n# Title\n\n```";
        let out = strip_code_fence(input);
        assert_eq!(out, "# Title", "got: {out:?}");
    }

    #[test]
    fn strip_code_fence_empty_string() {
        assert_eq!(strip_code_fence(""), "");
    }

    // ── find_git_root ─────────────────────────────────────────────────────────

    #[test]
    fn find_git_root_returns_none_when_no_git_dir() {
        let dir = std::env::temp_dir().join("trogon_no_git");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        assert!(find_git_root(&dir).is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn find_git_root_finds_direct_git_dir() {
        let dir = std::env::temp_dir().join("trogon_has_git");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        let root = find_git_root(&dir);
        assert_eq!(root, Some(dir.clone()));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn find_git_root_traverses_up_to_git_dir() {
        let base = std::env::temp_dir().join("trogon_git_traverse");
        let child = base.join("a").join("b").join("c");
        std::fs::remove_dir_all(&base).ok();
        std::fs::create_dir_all(&child).unwrap();
        std::fs::create_dir_all(base.join(".git")).unwrap();
        let root = find_git_root(&child);
        assert_eq!(root, Some(base.clone()));
        std::fs::remove_dir_all(&base).ok();
    }

    // ── detect_languages ──────────────────────────────────────────────────────

    #[test]
    fn detect_languages_empty_dir_returns_none() {
        let dir = std::env::temp_dir().join("trogon_langs_empty");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        assert!(detect_languages(&dir).is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn detect_languages_cargo_toml_detected_as_rust() {
        let dir = std::env::temp_dir().join("trogon_langs_rust");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Cargo.toml"), "").unwrap();
        let langs = detect_languages(&dir);
        assert!(langs.contains(&"Rust"), "got: {langs:?}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn detect_languages_multiple_indicators() {
        let dir = std::env::temp_dir().join("trogon_langs_multi");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Cargo.toml"), "").unwrap();
        std::fs::write(dir.join("package.json"), "{}").unwrap();
        let langs = detect_languages(&dir);
        assert!(langs.contains(&"Rust"), "rust missing: {langs:?}");
        assert!(langs.contains(&"JavaScript/TypeScript"), "js missing: {langs:?}");
        std::fs::remove_dir_all(&dir).ok();
    }

    // ── build_init_prompt ─────────────────────────────────────────────────────

    #[test]
    fn build_init_prompt_contains_required_sections() {
        let dir = std::env::temp_dir().join("trogon_init_prompt");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Cargo.toml"), "").unwrap();

        let fs = MockFs::new();
        let prompt = build_init_prompt(&dir, &fs);

        assert!(prompt.contains("TROGON.md"), "missing TROGON.md: {prompt}");
        assert!(prompt.contains("Overview"), "missing Overview: {prompt}");
        assert!(prompt.contains("Architecture"), "missing Architecture: {prompt}");
        assert!(prompt.contains("Development"), "missing Development: {prompt}");
        assert!(prompt.contains("Notes"), "missing Notes: {prompt}");
        assert!(prompt.contains("Rust"), "missing detected language: {prompt}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn build_init_prompt_includes_readme_when_present() {
        let dir = std::env::temp_dir().join("trogon_init_readme");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();

        let fs = MockFs::new();
        let readme_path = dir.join("README.md");
        fs.add_file(readme_path.to_str().unwrap(), "My special readme content.");

        let prompt = build_init_prompt(&dir, &fs);
        assert!(prompt.contains("My special readme content."), "readme not included: {prompt}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn build_init_prompt_project_name_from_dir() {
        let dir = std::env::temp_dir().join("my_cool_project");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();

        let fs = MockFs::new();
        let prompt = build_init_prompt(&dir, &fs);
        assert!(prompt.contains("my_cool_project"), "project name missing: {prompt}");
        std::fs::remove_dir_all(&dir).ok();
    }

    // ── apply_model_switch ────────────────────────────────────────────────────

    use crate::cross_runner::mock::MockRunnerSwitcher;

    #[tokio::test]
    async fn model_switch_same_runner_sets_same_runner_flag() {
        let mut switcher = MockRunnerSwitcher::same_runner("acp", "sess-1");
        let outcome = apply_model_switch(&mut switcher, "acp", "sess-1", "claude-opus-4-7", "/ws")
            .await
            .unwrap();
        assert!(outcome.same_runner);
        assert_eq!(outcome.new_prefix, "acp");
        assert_eq!(outcome.new_session_id, "sess-1");
    }

    #[tokio::test]
    async fn model_switch_cross_runner_returns_new_prefix_and_session() {
        let mut switcher = MockRunnerSwitcher::cross_runner("acp.xai", "new-sess-99");
        let outcome = apply_model_switch(&mut switcher, "acp", "old-sess", "grok-3", "/ws")
            .await
            .unwrap();
        assert!(!outcome.same_runner);
        assert_eq!(outcome.new_prefix, "acp.xai");
        assert_eq!(outcome.new_session_id, "new-sess-99");
    }

    #[tokio::test]
    async fn model_switch_error_propagates() {
        let mut switcher = MockRunnerSwitcher::error("no runner found for model: unknown");
        let err = apply_model_switch(&mut switcher, "acp", "sess-1", "unknown", "/ws")
            .await
            .unwrap_err();
        assert!(err.contains("no runner found for model: unknown"), "got: {err}");
    }

    #[tokio::test]
    async fn model_switch_cross_runner_different_from_current_prefix() {
        let mut switcher = MockRunnerSwitcher::cross_runner("acp.openrouter", "s-42");
        let outcome = apply_model_switch(&mut switcher, "acp", "s-old", "gpt-4o", "/workspace")
            .await
            .unwrap();
        assert!(!outcome.same_runner, "cross-runner must not be same_runner");
        assert_ne!(outcome.new_prefix, "acp");
    }

    // ── resolve_model_alias ───────────────────────────────────────────────────

    #[test]
    fn resolve_model_alias_claude_shortcuts() {
        assert_eq!(resolve_model_alias("sonnet"), "claude-sonnet-4-6");
        assert_eq!(resolve_model_alias("opus"), "claude-opus-4-7");
        assert_eq!(resolve_model_alias("haiku"), "claude-haiku-4-5-20251001");
    }

    #[test]
    fn resolve_model_alias_grok_shortcuts() {
        assert_eq!(resolve_model_alias("grok"), "grok-3");
        assert_eq!(resolve_model_alias("grok-mini"), "grok-3-mini");
    }

    #[test]
    fn resolve_model_alias_codex_and_openrouter_defaults() {
        unsafe {
            std::env::remove_var("OPENROUTER_DEFAULT_MODEL");
            std::env::remove_var("CODEX_DEFAULT_MODEL");
        }
        assert_eq!(resolve_model_alias("openrouter"), "anthropic/claude-sonnet-4");
        assert_eq!(resolve_model_alias("codex"), "o4-mini");
    }

    #[test]
    fn resolve_model_alias_env_overrides() {
        unsafe {
            std::env::set_var("OPENROUTER_DEFAULT_MODEL", "openai/gpt-4o");
            std::env::set_var("CODEX_DEFAULT_MODEL", "o3");
        }
        assert_eq!(resolve_model_alias("openrouter"), "openai/gpt-4o");
        assert_eq!(resolve_model_alias("codex"), "o3");
        unsafe {
            std::env::remove_var("OPENROUTER_DEFAULT_MODEL");
            std::env::remove_var("CODEX_DEFAULT_MODEL");
        }
    }

    #[test]
    fn resolve_model_alias_passthrough() {
        assert_eq!(resolve_model_alias("o3"), "o3");
        assert_eq!(resolve_model_alias("gpt-4o"), "gpt-4o");
        assert_eq!(resolve_model_alias("custom/model-id"), "custom/model-id");
    }

    #[test]
    fn sync_cwd_from_change_directory_tool() {
        let mut cwd = PathBuf::from("/tmp");
        assert!(sync_cwd_from_tool(
            "change_directory",
            "Working directory is now /home/jorge/straw-hat/trogonai",
            &mut cwd,
        ));
        assert_eq!(cwd, PathBuf::from("/home/jorge/straw-hat/trogonai"));
    }

    #[test]
    fn sync_cwd_from_bash_cd_tool() {
        let mut cwd = PathBuf::from("/tmp");
        assert!(sync_cwd_from_tool(
            "bash",
            "Working directory is now /var/log",
            &mut cwd,
        ));
        assert_eq!(cwd, PathBuf::from("/var/log"));
    }

    #[test]
    fn sync_cwd_ignores_unrelated_tools() {
        let mut cwd = PathBuf::from("/tmp");
        assert!(!sync_cwd_from_tool("list_dir", "foo\nbar", &mut cwd));
        assert_eq!(cwd, PathBuf::from("/tmp"));
    }
}
