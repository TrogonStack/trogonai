//! Capture keyboard input *while a response is streaming*.
//!
//! The REPL's normal line editing (rustyline) is inactive during streaming, so
//! this module runs a small raw-mode reader on `/dev/tty` for the duration of a
//! turn. It lets the user:
//!   - type a message + Enter → queued, auto-submitted in order when the turn
//!     ends, and
//!   - press Ctrl+G then a line + Enter → the message jumps to the FRONT of the
//!     queue, so it's the first one sent once the current turn finishes.
//!
//! Terminal mode: we disable canonical mode and echo (so we see each byte and
//! control our own echo) but KEEP `ISIG`, so Ctrl+C still raises SIGINT and the
//! REPL's existing interrupt path is unchanged. A RAII guard always restores the
//! original termios and stops the reader thread, so the terminal can never be
//! left in raw mode.

use std::io::Write;
use std::os::fd::AsRawFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Ctrl+G — the next submitted line jumps to the front of the queue.
const PRIORITY_KEY: u8 = 0x07;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamInputEvent {
    /// A normal line submitted during streaming — queued, auto-submitted in order.
    Queued(String),
    /// A line submitted after Ctrl+G — jumps to the front of the queue.
    Priority(String),
}

/// Pure line-editor: feed it one byte at a time, get an event on Enter.
/// Kept free of I/O so it can be unit-tested.
pub(crate) struct LineEditor {
    buf: Vec<u8>,
    priority_mode: bool,
}

impl LineEditor {
    pub(crate) fn new() -> Self {
        Self { buf: Vec::new(), priority_mode: false }
    }

    /// Returns `true` if `byte` is a printable character that was appended (so
    /// the caller can echo it). Control bytes are handled internally.
    pub(crate) fn is_printable(byte: u8) -> bool {
        byte >= 0x20 && byte != 0x7f
    }

    pub(crate) fn feed(&mut self, byte: u8) -> Option<StreamInputEvent> {
        match byte {
            PRIORITY_KEY => {
                self.priority_mode = true;
                None
            }
            b'\r' | b'\n' => {
                let text = String::from_utf8_lossy(&self.buf).trim().to_string();
                let side = self.priority_mode;
                self.buf.clear();
                self.priority_mode = false;
                if text.is_empty() {
                    return None;
                }
                Some(if side {
                    StreamInputEvent::Priority(text)
                } else {
                    StreamInputEvent::Queued(text)
                })
            }
            0x7f | 0x08 => {
                self.buf.pop();
                None
            }
            b if Self::is_printable(b) => {
                self.buf.push(b);
                None
            }
            _ => None,
        }
    }

    #[cfg(test)]
    pub(crate) fn in_priority_mode(&self) -> bool {
        self.priority_mode
    }
}

/// Owns the raw-mode reader thread for the duration of a streaming turn. Drop
/// stops the thread and restores the terminal.
pub struct StreamInputReader {
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
    // Kept so the fd/termios outlive the thread; restored on drop.
    tty: std::fs::File,
    original: libc::termios,
}

impl StreamInputReader {
    /// Start reading `/dev/tty`. Returns the reader (drop to stop) and a receiver
    /// of input events. Returns `None` if `/dev/tty` can't be opened or put into
    /// raw mode (e.g. not a real terminal) — callers should just skip the feature.
    pub fn start() -> Option<(Self, tokio::sync::mpsc::UnboundedReceiver<StreamInputEvent>)> {
        let tty = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/tty")
            .ok()?;
        let fd = tty.as_raw_fd();

        let original = unsafe {
            let mut t: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(fd, &mut t) != 0 {
                return None;
            }
            t
        };
        // Raw-ish: no canonical line buffering, no echo, but keep ISIG so Ctrl+C
        // still generates SIGINT (the REPL handles interrupt via the signal).
        unsafe {
            let mut raw = original;
            raw.c_lflag &= !(libc::ICANON | libc::ECHO);
            raw.c_lflag |= libc::ISIG;
            raw.c_iflag &= !(libc::IXON);
            raw.c_cc[libc::VMIN] = 0;
            raw.c_cc[libc::VTIME] = 0;
            if libc::tcsetattr(fd, libc::TCSANOW, &raw) != 0 {
                return None;
            }
        }

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = Arc::clone(&stop);
        let reader_fd = fd;
        let handle = std::thread::spawn(move || {
            let mut editor = LineEditor::new();
            let mut tty_out = unsafe {
                use std::os::fd::FromRawFd;
                std::mem::ManuallyDrop::new(std::fs::File::from_raw_fd(reader_fd))
            };
            while !stop_thread.load(Ordering::Relaxed) {
                if !poll_readable(reader_fd, 100) {
                    continue;
                }
                let byte = match read_byte(reader_fd) {
                    Some(b) => b,
                    None => continue,
                };
                if byte == 0x1b {
                    // Swallow escape/CSI sequences (arrow keys, paste markers) so
                    // they don't land in the buffer as garbage.
                    consume_escape(reader_fd);
                    continue;
                }
                // Echo printable bytes and backspace so the user sees their input.
                if LineEditor::is_printable(byte) {
                    let _ = tty_out.write_all(&[byte]);
                    let _ = tty_out.flush();
                } else if byte == 0x7f || byte == 0x08 {
                    let _ = tty_out.write_all(b"\x08 \x08");
                    let _ = tty_out.flush();
                } else if byte == PRIORITY_KEY {
                    let _ = tty_out.write_all(b"\r\n\x1b[2m next> \x1b[0m");
                    let _ = tty_out.flush();
                }
                if let Some(ev) = editor.feed(byte) {
                    let _ = tty_out.write_all(b"\r\n");
                    let _ = tty_out.flush();
                    if tx.send(ev).is_err() {
                        break;
                    }
                }
            }
        });

        Some((Self { stop, handle: Some(handle), tty, original }, rx))
    }
}

