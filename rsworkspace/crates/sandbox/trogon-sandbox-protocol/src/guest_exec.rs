//! Shared plumbing for workers that run commands and read files through a guest-side shell, where the backend returns
//! a command's whole output in one reply.
//!
//! The guest script captures a command into temporary files, reports its exit status (or the word `timeout` when the
//! deadline stopped it) and keeps at most [`GUEST_STREAM_LIMIT`] bytes of each stream (the first and last halves) so a
//! runaway command cannot make the reply itself unbounded. [`parse_guest_capture`] then applies the request's
//! retention budget through [`CappedOutput`] exactly like the other workers.

use base64::Engine as _;
use trogon_sandbox_contract::{CommandOutput, CommandTimeoutStatus, OutputTruncation};

use crate::{CappedOutput, GuestCaptureError};

/// Most bytes of one stream the guest returns; beyond it the middle is cut in
/// the guest and reported as dropped.
pub const GUEST_STREAM_LIMIT: usize = 16 * 1024 * 1024;

/// What the guest script prints in place of the exit status when the deadline
/// stopped the command. GNU `timeout` reports a stop as 124 or 137; busybox
/// `timeout` execs the command in its own process and reports whatever the
/// signal leaves (143 for TERM, 137 for KILL). A command can return any of
/// those itself, so the script does not read them: the shell that runs the
/// command traps TERM and writes a marker file, and the script reports the
/// marker. That shell also writes an "armed" file once the trap is in place, so
/// a TERM that lands before the trap (the command never started) still reads
/// as a timeout rather than as 143.
const TIMED_OUT_REPLY: &str = "timeout";

/// The `sh -c` script that captures one command. `timeout_bin` and `env_bin`
/// name the guest's `timeout` and `env` binaries. Invoke it as
/// `sh -c <script> sandbox-capture <limit>s <cwd> [KEY=VALUE...] <command...>`.
///
/// The first line it prints is the command's own exit status, whatever the
/// value, or `timeout` when the deadline stopped it.
pub fn guest_capture_script(timeout_bin: &str, env_bin: &str) -> String {
    let half = GUEST_STREAM_LIMIT / 2;
    // Runs the command in the background of a shell that traps the TERM
    // `timeout` sends: the trap writes the marker, passes TERM on and, as
    // `timeout -k` would, escalates to KILL after a second. Nothing here
    // quotes with a single quote, so it can sit inside the outer `'...'`.
    let inner = format!(
        "t=$1; c=; \
         f() {{ : >\"$t\"; if [ -n \"$c\" ]; then kill -TERM \"$c\" 2>/dev/null; {{ sleep 1; kill -KILL \"$c\" 2>/dev/null; }} & fi; }}; \
         trap f TERM; : >\"$t.armed\"; cd -- \"$2\" || exit 1; shift 2; \
         {env_bin} -- \"$@\" & c=$!; \
         if [ -e \"$t\" ]; then kill -TERM \"$c\" 2>/dev/null; fi; \
         wait \"$c\"; r=$?; \
         while kill -0 \"$c\" 2>/dev/null; do wait \"$c\"; r=$?; done; \
         exit $r"
    );
    format!(
        "d=$(mktemp -d) || exit 1; trap 'rm -rf -- \"$d\"' EXIT; limit=$1; cwd=$2; shift 2; \
         {timeout_bin} -k 1s \"$limit\" /bin/sh -c '{inner}' sandbox-exec \"$d/timedout\" \"$cwd\" \"$@\" >\"$d/out\" 2>\"$d/err\"; rc=$?; \
         if [ -e \"$d/timedout\" ]; then rc={TIMED_OUT_REPLY}; \
         elif [ ! -e \"$d/timedout.armed\" ] && [ \"$rc\" -lt 125 -o \"$rc\" -gt 127 ]; then rc={TIMED_OUT_REPLY}; fi; \
         printf '%s\\n' \"$rc\"; \
         for f in out err; do \
         n=$(wc -c <\"$d/$f\"); printf '%s\\n' \"$n\"; \
         if [ \"$n\" -le {GUEST_STREAM_LIMIT} ]; then base64 -w 0 <\"$d/$f\"; printf '\\n\\n'; \
         else head -c {half} \"$d/$f\" | base64 -w 0; printf '\\n'; tail -c {half} \"$d/$f\" | base64 -w 0; printf '\\n'; fi; \
         done"
    )
}

fn retained_capture_location() -> &'static str {
    r#"capture_id=$1; base=${TMPDIR:-/tmp};
