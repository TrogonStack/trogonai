use std::time::Duration;

/// The unprivileged user E2B sandboxes run commands as unless a request names another.
pub(crate) const DEFAULT_USER: &str = "user";

/// Header E2B's edge reads the traffic token from on ports that are not public.
pub(crate) const TRAFFIC_TOKEN_HEADER: &str = "e2b-traffic-access-token";

/// Prefix of the per-execution directory the launcher keeps its output and exit marker in.
pub(crate) const EXECUTION_DIRECTORY_PREFIX: &str = "/tmp/.trogon-exec-";

/// The JetStream key-value bucket that holds durable command launch custody.
pub(crate) const COMMAND_CUSTODY_BUCKET: &str = "sandbox-e2b-command-custody";

/// Largest custody record the bucket accepts.
pub(crate) const MAX_CUSTODY_RECORD_BYTES: usize = 8192;

/// The only custody record layout this worker reads and writes.
pub(crate) const CUSTODY_RECORD_VERSION: u32 = 1;

/// Length of a launch fingerprint, a hex SHA-256 digest.
pub(crate) const LAUNCH_FINGERPRINT_HEX_LEN: usize = 64;

/// How often a custody compare-and-swap is retried before the outcome is reported as pending.
pub(crate) const MAX_CUSTODY_ATTEMPTS: usize = 16;

/// Longest identity, in bytes, a custody owner may carry for the lease or sandbox.
pub(crate) const MAX_LEASE_IDENTITY_BYTES: usize = 1024;
pub(crate) const MAX_SANDBOX_IDENTITY_BYTES: usize = 128;

/// How long a helper script may run through envd.
pub(crate) const SCRIPT_TIMEOUT: Duration = Duration::from_secs(30);

/// How long a deadline probe or watchdog script may run through envd.
pub(crate) const DEADLINE_SCRIPT_TIMEOUT: Duration = Duration::from_secs(5);

/// How often a foreground command is checked for its native end.
pub(crate) const FOREGROUND_POLL_INTERVAL: Duration = Duration::from_millis(20);

/// How long the deadline supervisor waits before retrying an uncertain reconciliation.
pub(crate) const SUPERVISOR_RETRY_INTERVAL: Duration = Duration::from_millis(100);

/// Largest exit marker, in bytes, the worker will read.
pub(crate) const MAX_EXIT_MARKER_BYTES: usize = 64;

/// Largest deadline outcome marker, in bytes, the worker will read.
pub(crate) const MAX_DEADLINE_OUTCOME_BYTES: usize = 128;

/// Exit code the identity probe reports when no process of the group is left.
pub(crate) const PROCESS_ABSENT_EXIT_CODE: i32 = 44;

/// Runs a launcher's directory removal once its output has been settled.
pub(crate) const REMOVE_DIRECTORY_SCRIPT: &str = "rm -rf -- \"$1\"";

/// Applies a requested file mode after an upload.
pub(crate) const CHMOD_SCRIPT: &str = "chmod \"$1\" -- \"$2\"";

