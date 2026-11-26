#!/bin/sh
set -eu

mount -t cgroup2 cgroup2 /sys/fs/cgroup
mkdir /sys/fs/cgroup/outer-supervisor /sys/fs/cgroup/guest-root
for pid in $(cat /sys/fs/cgroup/cgroup.procs); do
  echo "$pid" > /sys/fs/cgroup/outer-supervisor/cgroup.procs 2>/dev/null || true
done
echo '+cpu +memory +pids' > /sys/fs/cgroup/cgroup.subtree_control

exec unshare --mount --pid --fork --kill-child=KILL --mount-proc --propagation private /bin/sh -c '
  echo $$ > /sys/fs/cgroup/guest-root/cgroup.procs
  exec unshare --cgroup /target/debug/trogon-sandbox-guest-init
'
