#!/usr/bin/env bash
# Spike driver for trogon-sandbox-hluk-worker on a KVM host. Prints Markdown to stdout, diagnostics to stderr.
#   run.sh probe    what hluk's CLI does with the things a run_command needs, and its raw per-run latency
#   run.sh build    the hluk and docker workers, the latency example, the conformance tests (release) and the
#                   docker worker's keep-alive image
#   run.sh measure  exec latency through NATS -> worker -> hluk for hluk and docker, and the conformance suite
set -uo pipefail

root=$(git rev-parse --show-toplevel)
version=${HLUK_VERSION:-v0.21.0}
bin=${RUNNER_TEMP:-/tmp}/hluk-bin
export PATH=$bin:$PATH
work=$(mktemp -d "${RUNNER_TEMP:-/tmp}/hluk-spike.XXXXXX")
runs=${RUNS:-50}
iterations=${ITERATIONS:-50}
target=$root/rsworkspace/target
nats_url=nats://127.0.0.1:4222
# The docker worker execs into the container its image starts, so the image has to keep running.
docker_image=sandbox-conformance:local

install_hluk() {
  command -v hluk >/dev/null && return
  mkdir -p "$bin"
  curl -fsSL "https://raw.githubusercontent.com/hyperlight-dev/hyperlight-unikraft/$version/install.sh" \
    | HLUK_VERSION=$version HLUK_INSTALL_DIR=$bin sh >&2
}

oneline() { tr '|\n' '  ' <<<"$1" | cut -c1-200; }

# One row: what the command printed and how it exited.
probe() { # label cmd...
  local label=$1 out code
  shift
  out=$("$@" 2>"$work/err" </dev/null)
  code=$?
  printf '| %s | %s | %s | %s |\n' "$label" "$code" "$(oneline "$out")" "$(oneline "$(<"$work/err")")"
}

# p50, p99 and mean of microsecond samples on stdin, in milliseconds.
summary() {
  sort -n | awk '{ v[NR] = $1; s += $1 } END {
    printf "%d | %.2f ms | %.2f ms | %.2f ms", NR, v[int(0.5 * NR + 0.999)] / 1000, v[int(0.99 * NR + 0.999)] / 1000, s / NR / 1000 }'
}

series() { # label cmd...
  local label=$1 start samples=()
  shift
  for _ in $(seq "$runs"); do
    start=${EPOCHREALTIME/./}
    "$@" >/dev/null 2>&1 </dev/null || { echo "| $label | failed | | | |"; return; }
    samples+=($((${EPOCHREALTIME/./} - start)))
  done
  echo "| $label | $(printf '%s\n' "${samples[@]}" | summary) |"
}

