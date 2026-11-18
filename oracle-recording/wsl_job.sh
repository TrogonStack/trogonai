#!/usr/bin/env bash
# Runs inside a WSL distribution on a hosted Windows runner: Node and Python run natively in the distribution, the
# SDK's own /proc/version read sees the WSL kernel, and /mnt/c is moved off the Windows drive so the reg.exe at the
# WSL path is the stand-in, never the real one.
#
# Usage: wsl_job.sh <refit|goldens|band37|band39|band40|mg37> <checkout> <output folder> <call> <context> [<shard> <shards>]
set -euo pipefail

mode="$1" checkout="$2" out="$3" call="$4" context="$5" shard="${6:-1}" shards="${7:-1}"
mkdir -p "$out/goldens"

{
  cat /proc/version
  uname -a
} | tee "$out/host.txt"
grep -qi microsoft /proc/version
# The first boot, before wsl.conf moved the automount root, can leave an empty /mnt/c folder behind on the rootfs.
# What matters is that no Windows drive is mounted there and no real reg.exe is reachable at the WSL path.
{
  ls -la /mnt
  findmnt -rn -o TARGET,FSTYPE,SOURCE
} | tee -a "$out/host.txt"
! mountpoint -q /mnt/c
test ! -e /mnt/c/Windows/System32/reg.exe

export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq rustc curl ca-certificates xz-utils git >/dev/null

rm -rf ~/repo
cp -r "$checkout" ~/repo
cd ~/repo
bash oracle-recording/check_fixture_bytes.sh ~/repo "$out"

rustc --edition 2021 -O -o ~/reg.exe oracle-recording/native/registry_stand_in.rs
install -D -m 0755 ~/reg.exe /mnt/c/Windows/System32/reg.exe

curl -fsSLo ~/node.tar.xz https://nodejs.org/dist/v24.13.1/node-v24.13.1-linux-x64.tar.xz
echo "30215f90ea3cd04dfbc06e762c021393fa173a1d392974298bbc871a8e461089  $HOME/node.tar.xz" | sha256sum -c -
tar -xJf ~/node.tar.xz -C ~
node_bin="$HOME/node-v24.13.1-linux-x64/bin/node"

curl -fsSLo ~/mise.tar.gz https://github.com/jdx/mise/releases/download/v2026.10.25/mise-v2026.10.25-linux-x64.tar.gz
echo "199966edaad1b208eb4c72955f1722dafb8fa799f575db720dea05aaa8216546  $HOME/mise.tar.gz" | sha256sum -c -
tar -xzf ~/mise.tar.gz -C ~
# From the home folder, so mise reads no repository config.
(cd ~ && ~/mise/bin/mise install python@3.12.16)
py="$(cd ~ && ~/mise/bin/mise exec python@3.12.16 -- python3 -c 'import sys; print(sys.executable)')"

{
  "$node_bin" -p 'JSON.stringify({version: process.version, platform: process.platform, arch: process.arch, v8: process.versions.v8, now: Date.now()})'
  "$py" --version
} | tee -a "$out/host.txt"

"$py" -I rsworkspace/crates/platform/trogon-claude-agent-sdk/.config/mise/lib/claude-sdk-freshness/fetch_pinned_typescript.py ~/package

export R3_NATIVE=1 R3_PYTHON=3.12.16 R3_WSL=1 R3_CALL="$call" R3_CONTEXT="$context"

case "$mode" in
  refit)
    export R3_ROUND4_PASSES=60 R5_TOPUP_RUNS=50 R5_CHAIN_PASSES=100 R3_BAND_RUNS=50
    export R3_ONLY='^(reg3c:|reg5:H|reg6:|reg7:|rec7:fixture:org_held|rec8:events:org_held|key:org_copy|key:origin_merge)'
    "$py" -I oracle-recording/r3.py oracle-recording/native "$node_bin" ~/package "$out/r3.json" 1 1 \
      2> >(tee "$out/r3.log" >&2)
    ;;
  band37)
    # The organization pairs' union band over the caller contexts (PLAN Update 93); band37 sets each run's context.
    export R3_ONLY='^(pm8:|band37:)' R3_EVERY_SHARD=1
    "$py" -I oracle-recording/r3.py oracle-recording/native "$node_bin" ~/package "$out/r3.json" "$shard" "$shards" \
      2> >(tee "$out/r3.log" >&2)
    ;;
  band39)
    # The organization pairs located for this call position and their union band (PLAN Update 99); band39 sets each
    # run's context.
    export R3_ONLY='^(pm8:|band39:)' R3_EVERY_SHARD=1
    "$py" -I oracle-recording/r3.py oracle-recording/native "$node_bin" ~/package "$out/r3.json" "$shard" "$shards" \
      2> >(tee "$out/r3.log" >&2)
    ;;
  band40)
    # The organization pairs located for this call position, placed on toplevel and nested, every context run in full
    # (PLAN Update 106); band40 sets each run's context.
    export R3_ONLY='^(pm8:|band40:)' R3_EVERY_SHARD=1
    "$py" -I oracle-recording/r3.py oracle-recording/native "$node_bin" ~/package "$out/r3.json" "$shard" "$shards" \
      2> >(tee "$out/r3.log" >&2)
    ;;
  mg37)
    # The organization goldens at their band37 union margins, observed from every caller context (PLAN Update 96).
    export R3_ONLY='^(pm8:|mg37:)'
    "$py" -I oracle-recording/r3.py oracle-recording/native "$node_bin" ~/package "$out/r3.json" 1 1 \
      2> >(tee "$out/r3.log" >&2)
    ;;
  goldens)
    golden=ts_managed_settings_linux_x64.json
    cp "oracle-recording/native/committed/$golden" "$out/goldens/$golden"
    status=0
    "$py" -I oracle-recording/native/record.py ts_managed_settings_goldens.py "$out/goldens/$golden.tally.json" \
      "$node_bin" ~/package "$out/goldens/$golden" 2> >(tee "$out/goldens/$golden.log" >&2) || status=$?
    diff -u "oracle-recording/native/committed/$golden" "$out/goldens/$golden" > "$out/goldens/$golden.diff" || true
    exit "$status"
    ;;
  *)
    echo "unknown mode $mode" >&2
    exit 2
    ;;
esac
