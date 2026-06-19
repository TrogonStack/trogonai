use acp_nats::{AcpPrefix, Config, NatsAuth, NatsConfig};
use clap::{Parser, Subcommand};
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::time::Duration;
use trogon_cli::Session as _;
use trogon_cli::{
    CrossRunnerSwitcher, McpManager, NatsSessionFactory, OutputFormat, PrintOptions, RealFs, SessionEntry,
    SessionFactory, SessionIndex, SessionInit, connect_or_start_nats, persist_session, repl::resolve_model_alias,
    session::TrogonSession, should_use_ansi,
};

#[derive(Subcommand)]
enum Command {
    /// Start the local dev stack (NATS, wasm runtime, runners)
    Dev,
    /// Run health checks on the Trogon stack
    Doctor,
    /// Manage sessions on the current runner
    Sessions {
        #[command(subcommand)]
        action: Option<SessionsAction>,
    },
}

#[derive(Subcommand)]
enum SessionsAction {
    /// List sessions on the current runner (the non-interactive form of /sessions)
    List,
}

#[derive(Parser)]
#[command(name = "trogon", about = "Trogon AI CLI")]
struct Args {
    #[command(subcommand)]
    command: Option<Command>,
    /// NATS server URL (overrides TROGON_NATS_URL)
    #[arg(
        long,
        env = "TROGON_NATS_URL",
        default_value = "nats://localhost:4222",
        global = true
    )]
    nats_url: String,

    /// ACP prefix (overrides ACP_PREFIX)
    #[arg(long, env = "ACP_PREFIX", default_value = "acp.claude", global = true)]
    prefix: String,

    /// Non-interactive mode: send PROMPT and print result to stdout.
    /// Omit the value to read the prompt from stdin instead:
    ///   trogon --print "explain this" < error.log
    ///   echo "what is 2+2?" | trogon --print
    #[arg(short = 'p', long, num_args = 0..=1, default_missing_value = "-")]
    print: Option<String>,

    /// Output format for non-interactive mode: "text" (default), "json", or
    /// "stream-json". json emits a single line {"text":...,"stop_reason":...};
    /// stream-json emits one JSON event per line (NDJSON), ending with a result.
    #[arg(long, default_value = "text")]
    output_format: String,

    /// Emit NDJSON tool lines during --print: {"type":"tool","name":"...","output":"..."}
    #[arg(long)]
    print_tools: bool,

    /// Set bypassPermissions before the prompt (skips permission gates — use with care)
    #[arg(long)]
    dangerously_skip_permissions: bool,

    /// Start the interactive session in plan mode (writes are denied; plan first)
    #[arg(long, conflicts_with = "dangerously_skip_permissions")]
    plan: bool,

    /// Enable verbose (debug-level) logging for the CLI. Respects an explicit RUST_LOG.
    #[arg(short = 'v', long)]
    verbose: bool,

    /// Override the agent's built-in system prompt entirely (replaces the default identity)
    #[arg(long)]
    system_prompt: Option<String>,

    /// Append extra text to the agent's system prompt (kept alongside the default)
    #[arg(long)]
    append_system_prompt: Option<String>,

    /// Grant the agent an additional working directory (repeatable)
    #[arg(long, value_name = "PATH")]
    add_dir: Vec<PathBuf>,

    /// Give the interactive session a human-readable name (shown in /status, recorded in the index)
    #[arg(long, value_name = "NAME")]
    name: Option<String>,

    /// Model id for --print (resolved via aliases; uses --prefix runner)
    #[arg(long)]
    model: Option<String>,

    /// Run health checks on NATS, registry, runners, and compactor
    #[arg(long)]
    doctor: bool,

    /// Buffer assistant text until a tool boundary instead of streaming it live
    #[arg(long)]
    no_stream: bool,

    /// Force plain-text output in --print (no ANSI markdown rendering)
    #[arg(long)]
    plain: bool,

    /// MCP config file(s) for --print. Repeatable; each value may be comma-separated.
    #[arg(long = "mcp-config", value_name = "PATH")]
    mcp_config: Vec<PathBuf>,

    /// Use only --mcp-config files in --print (ignore ~/.config/trogon/mcp.json and .mcp.json)
    #[arg(long = "strict-mcp-config")]
    strict_mcp_config: bool,

    /// Resume the last session for this project (from ~/.local/share/trogon/sessions.json)
    #[arg(
        long = "continue-session",
        short = 'c',
        short_alias = 'r',
        alias = "continue",
        visible_alias = "resume",
        conflicts_with = "session_id"
    )]
    continue_session: bool,

    /// Attach to a specific session id (uses --prefix runner)
    #[arg(long, conflicts_with = "continue_session")]
    session_id: Option<String>,
}