case "$capture_id" in ''|*[!A-Z0-9]*) exit 125;; esac
case "$base" in /*) ;; *) exit 125;; esac
d="${base%/}/sandbox-capture-$capture_id""#
}

fn retained_capture_owner_check() -> &'static str {
    r#"[ -d "$d" ] && [ ! -L "$d" ] && [ -f "$d/owner" ] && [ ! -L "$d/owner" ] || exit 125
[ "$(wc -c <"$d/owner")" -eq "$((${#capture_id}+1))" ] && [ "$(cat "$d/owner")" = "$capture_id" ] || exit 125"#
}

fn retained_capture_streams() -> String {
    let half = GUEST_STREAM_LIMIT / 2;
    format!(
        r#"producer_status="$d/producer-status-$$"
trap 'rm -f -- "$producer_status"' EXIT
printf '%s\n' "$rc"
for f in out err; do
  [ -f "$d/$f" ] && [ ! -L "$d/$f" ] || exit 125
  n=$(wc -c <"$d/$f") || exit 125
  printf '%s\n' "$n"
  if [ "$n" -le {GUEST_STREAM_LIMIT} ]; then
    base64 -w 0 <"$d/$f" || exit 125
    printf '\n\n'
  else
    rm -f -- "$producer_status" || exit 125
    {{ head -c {half} "$d/$f"; printf '%s\n' "$?" >"$producer_status"; }} | base64 -w 0 || exit 125
    [ "$(cat "$producer_status")" = 0 ] || exit 125
    printf '\n'
    rm -f -- "$producer_status" || exit 125
    {{ tail -c {half} "$d/$f"; printf '%s\n' "$?" >"$producer_status"; }} | base64 -w 0 || exit 125
    [ "$(cat "$producer_status")" = 0 ] || exit 125
    printf '\n'
  fi
  [ "$(wc -c <"$d/$f")" -eq "$n" ] || exit 125
done"#
    )
}

/// Captures into an exclusive directory retained until the owning worker cleans it up.
/// Invoke as `sh -c <script> sandbox-capture <capture-id> <limit>s <cwd> [KEY=VALUE...] <command...>`.
pub fn retained_guest_capture_script(timeout_bin: &str, env_bin: &str) -> String {
    let location = retained_capture_location();
    let streams = retained_capture_streams();
    let inner = format!(
        "t=$1; c=; \
         f() {{ : >\"$t\"; if [ -n \"$c\" ]; then kill -TERM \"$c\" 2>/dev/null; {{ sleep 1; kill -KILL \"$c\" 2>/dev/null; }} & fi; }}; \
         trap f TERM; : >\"$t.armed\"; cd -- \"$2\" || exit 1; shift 2; \
         {env_bin} -- \"$@\" & c=$!; \
         if [ -e \"$t\" ]; then kill -TERM \"$c\" 2>/dev/null; fi; \
         wait \"$c\"; r=$?; \
         while kill -0 \"$c\" 2>/dev/null; do wait \"$c\"; r=$?; done; \
         exit $r"
    );
    format!(
        r#"{location}
shift; limit=$1; cwd=$2; shift 2
umask 077
mkdir -m 700 -- "$d" || exit 125
: >"$d/out" && : >"$d/err" || exit 125
printf '%s\n' "$capture_id" >"$d/owner.pending" && mv -- "$d/owner.pending" "$d/owner" || exit 125
{timeout_bin} -k 1s "$limit" /bin/sh -c '{inner}' sandbox-exec "$d/timedout" "$cwd" "$@" >"$d/out" 2>"$d/err"; rc=$?
if [ -e "$d/timedout" ]; then rc={TIMED_OUT_REPLY};
elif [ ! -e "$d/timedout.armed" ] && [ "$rc" -lt 125 -o "$rc" -gt 127 ]; then rc={TIMED_OUT_REPLY}; fi
printf '%s\n' "$rc" >"$d/status.pending" && mv -- "$d/status.pending" "$d/status" || exit 125
{streams}"#
    )
}

/// Encodes existing retained streams without running the original command again.
/// The second argument is `completed` to require the saved user status, or a known native exit.
pub fn retained_guest_capture_read_script() -> String {
    let location = retained_capture_location();
    let owner = retained_capture_owner_check();
    let streams = retained_capture_streams();
    format!(
        r#"{location}
{owner}
if [ "$2" = completed ]; then
  [ -f "$d/status" ] && [ ! -L "$d/status" ] || exit 125
  [ "$(wc -c <"$d/status")" -le 8 ] || exit 125
  rc=$(cat "$d/status") || exit 125
else rc=$2; fi
case "$rc" in timeout) ;; ''|*[!0-9]*) exit 125;; *) [ "$rc" -le 255 ] || exit 125;; esac
{streams}"#
    )
}

/// Removes only the exact owned capture directory, and treats an already absent directory as settled.
pub fn retained_guest_capture_cleanup_script() -> String {
    let location = retained_capture_location();
    let owner = retained_capture_owner_check();
    format!(
        r#"{location}
if [ ! -e "$d" ] && [ ! -L "$d" ]; then exit 0; fi
{owner}
rm -rf -- "$d" || exit 125
[ ! -e "$d" ] && [ ! -L "$d" ]"#
    )
}

/// Turns the guest script's output into a [`CommandOutput`] under the `cap`-byte per-stream budget.
///
/// A command the deadline stopped reports exit -1 with `timed_out` set, the same shape the container worker uses; any
/// exit status the command returned itself, 124 included, is reported as is.
pub fn parse_guest_capture(stdout: &str, timeout_seconds: u32, cap: usize) -> Result<CommandOutput, GuestCaptureError> {
    let mut lines = stdout.split('\n');
    let status = lines.next().ok_or(GuestCaptureError::MissingExitCode)?.trim();
    let timed_out = status == TIMED_OUT_REPLY;
    let exit_code: i32 = if timed_out {
        CommandOutput::TIMED_OUT_EXIT_CODE
    } else {
        status.parse().map_err(|_| GuestCaptureError::InvalidExitCode)?
    };
    let mut streams = Vec::with_capacity(2);
    for _ in 0..2 {
        let total: u64 = lines
            .next()
            .ok_or(GuestCaptureError::MissingStreamSize)?
            .trim()
            .parse()
            .map_err(|_| GuestCaptureError::InvalidStreamSize)?;
        let head = decode(lines.next().unwrap_or(""))?;
        let tail = decode(lines.next().unwrap_or(""))?;
        let sent = (head.len() + tail.len()) as u64;
        if sent > total {
            return Err(GuestCaptureError::MoreThanHeld);
        }
        let mut capped = CappedOutput::new(cap);
        capped.push(&head);
        capped.skip(total - sent);
        capped.push(&tail);
        streams.push((capped.truncation(), capped.into_bytes()));
    }
    let (err_truncated, stderr) = streams.pop().ok_or(GuestCaptureError::MissingStreamSize)?;
    let (out_truncated, stdout) = streams.pop().ok_or(GuestCaptureError::MissingStreamSize)?;
    let truncated = OutputTruncation::from(out_truncated.is_truncated() || err_truncated.is_truncated());
    if timed_out {
        return Ok(CommandOutput {
            exit_code: CommandOutput::TIMED_OUT_EXIT_CODE,
            stdout,
            stderr: format!("command timed out after {timeout_seconds}s").into_bytes(),
            timed_out: CommandTimeoutStatus::TimedOut,
            truncated,
        });
    }
    Ok(CommandOutput {
        exit_code,
        stdout,
        stderr,
        timed_out: CommandTimeoutStatus::WithinDeadline,
        truncated,
    })
}

fn decode(line: &str) -> Result<Vec<u8>, GuestCaptureError> {
    base64::engine::general_purpose::STANDARD
        .decode(line.trim())
        .map_err(GuestCaptureError::Encoding)
}

/// Signals every process whose environment carries a marker. Invoke it as
/// `sh -c <script> sh KEY=VALUE <SIGNAL>`; it exits 0 when at least one
/// process was signaled. Children inherit the marker, so the whole process
/// tree of a command started with it in its environment is covered. The scan
/// repeats after a short pause, signaling only processes it has not signaled
/// yet, so a child forked after one pass listed `/proc`, or by a signal
/// handler, is caught by the next instead of being orphaned with the
/// command's output still open.
pub const SIGNAL_BY_MARKER_SCRIPT: &str = r#"marker="$1"; sig="$2"; self=$$; found=1; seen=" "; pass=0
while [ "$pass" -lt 5 ]; do
  new=1
  for environ in /proc/[0-9]*/environ; do
    pid=${environ#/proc/}; pid=${pid%/environ}
    [ "$pid" = "$self" ] && continue
    case "$seen" in *" $pid "*) continue ;; esac
    if tr '\0' '\n' < "$environ" 2>/dev/null | grep -qxF "$marker"; then
      kill "-$sig" "$pid" 2>/dev/null && { found=0; new=0; seen="$seen$pid "; }
    fi
  done
  [ "$new" = 0 ] || break
  pass=$((pass+1))
  sleep 0.1 2>/dev/null || sleep 1
done
exit $found"#;
