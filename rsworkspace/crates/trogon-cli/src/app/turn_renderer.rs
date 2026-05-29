//! Maps `StreamEvent` → terminal output for one assistant turn.
//!
//! Visual style is the unified CLI design (assistant prefix, running-tool pill,
//! `── tokens ──` footer). Behavior keeps full functionality: streamed vs
//! buffered (markdown-rendered) text, tool output, and diffs. When a new
//! `StreamEvent` variant is added, add a match arm here.

use std::io::Write as _;

use agent_client_protocol::ToolCallStatus;

use crate::app::display::print_assistant_prefix;
use crate::session::StreamEvent;
use crate::terminal::{print_tool_output, reset_display};

/// Token counters updated during a turn.
#[derive(Debug, Clone, Copy, Default)]
pub struct TurnMetrics {
    pub used_tokens: u64,
    pub context_size: u64,
}

/// A tool finished with output the REPL may use to sync its cwd.
#[derive(Debug, Clone)]
pub struct CwdSync {
    pub tool_name: String,
    pub output: String,
}

/// Why the turn loop should stop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnStop {
    Done { reason: String },
    Error(String),
}

/// Renders streaming events for one assistant turn.
pub struct TurnRenderer {
    /// When true, text tokens print as they arrive; otherwise they are buffered
    /// and markdown-rendered when the turn (or a tool call) interrupts the text.
    stream: bool,
    response_buf: String,
    /// True once the bold `Trogon` prefix has been printed for the current text run.
    text_started: bool,
    tools_done: u32,
    running_tool: Option<String>,
    /// True while a `▸ … tools` pill is live on stderr (no trailing newline).
    pill_line: bool,
    stop: Option<TurnStop>,
}

impl TurnRenderer {
    pub fn new(stream: bool) -> Self {
        Self {
            stream,
            response_buf: String::new(),
            text_started: false,
            tools_done: 0,
            running_tool: None,
            pill_line: false,
            stop: None,
        }
    }

    pub fn take_stop(&mut self) -> Option<TurnStop> {
        self.stop.take()
    }

    pub fn is_stopped(&self) -> bool {
        self.stop.is_some()
    }

    pub fn on_ctrl_c(&mut self) {
        self.flush_pill();
        eprintln!("\n[cancelled]");
    }

    pub fn handle(&mut self, event: StreamEvent, metrics: &mut TurnMetrics) -> Option<CwdSync> {
        let mut cwd_sync = None;
        match event {
            StreamEvent::Text(text) => {
                self.flush_pill();
                if self.stream {
                    self.start_text();
                    print!("{text}");
                    let _ = std::io::stdout().flush();
                } else {
                    self.response_buf.push_str(&text);
                }
            }
            StreamEvent::Thinking => {}
            StreamEvent::ToolCall(name) => {
                self.flush_buffered_markdown();
                if self.text_started {
                    println!();
                    self.text_started = false;
                }
                self.running_tool = Some(name);
                self.render_tool_pill();
            }
            StreamEvent::Diff(diff) => {
                self.flush_pill();
                reset_display();
                eprintln!("{diff}");
            }
            StreamEvent::ToolFinished {
                name,
                output,
                exit_code,
                status,
            } => {
                self.tools_done += 1;
                self.running_tool = None;
                let failed = matches!(status, ToolCallStatus::Failed);
                let code_suffix = exit_code.map(|c| format!(" (exit {c})")).unwrap_or_default();
                let badge = if failed { " ✗" } else { "" };
                self.flush_pill();
                eprintln!(
                    "\x1b[2m  {} {}{code_suffix}{badge}\x1b[0m",
                    self.tools_done, name
                );
                if !output.is_empty() {
                    print_tool_output(&output);
                    cwd_sync = Some(CwdSync {
                        tool_name: name,
                        output,
                    });
                }
            }
            StreamEvent::Usage {
                used_tokens,
                context_size,
            } => {
                metrics.used_tokens = used_tokens;
                metrics.context_size = context_size;
            }
            StreamEvent::Error(msg) => {
                self.flush_pill();
                self.flush_buffered_markdown();
                eprintln!("\n\x1b[31merror: {msg}\x1b[0m");
                let _ = std::io::stdout().flush();
                self.stop = Some(TurnStop::Error(msg));
            }
            StreamEvent::Done(reason) => {
                self.flush_pill();
                self.flush_buffered_markdown();
                if self.text_started {
                    println!();
                    self.text_started = false;
                }
                if reason == "cancelled" {
                    eprintln!("\n[cancelled]");
                } else if reason == "maxTurnRequests" {
                    eprintln!(
                        "\n\x1b[33m[max tool rounds reached — try rephrasing or using \x1b[35m/compact\x1b[33m]\x1b[0m"
                    );
                } else if metrics.context_size > 0 {
                    let pct = metrics.used_tokens * 100 / metrics.context_size;
                    eprintln!(
                        "\x1b[90m── {}/{} tokens ({}%) ──\x1b[0m",
                        fmt_tokens(metrics.used_tokens),
                        fmt_tokens(metrics.context_size),
                        pct,
                    );
                    println!();
                } else {
                    println!();
                }
                let _ = std::io::stdout().flush();
                self.stop = Some(TurnStop::Done { reason });
            }
        }
        cwd_sync
    }