/// `trogon sessions list`: query the runner for its sessions and print a table,
/// merging the local session index for model + name (mirrors the REPL's
/// /sessions). Uses an attach-only handle — no new session is created.
async fn run_sessions_list(nats_url: &str, prefix: &str) -> anyhow::Result<()> {
    let (nats, _nats_server) = connect_or_start_nats(nats_url, Duration::from_secs(3)).await?;
    let factory = NatsSessionFactory::new(nats);
    // list_sessions targets a runner-global subject and ignores the session id.
    let session = factory.attach_session(prefix, "sessions-list".to_string());
    let list = session.list_sessions().await?;

    if list.is_empty() {
        println!("no sessions on runner {prefix}");
        return Ok(());
    }

    let index = SessionIndex::load(&RealFs);
    let project = std::env::current_dir()?;
    let project = project.canonicalize().unwrap_or(project);

    println!("{:<36}  {:<20}  name / cwd", "session_id", "updated");
    for s in list {
        let updated = s.updated_at.as_deref().unwrap_or("-");
        let entry = index
            .get_for_prefix(&project, prefix)
            .filter(|e| e.session_id == s.session_id);
        let model = entry.map(|e| e.model.as_str()).unwrap_or("-");
        let label = entry
            .and_then(|e| e.name.as_deref())
            .or_else(|| s.title.as_deref().filter(|t| !t.is_empty()))
            .unwrap_or(&s.cwd);
        println!("{:<36}  {:<20}  {label}  [model: {model}]", s.session_id, updated);
    }
    Ok(())
}

fn trogon_dev_script() -> anyhow::Result<PathBuf> {
    if let Ok(path) = std::env::var("TROGON_DEV_SCRIPT") {
        return Ok(PathBuf::from(path));
    }
    let exe = std::env::current_exe()?;
    let release_dir = exe
        .parent()
        .ok_or_else(|| anyhow::anyhow!("could not resolve trogon binary directory"))?;
    let script = release_dir.join("../../scripts/trogon-dev.sh");
    script
        .canonicalize()
        .map_err(|_| anyhow::anyhow!("trogon dev script not found at {}", script.display()))
}

/// Initialize the CLI tracing subscriber.
///
/// Without `--verbose` the CLI stays quiet unless the user opts in via `RUST_LOG`
/// (default filter: `warn`). With `--verbose` the default filter is raised to
/// `debug`, while an explicit `RUST_LOG` always wins so power users keep full
/// control. Logs go to stderr so they never pollute `--print` stdout output.
fn init_tracing(verbose: bool) {
    use tracing_subscriber::{EnvFilter, fmt};

    let default_level = if verbose { "debug" } else { "warn" };
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_level));

    // `try_init` so a double-init (e.g. in tests) is a no-op rather than a panic.
    let _ = fmt().with_env_filter(filter).with_writer(std::io::stderr).try_init();
}

