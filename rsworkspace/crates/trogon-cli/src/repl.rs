use crate::session::{StreamEvent, TrogonSession};
use async_nats::Client;
use rustyline::DefaultEditor;
use rustyline::error::ReadlineError;
use std::io::Write;
use std::path::PathBuf;

const HISTORY_PATH: &str = "~/.local/share/trogon/history";

pub async fn run(nats: Client, prefix: &str, cwd: PathBuf) -> anyhow::Result<()> {
    let session = TrogonSession::new(nats, prefix, cwd).await?;

    let history_path = expand_tilde(HISTORY_PATH);
    if let Some(dir) = history_path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }

    let mut rl = DefaultEditor::new()?;
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

                match session.prompt(&line).await {
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