pub(crate) const LAUNCH_SCRIPT: &str = r#"d=$1; budget=$2; expiry=$3; watchdog=$4; release=$5; shift 5
set +m
umask 077
for tool in bash setsid nohup sleep mkdir mv rm; do command -v "$tool" >/dev/null || exit 126; done
[ -r /proc/self/stat ] || exit 126
mkdir -- "$d" || exit 1
nohup setsid bash -c 'd=$1; shift
: >"$d/runner-ready"
n=0; while [ ! -e "$d/ready" ] && [ "$n" -lt 1000 ]; do sleep 0.01; n=$((n+1)); done
[ -e "$d/ready" ] || exit 125
# Keep the collector alive until its foreground command supplies the real TERM result.
trap : TERM
"$@" >"$d/out" 2>"$d/err" </dev/null
printf "%s\n" "$?" >"$d/exit.tmp"; mv "$d/exit.tmp" "$d/exit"
while :; do
  live=0
  for path in /proc/[0-9]*/stat; do
    IFS= read -r line 2>/dev/null <"$path" || continue
    p=${line%% *}; fields=${line##*) }; set -- $fields
    if [ "$p" != "$$" ] && [ "$1" != Z ] && [ "$3" = "$$" ] && [ "$4" = "$$" ]; then live=1; break; fi
  done
  [ "$live" = 1 ] || break
  sleep 0.05
done' bash "$d" "$@" >/dev/null 2>&1 </dev/null &
p=$!
n=0; while [ ! -e "$d/runner-ready" ] && [ "$n" -lt 200 ]; do sleep 0.01; n=$((n+1)); done
[ -e "$d/runner-ready" ] || { kill "$p" 2>/dev/null; exit 1; }
if ! IFS= read -r line <"/proc/$p/stat"; then kill "$p" 2>/dev/null; exit 1; fi
fields=${line##*) }; set -- $fields
[ "$3" = "$p" ] && [ "$4" = "$p" ] || { kill "$p" 2>/dev/null; exit 1; }
shift 19; started=$1
if [ "$expiry" != 0 ]; then
  delay=$(printf '%s.%03d' "$((budget / 1000))" "$((budget % 1000))")
  nohup setsid bash -c "$watchdog" bash "$d" "$expiry" KILL "$p" "$started" "$delay" >/dev/null 2>&1 </dev/null &
  n=0; while [ ! -e "$d/watchdog-ready" ] && [ "$n" -lt 200 ]; do sleep 0.01; n=$((n+1)); done
  [ -e "$d/watchdog-ready" ] || { kill -KILL -- "-$p" 2>/dev/null; exit 1; }
fi
printf '%s %s\n' "$p" "$started"
[ "$release" = immediate ] && : >"$d/ready"
exit 0
"#;

pub(crate) const PROCESS_IDENTITY: &str = r#"p=$1
if IFS= read -r line 2>/dev/null <"/proc/$p/stat"; then
  fields=${line##*) }; set -- $fields
  if [ "$1" != Z ]; then
    [ "$3" = "$p" ] && [ "$4" = "$p" ] || exit 45
    shift 19; printf '%s %s\n' "$p" "$1"; exit 0
  fi
fi
for path in /proc/[0-9]*/stat; do
  IFS= read -r line 2>/dev/null <"$path" || continue
  fields=${line##*) }; set -- $fields
  if [ "$1" != Z ] && [ "$3" = "$p" ] && [ "$4" = "$p" ]; then exit 45; fi
done
exit 44
"#;

pub(crate) const KILL_SCRIPT: &str = r#"d=$1; s=$2; p=$3; expected=$4
IFS= read -r line <"/proc/$p/stat" || exit 1
fields=${line##*) }; set -- $fields
[ "$3" = "$p" ] && [ "$4" = "$p" ] || exit 1
shift 19
[ "$1" = "$expected" ] || exit 1
kill -s "$s" -- "-$p" 2>/dev/null || exit 1
[ "$s" = KILL ] || exit 0
n=0
while [ "$n" -lt 200 ]; do
  live=0
  for path in /proc/[0-9]*/stat; do
    IFS= read -r line 2>/dev/null <"$path" || continue
    fields=${line##*) }; set -- $fields
    if [ "$1" != Z ] && [ "$3" = "$p" ] && [ "$4" = "$p" ]; then live=1; break; fi
  done
  if [ "$live" = 0 ]; then
    if [ ! -e "$d/exit" ]; then printf '137\n' >"$d/exit.tmp" && mv -- "$d/exit.tmp" "$d/exit"; fi
    exit 0
  fi
  sleep 0.01 || exit 1
  n=$((n+1))
done
exit 1
"#;

pub(crate) const WATCHDOG_SCRIPT: &str = r#"d=$1; expiry=$2; s=$3; p=$4; expected=$5; delay=${6:-0}
group_live() {
  for path in /proc/[0-9]*/stat; do
    IFS= read -r line 2>/dev/null <"$path" || continue
    fields=${line##*) }; set -- $fields
    if [ "$1" != Z ] && [ "$3" = "$p" ] && [ "$4" = "$p" ]; then return 0; fi
  done
  return 1
}
if [ "$#" -gt 5 ]; then
  sleep "$delay" & timer=$!
  trap 'kill "$timer" 2>/dev/null; wait "$timer" 2>/dev/null' EXIT
  : >"$d/watchdog-ready"
  while kill -0 "$timer" 2>/dev/null; do
    group_live || exit 0
    sleep 0.05 || exit 1
  done
  wait "$timer" || exit 1
fi
IFS= read -r line <"/proc/$p/stat" || exit 1
fields=${line##*) }; set -- $fields
[ "$3" = "$p" ] && [ "$4" = "$p" ] || exit 1
shift 19
[ "$1" = "$expected" ] || exit 1
kill -s "$s" -- "-$p" 2>/dev/null || exit 1
n=0
while [ "$n" -lt 200 ]; do
  if ! group_live; then
    printf '%s %s %s\n' "$p" "$expected" "$expiry" >"$d/deadline-outcome.tmp" || exit 1
    mv -- "$d/deadline-outcome.tmp" "$d/deadline-outcome" || exit 1
    if [ ! -e "$d/exit" ]; then printf '137\n' >"$d/exit.tmp" && mv -- "$d/exit.tmp" "$d/exit"; fi
    exit 0
  fi
  sleep 0.01 || exit 1
  n=$((n+1))
done
exit 1
"#;

pub(crate) const ARM_SCRIPT: &str = r#"d=$1; expiry=$2; p=$3; expected=$4; delay=$5; watchdog=$6
for tool in bash setsid nohup sleep mv rm; do command -v "$tool" >/dev/null || exit 126; done
rm -f -- "$d/watchdog-ready" || exit 1
nohup setsid bash -c "$watchdog" bash "$d" "$expiry" KILL "$p" "$expected" "$delay" >/dev/null 2>&1 </dev/null &
n=0
while [ ! -e "$d/watchdog-ready" ] && [ "$n" -lt 200 ]; do sleep 0.01; n=$((n+1)); done
[ -e "$d/watchdog-ready" ]
"#;

pub(crate) const RELEASE_SCRIPT: &str = r#"d=$1; p=$2; expected=$3
IFS= read -r line <"/proc/$p/stat" || exit 1
fields=${line##*) }; set -- $fields
[ "$3" = "$p" ] && [ "$4" = "$p" ] || exit 1
shift 19
[ "$1" = "$expected" ] || exit 1
: >"$d/ready"
"#;