/// Build the per-session init metadata from the CLI flags.
///
/// `--add-dir` paths are canonicalised to absolute form; entries that do not
/// resolve to an existing directory are warned about and skipped (matching the
/// CLI's warn-and-continue style elsewhere) so a typo never aborts startup.
fn build_session_init(
    system_prompt: Option<String>,
    append_system_prompt: Option<String>,
    add_dir: &[PathBuf],
    settings: &trogon_cli::Settings,
) -> SessionInit {
    let mut additional_roots = Vec::new();
    for dir in add_dir {
        match dir.canonicalize() {
            Ok(p) if p.is_dir() => additional_roots.push(p.to_string_lossy().into_owned()),
            Ok(p) => eprintln!("warning: --add-dir {} is not a directory — skipping", p.display()),
            Err(e) => eprintln!(
                "warning: --add-dir {} could not be resolved ({e}) — skipping",
                dir.display()
            ),
        }
    }
    // Runner-side hooks (PreToolUse/PostToolUse/PostToolBatch/SubagentStop) fire
    // inside the agent loop; CLI-side events (SessionStart/PreCompact/Stop/
    // UserPromptSubmit/Notification) run in the REPL, not here.
    let tool_hooks = if settings.hooks.pre_tool_use.is_empty()
        && settings.hooks.post_tool_use.is_empty()
        && settings.hooks.post_tool_batch.is_empty()
        && settings.hooks.subagent_stop.is_empty()
    {
        None
    } else {
        Some(trogon_runner_tools::HooksConfig {
            pre_tool_use: settings.hooks.pre_tool_use.clone(),
            post_tool_use: settings.hooks.post_tool_use.clone(),
            post_tool_batch: settings.hooks.post_tool_batch.clone(),
            subagent_stop: settings.hooks.subagent_stop.clone(),
            ..Default::default()
        })
    };
    SessionInit {
        system_prompt_override: system_prompt,
        append_system_prompt,
        additional_roots,
        // permissions.additionalDirectories from settings.json.
        additional_read_dirs: settings.permissions.additional_directories.clone(),
        // permissions.allow/deny translated to trogon rule-text.
        permission_rules: settings.permission_rules_text(),
        tool_hooks,
        env: settings.env.clone(),
    }
}

fn run_dev_stack() -> anyhow::Result<()> {
    let script = trogon_dev_script()?;
    let status = std::process::Command::new("bash").arg(script).status()?;
    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}

/// The model to use when none is forced on the CLI: an explicit `--model` always
/// wins; otherwise fall back to `settings.json`'s `model`. `None` means "let the
/// runner use its default".
fn effective_model(arg: Option<&str>, settings_model: Option<&str>) -> Option<String> {
    arg.map(str::to_string).or_else(|| settings_model.map(str::to_string))
}

fn resolve_resume(continue_session: bool, session_id: Option<&str>, prefix: &str, cwd: &Path) -> Option<SessionEntry> {
    if continue_session {
        let index = SessionIndex::load(&RealFs);
        let canon = match cwd.canonicalize() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("warning: current directory could not be resolved ({e}), session lookup may fail");
                cwd.to_path_buf()
            }
        };
        index.get_last(&canon).cloned()
    } else {
        session_id.map(|id| SessionEntry {
            prefix: prefix.to_string(),
            session_id: id.to_string(),
            model: String::new(),
            updated_at: String::new(),
            name: None,
        })
    }
}

async fn start_print_session(
    nats: async_nats::Client,
    mcp: &mut McpManager,
    prefix: &str,
    cwd: PathBuf,
    init: &SessionInit,
) -> anyhow::Result<TrogonSession<async_nats::Client>> {
    let mcp_servers = mcp.spawn_pending(&RealFs).await;
    let session = TrogonSession::new_with_init(nats, prefix, cwd, mcp_servers, init).await?;
    mcp.commit_pending(session.session_id());
    Ok(session)
}