cmd_probe() {
  install_hluk || { echo "hluk $version did not install"; exit 1; }
  hluk --version >&2
  mkdir -p "$work/ws"
  echo from-host >"$work/ws/in"
  echo "### hluk $version CLI semantics"
  echo
  echo "| probe | exit | stdout | stderr |"
  echo "|---|---|---|---|"
  probe "bash warm-up (pull + snapshot)" hluk run --runtime bash --exec true
  probe "bash: stdout, stderr, exit 3" hluk run --runtime bash --exec 'echo out; echo err >&2; exit 3'
  probe "bash: unknown command" hluk run --runtime bash --exec 'definitely-not-a-command-xyz'
  probe "bash: sh -c 'exit 42'" hluk run --runtime bash --exec "'sh' '-c' 'exit 42'"
  probe "bash: child status in \$?" hluk run --runtime bash --exec "sh -c 'exit 42'; echo \$?; false; echo \$?"
  probe "bash: subshell (exit 42)" hluk run --runtime bash --exec '(exit 42); echo $?'
  probe "bash: exec sh -c 'exit 42'" hluk run --runtime bash --exec "exec sh -c 'exit 42'"
  probe "bash: exec unknown command" hluk run --runtime bash --exec 'exec definitely-not-a-command-xyz'
  probe "bash: command -v unknown" hluk run --runtime bash --exec 'command -v definitely-not-a-command-xyz; echo $?'
  probe "bash: unknown, then exit \$?" hluk run --runtime bash --exec 'definitely-not-a-command-xyz
exit $?'
  probe "bash: sh -c 'exit 42', then exit \$?" hluk run --runtime bash --exec "sh -c 'exit 42'
exit \$?"
  probe "bash: printf binary bytes" hluk run --runtime bash --exec "printf '\\377\\376x' | od -An -tx1"
  probe "bash: binary stdout via od on host" sh -c "hluk run --runtime bash --exec \"printf '\\377\\376x'\" | od -An -tx1"
  probe "bash: wait status" hluk run --runtime bash --exec "sh -c 'exit 42' & wait \$!; echo \$?"
  probe "bash: applets" hluk run --runtime bash --exec 'busybox --list | tr "\n" " "'
  probe "bash: --env" hluk run --runtime bash --env 'FOO=per call value' --exec 'printenv FOO'
  probe "bash: cat of closed stdin" hluk run --runtime bash --exec cat
  probe "bash: default cwd" hluk run --runtime bash --exec pwd
  probe "bash: cd missing dir" hluk run --runtime bash --exec "cd /nope || exit 1
echo ran"
  probe "bash: --mount rw read+write" hluk run --runtime bash --mount "$work/ws:/workspace" \
    --exec 'cat /workspace/in; echo from-guest > /workspace/out; ls /workspace'
  probe "host sees guest write" cat "$work/ws/out"
  probe "bash: --mount ro write" hluk run --runtime bash --mount "$work/ws:/workspace:ro" \
    --exec 'echo x > /workspace/ro-out && echo WROTE || echo REFUSED'
  probe "bash: /tmp write" hluk run --runtime bash --exec 'echo kept > /tmp/state && cat /tmp/state'
  probe "bash: /tmp after a new run" hluk run --runtime bash --exec 'cat /tmp/state'
  mkdir -p "$work/tmp"
  probe "bash: two mounts, /tmp write" hluk run --runtime bash --mount "$work/ws:/workspace" --mount "$work/tmp:/tmp" \
    --exec 'echo kept > /tmp/state && ls /workspace'
  probe "bash: mounted /tmp after a new run" hluk run --runtime bash --mount "$work/tmp:/tmp" --exec 'cat /tmp/state'
  probe "host sees mounted /tmp" cat "$work/tmp/state"
  probe "python: mounted /tmp tempfile" hluk run --runtime python --mount "$work/tmp:/tmp" \
    --exec 'import tempfile; print(tempfile.mkstemp()[1])'
  probe "bash: connect without --net" hluk run --runtime bash \
    --exec 'nc -w 3 1.1.1.1 443 </dev/null && echo CONNECTED || echo DENIED'
  probe "bash: connect with --net" hluk run --runtime bash --net \
    --exec 'nc -w 3 1.1.1.1 443 </dev/null && echo CONNECTED || echo DENIED'
  probe "bash: sleep 30, killed at 2 s" timeout 2 hluk run --runtime bash --exec 'sleep 30'
  probe "python warm-up" hluk run --runtime python --exec pass
  probe "python: exit 4" hluk run --runtime python --exec 'import sys; print(sys.version); sys.exit(4)'
  probe "node warm-up" hluk run --runtime node --exec 0
  probe "node: exit 5" hluk run --runtime node --exec 'console.log(process.version); process.exit(5)'
  echo
  echo "### Raw hluk run per call, no NATS or worker ($runs runs)"
  echo
  echo "| command | n | p50 | p99 | mean |"
  echo "|---|---|---|---|---|"
  series "hluk run --runtime bash --exec true" hluk run --runtime bash --exec true
  series "… one mount" hluk run --runtime bash --mount "$work/ws:/workspace" --exec "cd '/workspace' || exit 1
'true'"
  series "… one mount, exec" hluk run --runtime bash --mount "$work/ws:/workspace" --exec "cd '/workspace' || exit 1
exec 'true'"
  series "… both mounts" hluk run --runtime bash --mount "$work/ws:/workspace" --mount "$work/tmp:/tmp" \
    --exec "cd '/workspace' || exit 1
'true'"
  series "… both mounts, exit \$? (what the worker runs)" hluk run --runtime bash \
    --mount "$work/ws:/workspace" --mount "$work/tmp:/tmp" --exec "cd '/workspace' || exit 1
'true'
exit \$?"
  series "hluk run --runtime python --exec pass" hluk run --runtime python --exec pass
}

