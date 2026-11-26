#!/bin/sh
# Boots the guest init as PID 1 in fresh mount, PID and cgroup namespaces. Run it as root on a
# disposable Linux host or container. GUEST_INIT_PROBE_CGROUP names a cgroup2 directory, relative
# to the cgroup2 root, that the caller delegates to the probe: it must hold no processes and must
# enable `+cpu +memory +pids` in its cgroup.subtree_control. The cgroup2 mount happens inside a
# private mount namespace, every cgroup change stays in a probe subtree under the delegated
# directory, and the subtree is removed again on exit.
set -eu

if [ "${GUEST_INIT_PROBE_PRIVATE_MOUNTS:-}" != 1 ]; then
  GUEST_INIT_PROBE_PRIVATE_MOUNTS=1 exec unshare --mount --propagation private /bin/sh "$0" "$@"
fi

: "${GUEST_INIT_PROBE_CGROUP:?set GUEST_INIT_PROBE_CGROUP to the delegated cgroup2 directory}"

mount -t cgroup2 cgroup2 /sys/fs/cgroup
origin="/sys/fs/cgroup$(sed -n 's/^0:://p' /proc/self/cgroup)"
delegated="/sys/fs/cgroup/${GUEST_INIT_PROBE_CGROUP#/}"
if [ ! -f "$delegated/cgroup.subtree_control" ]; then
  echo "$delegated is not a cgroup2 directory" >&2
  exit 1
fi
probe="$delegated/guest-init-probe"
if [ -e "$probe" ]; then
  echo "$probe already exists; remove it before running the probe" >&2
  exit 1
fi

cleanup() {
  echo $$ > "$origin/cgroup.procs" 2>/dev/null || true
  rmdir "$probe/guest-root" "$probe/supervisor" "$probe" 2>/dev/null || true
}
trap cleanup EXIT
trap 'exit 130' INT TERM

mkdir "$probe" "$probe/supervisor" "$probe/guest-root"
echo $$ > "$probe/supervisor/cgroup.procs"
echo '+cpu +memory +pids' > "$probe/cgroup.subtree_control"

unshare --mount --pid --fork --kill-child=KILL --mount-proc --propagation private /bin/sh -c '
  echo $$ > "$1/cgroup.procs"
  exec unshare --cgroup /target/debug/trogon-sandbox-guest-init
' guest-init-probe "$probe/guest-root"