async fn resume_print_session(
    nats: async_nats::Client,
    mcp: &mut McpManager,
    prefix: &str,
    session_id: &str,
    cwd: &Path,
) -> anyhow::Result<TrogonSession<async_nats::Client>> {
    let mcp_servers = mcp.spawn_pending(&RealFs).await;
    let session = TrogonSession::from_existing(nats, prefix, session_id.to_string());
    session.load_session(session_id, cwd, mcp_servers).await?;
    mcp.commit_pending(session_id);
    Ok(session)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    trogon_cli::env_local::load_env_local();
    let args = Args::parse();

    init_tracing(args.verbose);

    if matches!(args.command, Some(Command::Dev)) {
        return run_dev_stack();
    }

    if args.doctor || matches!(args.command, Some(Command::Doctor)) {
        return trogon_cli::doctor::run(&args.nats_url).await;
    }

    // `trogon sessions [list]` — non-interactive equivalent of the REPL's /sessions.
    if let Some(Command::Sessions { action }) = &args.command {
        match action {
            None | Some(SessionsAction::List) => {
                return run_sessions_list(&args.nats_url, &args.prefix).await;
            }
        }
    }

    let cwd = std::env::current_dir()?;

    // Layered settings.json (user → project → project-local).
    let settings = trogon_cli::Settings::load(&RealFs, &cwd);

    // permissions.defaultMode seeds the startup mode when not overridden by an
    // explicit flag or TROGON_MODE. (Startup-only; set before any threads spawn.)
    if std::env::var_os("TROGON_MODE").is_none()
        && let Some(mode) = settings.permissions.default_mode.as_deref()
    {
        // SAFETY: called once at startup before any worker threads are spawned.
        unsafe { std::env::set_var("TROGON_MODE", mode) };
    }

    // cleanupPeriodDays: prune stale sessions from the index on startup.
    if let Some(days) = settings.cleanup_period_days {
        let mut index = SessionIndex::load(&RealFs);
        let removed = index.prune_older_than(days);
        if removed > 0 {
            if let Err(e) = index.save(&RealFs) {
                eprintln!("warning: could not save pruned session index: {e}");
            } else {
                eprintln!("cleaned up {removed} session(s) older than {days} days");
            }
        }
    }

    let session_init = build_session_init(
        args.system_prompt.clone(),
        args.append_system_prompt.clone(),
        &args.add_dir,
        &settings,
    );

    // MED-40: keep `nats_server` in a named binding (not `_child`) so Drop is
    // visible and we can call `drop()` explicitly before `process::exit`.
    let (nats, nats_server) = connect_or_start_nats(&args.nats_url, Duration::from_secs(3)).await?;

    if let Some(prompt_arg) = &args.print {
        let prompt = if prompt_arg == "-" {
            use std::io::Read as _;
            let mut buf = String::new();
            std::io::stdin().read_to_string(&mut buf)?;
            buf.trim().to_string()
        } else {
            prompt_arg.clone()
        };
        if prompt.is_empty() {
            eprintln!("error: prompt is empty — pass a string or pipe text to stdin");
            // MED-40: drop KillOnDrop guard so the autostarted NATS server is killed.
            drop(nats_server);
            std::process::exit(1);
        }
        let format = match args.output_format.as_str() {
            "json" => OutputFormat::Json,
            "stream-json" => OutputFormat::StreamJson,
            _ => OutputFormat::Text,
        };
        let options = PrintOptions {
            print_tools: args.print_tools,
            use_ansi: should_use_ansi(args.plain, std::io::stdout().is_terminal()),
        };
        let chosen_model = effective_model(args.model.as_deref(), settings.model.as_deref());
        let resume = resolve_resume(args.continue_session, args.session_id.as_deref(), &args.prefix, &cwd);
        if args.continue_session && resume.is_none() {
            eprintln!("warning: no saved session for this project — starting fresh");
        }
        let resumed = resume.is_some();

        let mut mcp_manager = McpManager::load_for_cli(&RealFs, Some(&cwd), &args.mcp_config, args.strict_mcp_config);

        // Resolve the target runner prefix: if a model is requested, look it up in
        // the registry so cross-runner models (e.g. `--model haiku` while prefix is
        // `acp.grok`) route to the correct runner instead of failing with "unknown model".
        let mut target_prefix = if let Some(model) = &chosen_model {
            let model_id = resolve_model_alias(model);
            let js = async_nats::jetstream::new(nats.clone());
            let reg_store = trogon_registry::ReprovisioningStore::new(js)
                .await
                .map_err(|e| anyhow::anyhow!("registry provisioning failed: {e}"))?;
            let registry = trogon_registry::Registry::new(reg_store);
            match registry.find_by_model(&model_id).await {
                Ok(Some(cap)) => cap.metadata["acp_prefix"].as_str().unwrap_or(&args.prefix).to_string(),
                _ => args.prefix.clone(),
            }
        } else {
            args.prefix.clone()
        };
        if let Some(entry) = &resume {
            target_prefix = entry.prefix.clone();
        }

        let nats_for_perm = nats.clone();
        let session = if let Some(entry) = resume {
            match resume_print_session(nats, &mut mcp_manager, &target_prefix, &entry.session_id, &cwd).await {
                Ok(s) => {
                    eprintln!("resumed session {} on {target_prefix}", s.session_id());
                    s
                }
                Err(e) => {
                    eprintln!("warning: could not resume {}: {e} — starting fresh", entry.session_id);
                    start_print_session(
                        nats_for_perm.clone(),
                        &mut mcp_manager,
                        &target_prefix,
                        cwd.clone(),
                        &session_init,
                    )
                    .await?
                }
            }
        } else {
            start_print_session(nats, &mut mcp_manager, &target_prefix, cwd.clone(), &session_init).await?
        };
        // Permission handling in non-interactive mode: `--dangerously-skip-permissions`
        // sets `bypassPermissions` (auto-allow, no prompts). Otherwise — since there is
        // no human to answer a prompt — start an auto-DENY responder so approval-gated
        // tools (bash, MCP tools like `spawn_agent`) are denied and the turn completes
        // instead of hanging forever waiting for input.
        let mut deny_task = None;
        if args.dangerously_skip_permissions {
            if let Err(e) = session.set_mode("bypassPermissions").await {
                eprintln!("warning: could not set bypassPermissions: {e}");
            }
        } else if let Some(task) =
            trogon_cli::print::spawn_auto_deny_permissions(nats_for_perm, &target_prefix, session.session_id()).await
        {
            deny_task = Some(task);
        } else {
            mcp_manager.shutdown_session(session.session_id()).await;
            drop(nats_server);
            std::process::exit(1);
        }
        if !resumed && let Some(model) = &chosen_model {
            let model_id = resolve_model_alias(model);
            if let Err(e) = session.set_model(&model_id).await {
                eprintln!("error: could not set model: {e}");
                mcp_manager.shutdown_session(session.session_id()).await;
                // MED-40: drop KillOnDrop guard before exit.
                drop(nats_server);
                std::process::exit(1);
            }
        }
        let session_id = session.session_id().to_string();
        let session_model = session.current_model();
        // RUN-2: codex is descoped to observational-only — warn on stderr (keeps
        // --print stdout clean) when the resolved runner routes to codex.
        trogon_cli::app::warn_if_codex_observational(&target_prefix);
        let code = trogon_cli::print::run(session, &prompt, format, options).await;
        let project_dir = cwd.canonicalize().unwrap_or_else(|_| cwd.clone());
        persist_session(
            &RealFs,
            &project_dir,
            &target_prefix,
            &session_id,
            &session_model,
            args.name.as_deref(),
        );
        mcp_manager.shutdown_session(&session_id).await;
        if let Some(task) = deny_task {
            task.abort();
        }
        // MED-40: explicitly drop the KillOnDrop guard so the auto-started NATS
        // server process is killed before process::exit bypasses normal Drop.
        drop(nats_server);
        std::process::exit(code as i32);
    } else {
        let acp_prefix = AcpPrefix::new(&args.prefix).map_err(|e| anyhow::anyhow!("invalid ACP prefix: {e}"))?;
        let nats_config = NatsConfig::new(vec![args.nats_url.clone()], NatsAuth::None);
        let acp_config = Config::new(acp_prefix, nats_config);
        let js = async_nats::jetstream::new(nats.clone());
        // MED-36: use a store that re-provisions the (in-memory) AGENT_REGISTRY
        // bucket on failure, so /model and /status keep working after a NATS server
        // restart instead of erroring until the CLI is restarted.
        let reg_store = trogon_registry::ReprovisioningStore::new(js)
            .await
            .map_err(|e| anyhow::anyhow!("registry provisioning failed: {e}"))?;
        let registry = trogon_registry::Registry::new(reg_store);
        let registry_for_repl = registry.clone();
        let switcher = CrossRunnerSwitcher::new(nats.clone(), acp_config.clone(), registry);
        let factory = NatsSessionFactory::new(nats.clone());

        let resume = resolve_resume(args.continue_session, args.session_id.as_deref(), &args.prefix, &cwd);

        if args.continue_session && resume.is_none() {
            eprintln!("warning: no saved session for this project — starting fresh");
        }

        let repl_default_model =
            effective_model(args.model.as_deref(), settings.model.as_deref()).map(|m| resolve_model_alias(&m));

        trogon_cli::runtime::run_interactive(
            factory,
            &args.prefix,
            cwd,
            RealFs,
            switcher,
            registry_for_repl,
            nats,
            acp_config,
            args.nats_url,
            !args.no_stream,
            repl_default_model,
            resume,
            args.dangerously_skip_permissions,
            args.plan,
            session_init,
            args.name,
        )
        .await?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Args, Command, SessionsAction, build_session_init, effective_model, resolve_resume};
    use clap::Parser;
    use std::path::{Path, PathBuf};
    use trogon_cli::{Settings, should_use_ansi};

    #[test]
    fn no_stream_defaults_off_and_flag_enables() {
        let args = Args::try_parse_from(["trogon"]).unwrap();
        assert!(!args.no_stream);

        let args = Args::try_parse_from(["trogon", "--no-stream"]).unwrap();
        assert!(args.no_stream);
    }

    #[test]
    fn sessions_list_subcommand_parses() {
        let args = Args::try_parse_from(["trogon", "sessions", "list"]).unwrap();
        assert!(matches!(
            args.command,
            Some(Command::Sessions {
                action: Some(SessionsAction::List)
            })
        ));
    }

    #[test]
    fn bare_sessions_subcommand_defaults_to_no_action() {
        // `trogon sessions` (no action) is accepted and treated as list.
        let args = Args::try_parse_from(["trogon", "sessions"]).unwrap();
        assert!(matches!(args.command, Some(Command::Sessions { action: None })));
    }

    #[test]
    fn prompts_flow_into_init_without_add_dir() {
        let init = build_session_init(Some("over".into()), Some("app".into()), &[], &Settings::default());
        assert_eq!(init.system_prompt_override.as_deref(), Some("over"));
        assert_eq!(init.append_system_prompt.as_deref(), Some("app"));
        assert!(init.additional_roots.is_empty());
    }

    #[test]
    fn existing_dir_is_canonicalised_into_roots() {
        // The current directory always exists and is a directory.
        let cwd = std::env::current_dir().unwrap();
        let init = build_session_init(None, None, &[cwd.clone()], &Settings::default());
        let expected = cwd.canonicalize().unwrap().to_string_lossy().into_owned();
        assert_eq!(init.additional_roots, vec![expected]);
    }

    #[test]
    fn missing_dir_is_skipped_not_fatal() {
        let init = build_session_init(None, None, &[PathBuf::from("/no/such/dir/xyzzy")], &Settings::default());
        assert!(init.additional_roots.is_empty());
    }

    #[test]
    fn settings_permissions_flow_into_init() {
        let mut settings = Settings::default();
        settings.permissions.additional_directories = vec!["/shared".into()];
        settings.permissions.allow = vec!["Bash(cargo test:*)".into()];
        let init = build_session_init(None, None, &[], &settings);
        assert_eq!(init.additional_read_dirs, vec!["/shared".to_string()]);
        assert!(init.permission_rules.unwrap().contains("allow_commands: cargo test"));
    }

    #[test]
    fn effective_model_precedence() {
        assert_eq!(effective_model(Some("a"), Some("b")), Some("a".into()));
        assert_eq!(effective_model(None, Some("b")), Some("b".into()));
        assert_eq!(effective_model(None, None), None);
    }

    #[test]
    fn continue_aliases_parse() {
        let cases = [
            ["trogon", "--continue"],
            ["trogon", "--continue-session"],
            ["trogon", "--resume"],
            ["trogon", "-c"],
            ["trogon", "-r"],
        ];
        for argv in cases {
            let args = Args::try_parse_from(argv).unwrap();
            assert!(args.continue_session, "expected continue flag for {argv:?}");
        }
    }

    #[test]
    fn plain_and_mcp_config_flags_parse() {
        let args = Args::try_parse_from([
            "trogon",
            "--plain",
            "--mcp-config",
            "a.json,b.json",
            "--mcp-config",
            "c.json",
            "--strict-mcp-config",
            "-p",
            "hi",
        ])
        .unwrap();
        assert!(args.plain);
        assert!(args.strict_mcp_config);
        assert_eq!(
            args.mcp_config,
            vec![PathBuf::from("a.json,b.json"), PathBuf::from("c.json")]
        );
    }

    #[test]
    fn resolve_resume_without_flags_is_none() {
        let cwd = std::env::current_dir().unwrap();
        assert!(resolve_resume(false, None, "acp.claude", &cwd).is_none());
    }

    #[test]
    fn resolve_resume_session_id_builds_entry() {
        let cwd = Path::new("/tmp/proj");
        let entry = resolve_resume(false, Some("sess-1"), "acp.claude", cwd).unwrap();
        assert_eq!(entry.session_id, "sess-1");
        assert_eq!(entry.prefix, "acp.claude");
    }

    #[test]
    fn print_plain_flag_forces_plain_even_on_tty() {
        assert!(!should_use_ansi(true, true));
    }
}
