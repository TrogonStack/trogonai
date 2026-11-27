/// Most ports one lease can open after create; matches the create-time cap
/// on `exposed_ports` plus one.
pub(crate) const MAX_FORWARDS_PER_LEASE: usize = 16;

/// Connects stdin/stdout to `127.0.0.1:$1` inside the container with the
/// first tool the image has: socat, nc, then bash's `/dev/tcp`.
///
/// Neither socat nor nc ends when the server closes first: they wait for the
/// stdin the exec keeps open for the client, so the client would never see
/// the end of the reply. `run` therefore watches the tool's socket and stops
/// the tool once the peer has closed and everything it sent was read (a
/// `CLOSE_WAIT` socket with an empty receive queue, on two polls in a row).
/// The client closing is the other direction: its end of input reaches the
/// tool as EOF, which half-closes the connection, and the tool is left to
/// drain the reply (socat's `-t` is raised so that wait is not cut at half a
/// second). bash's `/dev/tcp` cannot half-close, so there the client's end of
/// input only stops the sending side. The tool gets stdin through fd 3 because
/// a shell redirects a background command's stdin to `/dev/null` before it
/// applies `<&0`.
pub(super) const BRIDGE_SCRIPT: &str = r#"remote_closed() {
  for fd in /proc/$1/fd/*; do
    l=$(readlink "$fd" 2>/dev/null) || continue
    case $l in
      socket:\[*\])
        i=${l#socket:\[}; i=${i%\]}
        awk -v i="$i" '$10 == i && $4 == "08" && $5 ~ /:00000000$/ { f = 1 } END { exit !f }' /proc/net/tcp /proc/net/tcp6 2>/dev/null && return 0
        ;;
    esac
  done
  return 1
}
run() {
  exec 3<&0
  "$@" <&3 3<&- &
  pid=$!
  seen=0
  while :; do
    state=$(sed -n 's/^State:[[:space:]]*\(.\).*/\1/p' /proc/$pid/status 2>/dev/null)
    case $state in ""|Z) break ;; esac
    if remote_closed $pid; then
      seen=$((seen + 1))
      if [ $seen -ge 2 ]; then kill $pid 2>/dev/null; break; fi
    else
      seen=0
    fi
    sleep 0.2
  done
  wait $pid
  exit $?
}
if command -v socat >/dev/null 2>&1; then run socat -t 3600 - "TCP:127.0.0.1:$1"; fi
if command -v nc >/dev/null 2>&1; then run nc 127.0.0.1 "$1"; fi
if command -v bash >/dev/null 2>&1; then exec bash -c 'exec 4<&0; exec 3<>"/dev/tcp/127.0.0.1/$0" || exit 1; cat <&3 4<&- & reader=$!; cat <&4 >&3 4<&- & writer=$!; wait $reader; kill $writer 2>/dev/null; exit 0' "$1"; fi
echo "no socat, nc or bash in the image to reach port $1" >&2
exit 127"#;

/// Longest wait before retrying after a failed accept.
pub(super) const ACCEPT_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(100);

/// Size of the buffer used to relay client bytes into the bridge exec.
pub(super) const RELAY_BUFFER_BYTES: usize = 16 * 1024;
