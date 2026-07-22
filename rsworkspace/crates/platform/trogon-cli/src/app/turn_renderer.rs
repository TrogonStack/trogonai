//! Maps `StreamEvent` → terminal output for one assistant turn.
//!
//! Visual style is the unified CLI design (assistant prefix, running-tool pill,
//! `── tokens ──` footer). Behavior keeps full functionality: streamed vs
//! buffered (markdown-rendered) text, tool output, and diffs. When a new
//! `StreamEvent` variant is added, add a match arm here.

use std::io::Write as _;

use agent_client_protocol::schema::v1::ToolCallStatus;

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
    /// Full assistant text for this turn (used by optional REPL transcript capture).
    assistant_text: String,
    /// True once the bold `Trogon` prefix has been printed for the current text run.
    text_started: bool,
    tools_done: u32,
    running_tool: Option<String>,
    /// True while a status line (spinner / tool pill) is live on stderr (no
    /// trailing newline) and must be cleared before printing real output.
    pill_line: bool,
    stop: Option<TurnStop>,
    /// Streaming mode: incomplete trailing line awaiting a newline before it can
    /// be markdown-rendered (we render whole lines as they complete).
    stream_line_buf: String,
    /// Streaming mode: whether we are inside a ``` fenced code block.
    stream_in_code: bool,
    /// When the turn started — drives the live elapsed-time counter.
    turn_start: std::time::Instant,
    /// True while the model is working with nothing else on screen (no tool
    /// running, no text streaming) — shows `Thinking… (Ns)`.
    thinking: bool,
}

impl TurnRenderer {
    pub fn new(stream: bool) -> Self {
        Self {
            stream,
            response_buf: String::new(),
            assistant_text: String::new(),
            text_started: false,
            tools_done: 0,
            running_tool: None,
            pill_line: false,
            stop: None,
            stream_line_buf: String::new(),
            stream_in_code: false,
            turn_start: std::time::Instant::now(),
            thinking: true,
        }
    }

    pub fn take_stop(&mut self) -> Option<TurnStop> {
        self.stop.take()
    }

    pub fn is_stopped(&self) -> bool {
        self.stop.is_some()
    }

    /// Accumulated assistant text for the current turn.
    pub fn assistant_text(&self) -> &str {
        &self.assistant_text
    }

    pub fn on_ctrl_c(&mut self) {
        self.flush_pill();
        // In-progress feedback only — the confirming "[cancelled]" is printed when
        // the runner's terminal Done("cancelled") arrives (see handle()). Asserting
        // "[cancelled]" here would claim the turn stopped before it is confirmed.
        eprintln!("\n[cancelling…]");
    }

    pub fn handle(&mut self, event: StreamEvent, metrics: &mut TurnMetrics) -> Option<CwdSync> {
        let mut cwd_sync = None;
        match event {
            StreamEvent::Text(text) => {
                self.assistant_text.push_str(&text);
                self.thinking = false;
                self.flush_pill();
                if self.stream {
                    self.start_text();
                    // Buffer until we have whole lines, then markdown-render each
                    // completed line (bold/italic/code/headers/fences). The trailing
                    // partial line stays buffered until its newline arrives or the
                    // turn ends (flushed via `flush_stream_line`).
                    self.stream_line_buf.push_str(&text);
                    while let Some(nl) = self.stream_line_buf.find('\n') {
                        let line: String = self.stream_line_buf[..nl].to_string();
                        self.stream_line_buf.drain(..=nl);
                        if let Some(rendered) = crate::markdown::render_line(&line, true, &mut self.stream_in_code) {
                            println!("{rendered}");
                        }
                    }
                    let _ = std::io::stdout().flush();
                } else {
                    self.response_buf.push_str(&text);
                }
            }
            StreamEvent::Thinking => {}
            StreamEvent::ToolCall(name) => {
                self.flush_stream_line();
                self.flush_buffered_markdown();
                if self.text_started {
                    println!();
                    self.text_started = false;
                }
                self.thinking = false;
                self.running_tool = Some(name);
                self.render_status_line();
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
                eprintln!("\x1b[2m  {} {}{code_suffix}{badge}\x1b[0m", self.tools_done, name);
                if !output.is_empty() {
                    print_tool_output(&output);
                    cwd_sync = Some(CwdSync {
                        tool_name: name,
                        output,
                    });
                }
                // Back to waiting on the model for the next step.
                self.thinking = true;
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
                self.flush_stream_line();
                self.flush_buffered_markdown();
                eprintln!("\n\x1b[31merror: {msg}\x1b[0m");
                let _ = std::io::stdout().flush();
                self.stop = Some(TurnStop::Error(msg));
            }
            StreamEvent::Done(reason) => {
                self.flush_pill();
                self.flush_stream_line();
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
                    let pct = (metrics.used_tokens * 100)
                        .checked_div(metrics.context_size)
                        .unwrap_or(0);
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

    /// In streaming mode, render and print any buffered partial (newline-less)
    /// line as markdown. Called at turn/tool boundaries so the last line isn't
    /// lost. Prints without a trailing newline (the caller adds one if needed).
    fn flush_stream_line(&mut self) {
        if self.stream && !self.stream_line_buf.is_empty() {
            let line = std::mem::take(&mut self.stream_line_buf);
            if let Some(rendered) = crate::markdown::render_line(&line, true, &mut self.stream_in_code) {
                print!("{rendered}");
                let _ = std::io::stdout().flush();
            }
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

    /// Draw/refresh the live status line in place: a `Thinking… (Ns)` verb while
    /// the model works (no symbol, just the word + elapsed seconds), or the
    /// `▸ N tools · name running… (Ns)` pill while tools run.
    fn render_status_line(&mut self) {
        let elapsed = fmt_elapsed(self.turn_start.elapsed().as_secs());
        let line = if self.running_tool.is_some() || self.tools_done > 0 {
            let running = self
                .running_tool
                .as_deref()
                .map(|n| format!(" · {n} running…"))
                .unwrap_or_default();
            let total = self.tools_done + u32::from(self.running_tool.is_some());
            format!("▸ {total} tools{running} ({elapsed})")
        } else {
            format!("Thinking… ({elapsed})")
        };
        if self.pill_line {
            eprint!("\r\x1b[2K\x1b[2m{line}\x1b[0m");
        } else {
            eprint!("\n\x1b[2m{line}\x1b[0m");
            self.pill_line = true;
        }
        let _ = std::io::stderr().flush();
    }

    /// Advance the live status line (called on a timer by the REPL): refreshes
    /// the `Thinking…`/tool elapsed counter while the turn is in flight. Does
    /// nothing once stopped or while assistant text is actively streaming.
    pub fn tick(&mut self) {
        if self.stop.is_none() && (self.thinking || self.running_tool.is_some()) {
            self.render_status_line();
        }
    }
}

/// Human-readable elapsed time for the live status line: `45s`, `1m 10s`,
/// `2h 5m` — never a bare `70s`.
fn fmt_elapsed(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
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

    #[test]
    fn fmt_elapsed_is_human_readable() {
        assert_eq!(fmt_elapsed(45), "45s");
        assert_eq!(fmt_elapsed(70), "1m 10s");
        assert_eq!(fmt_elapsed(600), "10m 0s");
        assert_eq!(fmt_elapsed(3_725), "1h 2m");
    }
}