    /// Print the bold assistant prefix once per text run.
    fn start_text(&mut self) {
        if !self.text_started {
            print_assistant_prefix();
            self.text_started = true;
        }
    }

    /// In non-streaming mode, render and print any buffered text as markdown.
    fn flush_buffered_markdown(&mut self) {
        if !self.stream && !self.response_buf.is_empty() {
            self.start_text();
            print!("{}", crate::markdown::render(&self.response_buf));
            let _ = std::io::stdout().flush();
            self.response_buf.clear();
        }
    }

    /// Clear the live tool pill, if any, moving to a fresh line.
    fn flush_pill(&mut self) {
        if self.pill_line {
            eprint!("\r\x1b[2K\n");
            let _ = std::io::stderr().flush();
            self.pill_line = false;
        }
    }

    /// Draw/refresh the `▸ N tools · name running…` pill in place.
    fn render_tool_pill(&mut self) {
        let running = self
            .running_tool
            .as_deref()
            .map(|n| format!(" · {n} running…"))
            .unwrap_or_default();
        let total = self.tools_done + u32::from(self.running_tool.is_some());
        let line = if total == 0 {
            "▸ tools".to_string()
        } else {
            format!("▸ {total} tools{running}")
        };
        if self.pill_line {
            eprint!("\r\x1b[2K\x1b[2m{line}\x1b[0m");
        } else {
            eprint!("\n\x1b[2m{line}\x1b[0m");
            self.pill_line = true;
        }
        let _ = std::io::stderr().flush();
    }
}

fn fmt_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn done_signals_stop_with_reason() {
        let mut r = TurnRenderer::new(true);
        let mut m = TurnMetrics {
            used_tokens: 500,
            context_size: 10_000,
        };
        r.handle(StreamEvent::Done("end_turn".into()), &mut m);
        assert!(matches!(r.take_stop(), Some(TurnStop::Done { reason }) if reason == "end_turn"));
    }

    #[test]
    fn tool_finished_with_output_returns_cwd_sync() {
        let mut r = TurnRenderer::new(true);
        let mut m = TurnMetrics::default();
        let sync = r.handle(
            StreamEvent::ToolFinished {
                name: "bash".into(),
                output: "/tmp\n".into(),
                exit_code: Some(0),
                status: ToolCallStatus::Completed,
            },
            &mut m,
        );
        let sync = sync.expect("output present → cwd sync");
        assert_eq!(sync.tool_name, "bash");
        assert_eq!(sync.output, "/tmp\n");
    }

    #[test]
    fn error_signals_stop() {
        let mut r = TurnRenderer::new(true);
        let mut m = TurnMetrics::default();
        r.handle(StreamEvent::Error("boom".into()), &mut m);
        assert!(matches!(r.take_stop(), Some(TurnStop::Error(msg)) if msg == "boom"));
    }

    #[test]
    fn fmt_tokens_uses_compact_units() {
        assert_eq!(fmt_tokens(42), "42");
        assert_eq!(fmt_tokens(1_500), "1.5k");
        assert_eq!(fmt_tokens(2_000_000), "2.0M");
    }
}