cmd_build() {
  cd "$root/rsworkspace" || exit 1
  cargo build --release --locked -p trogon-sandbox-hluk-worker -p trogon-sandbox-docker-worker &&
    cargo build --release --locked -p trogon-sandbox-conformance --example latency &&
    cargo test --release --locked -p trogon-sandbox-hluk-worker &&
    cargo test --release --locked -p trogon-sandbox-conformance --test conformance --no-run &&
    printf 'FROM alpine:3.20\nCMD ["sleep","infinity"]\n' | docker build --quiet --tag "$docker_image" -
}

pids=()
cleanup() { for pid in "${pids[@]}"; do kill "$pid" 2>/dev/null; done; wait 2>/dev/null; }
trap cleanup EXIT

wait_for() { # log pattern seconds
  for _ in $(seq $(($3 * 5))); do grep -q "$2" "$1" && return 0; sleep 0.2; done
  echo "timed out waiting for '$2' in $1:" >&2
  cat "$1" >&2
  return 1
}

latency() { # provider image
  "$target/release/examples/latency" --nats-url "$nats_url" --provider "$1" --image "$2" --iterations "$iterations"
}

cmd_measure() {
  install_hluk || exit 1
  nats-server -a 127.0.0.1 -p 4222 >"$work/nats.log" 2>&1 &
  pids+=($!)
  wait_for "$work/nats.log" 'Server is ready' 10 || exit 1

  SANDBOX_DISPATCH_VERIFY=off SANDBOX_NATS_URL=$nats_url SANDBOX_HLUK_BIN=$bin/hluk \
    SANDBOX_HLUK_WORK_DIR=$work/leases "$target/release/trogon-sandbox-hluk-worker" >"$work/hluk-worker.log" 2>&1 &
  local hluk_pid=$!
  pids+=("$hluk_pid")
  wait_for "$work/hluk-worker.log" 'worker ready' 30 || exit 1

  echo "### Lease latency through NATS -> worker -> backend ($iterations leases each)"
  echo
  echo '"first exec" is one `run_command ["true"]` per lease; for hluk that is one `hluk run` restored from the warm snapshot.'
  echo
  echo '```text'
  latency hluk bash
  echo '```'

  echo
  echo "### Conformance suite against the hluk worker (image bash)"
  echo
  echo "| check | verdict | why |"
  echo "|---|---|---|"
  (cd "$root/rsworkspace" &&
    SANDBOX_CONFORMANCE_NATS_URL=$nats_url SANDBOX_CONFORMANCE_IMAGE=bash \
      cargo test --release --locked -p trogon-sandbox-conformance --test conformance \
      -- --ignored --exact hluk_worker_conforms --nocapture --test-threads 1 2>"$work/conformance.err") |
    grep '^| '
  tail -5 "$work/conformance.err" >&2
  echo "::group::hluk worker log" >&2
  tail -50 "$work/hluk-worker.log" >&2
  echo "::endgroup::" >&2
  kill "$hluk_pid"

  echo
  echo "### Same harness against the docker worker (alpine keep-alive image)"
  echo
  SANDBOX_DISPATCH_VERIFY=off SANDBOX_NATS_URL=$nats_url SANDBOX_IMAGE=$docker_image \
    SANDBOX_EGRESS_PROXY_IMAGE=$docker_image \
    SANDBOX_DOCKER_STATE_DIR=$work/docker-state SANDBOX_DOCKER_WORKER_OWNER="hluk-spike-$$" \
    "$target/release/trogon-sandbox-docker-worker" >"$work/docker-worker.log" 2>&1 &
  pids+=($!)
  if wait_for "$work/docker-worker.log" 'worker ready' 60; then
    echo '```text'
    latency local_docker "$docker_image"
    echo '```'
  else
    echo "docker worker did not start; see the job log"
  fi
}

case ${1:-} in
  probe) cmd_probe ;;
  build) cmd_build ;;
  measure) cmd_measure ;;
  *) echo "usage: $0 probe|build|measure" >&2; exit 2 ;;
esac