impl Drop for StreamInputReader {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
        // Restore the original terminal settings.
        let fd = self.tty.as_raw_fd();
        unsafe {
            libc::tcsetattr(fd, libc::TCSANOW, &self.original);
        }
    }
}

fn poll_readable(fd: i32, timeout_ms: i32) -> bool {
    let mut pfd = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
    let ready = unsafe { libc::poll(&mut pfd, 1, timeout_ms) };
    ready > 0 && pfd.revents & libc::POLLIN != 0
}

fn read_byte(fd: i32) -> Option<u8> {
    let mut buf = [0u8; 1];
    let n = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, 1) };
    if n == 1 {
        Some(buf[0])
    } else {
        None
    }
}

/// Discard the remainder of an ESC-introduced sequence (CSI / arrow / paste).
fn consume_escape(fd: i32) {
    // ESC alone or ESC [ ... final-byte. Read a few bytes with a short timeout.
    if !poll_readable(fd, 30) {
        return;
    }
    if read_byte(fd) != Some(b'[') {
        return;
    }
    loop {
        if !poll_readable(fd, 30) {
            break;
        }
        match read_byte(fd) {
            Some(b) if b.is_ascii_alphabetic() || b == b'~' => break,
            Some(_) => continue,
            None => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed_str(ed: &mut LineEditor, s: &str) -> Option<StreamInputEvent> {
        let mut last = None;
        for b in s.bytes() {
            last = ed.feed(b);
        }
        last
    }

    #[test]
    fn plain_line_is_queued() {
        let mut ed = LineEditor::new();
        assert_eq!(
            feed_str(&mut ed, "fix the tests\r"),
            Some(StreamInputEvent::Queued("fix the tests".into()))
        );
    }

    #[test]
    fn newline_also_submits() {
        let mut ed = LineEditor::new();
        assert_eq!(
            feed_str(&mut ed, "hello\n"),
            Some(StreamInputEvent::Queued("hello".into()))
        );
    }

    #[test]
    fn ctrl_g_marks_priority() {
        let mut ed = LineEditor::new();
        assert_eq!(ed.feed(PRIORITY_KEY), None);
        assert!(ed.in_priority_mode());
        assert_eq!(
            feed_str(&mut ed, "what is big-O of this?\r"),
            Some(StreamInputEvent::Priority("what is big-O of this?".into()))
        );
        // side mode resets after submit
        assert!(!ed.in_priority_mode());
        assert_eq!(
            feed_str(&mut ed, "next one\r"),
            Some(StreamInputEvent::Queued("next one".into()))
        );
    }

    #[test]
    fn backspace_edits_buffer() {
        let mut ed = LineEditor::new();
        // "helllo" then two backspaces ("hell") then "o" → "hello"
        assert_eq!(
            feed_str(&mut ed, "helllo\x7f\x7fo\r"),
            Some(StreamInputEvent::Queued("hello".into()))
        );
    }

    #[test]
    fn empty_line_emits_nothing() {
        let mut ed = LineEditor::new();
        assert_eq!(feed_str(&mut ed, "   \r"), None);
    }

    #[test]
    fn whitespace_is_trimmed() {
        let mut ed = LineEditor::new();
        assert_eq!(
            feed_str(&mut ed, "  spaced  \r"),
            Some(StreamInputEvent::Queued("spaced".into()))
        );
    }
}
