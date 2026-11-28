#!/usr/bin/env bash
# Build the Firecracker guest rootfs: Ubuntu 24.04 CI image plus sandbox-actor-agent.
# Runs on a Linux host with sudo (or as root). Re-running replaces the output atomically.
set -euo pipefail

# The guest architecture picks the Firecracker CI artifacts; --arch selects it (default x86_64).
ARCH="x86_64"
CI_ROOT="https://s3.amazonaws.com/spec.ccfc.min/firecracker-ci/v1.15"
# The engine image also carries the matching `dagger` CLI, so one pin covers both.
DAGGER_VERSION="v0.21.10"
DAGGER_ENGINE_IMAGE="registry.dagger.io/engine:${DAGGER_VERSION}@sha256:2a7c054e0864e297671c24a0726ebf1c9747d70291b8843b1726daaff200a334"
DAGGER_ROOT="/opt/dagger-engine"

usage() {
  cat <<USAGE
Usage: $0 --agent-bin PATH --out PATH [options]

  --agent-bin PATH     statically linked (<arch>-unknown-linux-musl) sandbox-actor-agent
  --arch ARCH          guest architecture: x86_64 (default) or aarch64
  --out PATH           ext4 rootfs image to produce
  --size SIZE          image size passed to truncate (default: 2G)
  --cache-dir DIR      where downloads are kept (default: dirname of --out)
  --vsock-port PORT    guest vsock port the agent listens on (default: 8110)
  --kernel-out PATH    also download the CI kernel to PATH
  --squashfs PATH      use this base squashfs instead of downloading it
  --squashfs-sha256 HEX
                       required SHA-256 of the --squashfs file (or set SANDBOX_SQUASHFS_SHA256);
                       the build stops on a mismatch before anything is unpacked
  --kernel-config PATH custom kernel configuration from build-guest-kernel.sh; keep native
                       container iptables instead of the CI kernel compatibility wrapper
  --with-dagger        also install the Dagger engine ${DAGGER_VERSION} and CLI (needs docker
                       on the build host; use --size 3G or more)
USAGE
  exit 2
}

AGENT_BIN=""
OUT=""
SIZE="2G"
CACHE_DIR=""
VSOCK_PORT=8110
KERNEL_OUT=""
KERNEL_CONFIG=""
SQUASHFS=""
SQUASHFS_EXPECTED="${SANDBOX_SQUASHFS_SHA256:-}"
WITH_DAGGER=0

while [ $# -gt 0 ]; do
  case "$1" in
    --agent-bin) AGENT_BIN="$2"; shift 2 ;;
    --out) OUT="$2"; shift 2 ;;
    --arch) ARCH="$2"; shift 2 ;;
    --size) SIZE="$2"; shift 2 ;;
    --cache-dir) CACHE_DIR="$2"; shift 2 ;;
    --vsock-port) VSOCK_PORT="$2"; shift 2 ;;
    --kernel-out) KERNEL_OUT="$2"; shift 2 ;;
    --kernel-config) KERNEL_CONFIG="$2"; shift 2 ;;
    --squashfs) SQUASHFS="$2"; shift 2 ;;
    --squashfs-sha256) SQUASHFS_EXPECTED="$2"; shift 2 ;;
    --with-dagger) WITH_DAGGER=1; shift ;;
    -h|--help) usage ;;
    *) echo "unknown argument: $1" >&2; usage ;;
  esac
done

[ -n "$AGENT_BIN" ] && [ -n "$OUT" ] || usage
case "$ARCH" in
  x86_64)
    SQUASHFS_SHA256="68321e0482baeb3844dafe8a6b08a6902401a7afc41fbfd8c3d9ea08aadd244f"
    KERNEL_SHA256="e20e46d0c36c55c0d1014eb20576171b3f3d922260d9f792017aeff53af3d4f2" ;;
  aarch64)
    SQUASHFS_SHA256="0efb6a3ff2982baa6ca7e3d940966516ba7ddd2df5deb3e6c2161d369a15d608"
    KERNEL_SHA256="e3544b10603acbf3db492cb52e000d22ba202cb4b63b9add027565683e11c591" ;;
  *) echo "unsupported --arch: $ARCH (use x86_64 or aarch64)" >&2; exit 2 ;;
esac
CI_BASE="${CI_ROOT}/${ARCH}"
SQUASHFS_URL="${CI_BASE}/ubuntu-24.04.squashfs"
KERNEL_URL="${CI_BASE}/vmlinux-6.1.155"
[ -x "$AGENT_BIN" ] || { echo "agent binary not found or not executable: $AGENT_BIN" >&2; exit 1; }

sha256_of() {
  if command -v sha256sum >/dev/null; then sha256sum "$1" | cut -d' ' -f1; else shasum -a 256 "$1" | cut -d' ' -f1; fi
}

# A caller-supplied base image is checked before anything else runs, and the file is left
# alone on a mismatch because it is not ours to delete.
if [ -n "$SQUASHFS" ]; then
  [ -n "$SQUASHFS_EXPECTED" ] || {
    echo "--squashfs needs --squashfs-sha256 (or SANDBOX_SQUASHFS_SHA256) to verify it" >&2; exit 1;
  }
  [ -f "$SQUASHFS" ] || { echo "squashfs not found: $SQUASHFS" >&2; exit 1; }
  got="$(sha256_of "$SQUASHFS" | tr 'A-F' 'a-f')"
  want="$(printf '%s' "$SQUASHFS_EXPECTED" | tr 'A-F' 'a-f')"
  if [ "$got" != "$want" ]; then
    echo "checksum mismatch for $SQUASHFS: expected $want, got $got" >&2
    exit 1
  fi
fi

AGENT_BIN="$(readlink -f "$AGENT_BIN")"
OUT="$(readlink -f -m "$OUT")"
CACHE_DIR="$(readlink -f -m "${CACHE_DIR:-$(dirname "$OUT")}")"

# A dynamic interpreter in the ELF header means the guest would need the host's libc.
if readelf -l "$AGENT_BIN" 2>/dev/null | grep -q "INTERP"; then
  echo "agent binary must be statically linked (build for ${ARCH}-unknown-linux-musl)" >&2
  exit 1
fi

if [ -n "$KERNEL_CONFIG" ]; then
  [ -z "$KERNEL_OUT" ] || { echo '--kernel-config cannot use the downloaded CI kernel' >&2; exit 1; }
  KERNEL_CONFIG="$(readlink -f "$KERNEL_CONFIG")"
  [ -f "$KERNEL_CONFIG" ] || { echo 'custom kernel configuration is missing' >&2; exit 1; }
  fragment="$(dirname "${BASH_SOURCE[0]}")/container-kernel.config"
  while IFS= read -r setting; do
    grep -Fxq -- "$setting" "$KERNEL_CONFIG" || {
      echo "custom kernel lacks required setting: $setting" >&2; exit 1;
    }
  done < "$fragment"
fi

SUDO=""
[ "$(id -u)" -eq 0 ] || SUDO="sudo"

for tool in unsquashfs mkfs.ext4 curl truncate; do
  command -v "$tool" >/dev/null || { echo "missing required tool: $tool" >&2; exit 1; }
done
if [ "$WITH_DAGGER" -eq 1 ]; then
  command -v docker >/dev/null || { echo "--with-dagger needs docker to fetch the engine image" >&2; exit 1; }
fi

mkdir -p "$CACHE_DIR" "$(dirname "$OUT")"

# Every download is checked against its pinned digest, cached copies included, and a
# mismatch removes the file and stops the build.
fetch() {
  local url="$1" dest="$2" want="$3" got
  if [ ! -s "$dest" ]; then
    curl -fSL --retry 3 -o "$dest.part" "$url"
    mv "$dest.part" "$dest"
  fi
  got="$(sha256_of "$dest")"
  if [ "$got" != "$want" ]; then
    rm -f "$dest"
    echo "checksum mismatch for $url: expected $want, got $got" >&2
    exit 1
  fi
}

if [ -z "$SQUASHFS" ]; then
  SQUASHFS="$CACHE_DIR/ubuntu-24.04.squashfs"
  fetch "$SQUASHFS_URL" "$SQUASHFS" "$SQUASHFS_SHA256"
fi
if [ -n "$KERNEL_OUT" ]; then
  mkdir -p "$(dirname "$KERNEL_OUT")"
  fetch "$KERNEL_URL" "$KERNEL_OUT" "$KERNEL_SHA256"
fi

STAGE="$(mktemp -d "$CACHE_DIR/rootfs-stage.XXXXXX")"
cleanup() { $SUDO rm -rf "$STAGE"; rm -f "$OUT.part"; }
trap cleanup EXIT

ROOT="$STAGE/root"
$SUDO unsquashfs -no-progress -d "$ROOT" "$SQUASHFS" >/dev/null

$SUDO install -D -m 0755 "$AGENT_BIN" "$ROOT/usr/local/bin/sandbox-actor-agent"

# The identity comes from `ate.actor_id=` on the kernel command line, which the worker always
# sets. A boot without it fails instead of inventing an identity.
$SUDO tee "$ROOT/usr/local/bin/write-actor-id" >/dev/null <<'SCRIPT'
#!/bin/sh
set -eu
id=""
for arg in $(cat /proc/cmdline); do
  case "$arg" in
    ate.actor_id=*) id="${arg#ate.actor_id=}" ;;
  esac
done
if [ -z "$id" ]; then
  echo "write-actor-id: no ate.actor_id= argument on the kernel command line" >&2
  exit 1
fi
mkdir -p /run/ate
printf '%s' "$id" > /run/ate/actor-id
SCRIPT
$SUDO chmod 0755 "$ROOT/usr/local/bin/write-actor-id"

$SUDO tee "$ROOT/etc/systemd/system/sandbox-actor-agent.service" >/dev/null <<UNIT
[Unit]
Description=Sandbox actor agent
After=local-fs.target

[Service]
Type=simple
Environment=SANDBOX_ACTOR_AGENT_VSOCK_PORT=${VSOCK_PORT}
ExecStartPre=/usr/local/bin/write-actor-id
ExecStart=/usr/local/bin/sandbox-actor-agent
Restart=always
RestartSec=1

[Install]
WantedBy=multi-user.target
UNIT

$SUDO mkdir -p "$ROOT/etc/systemd/system/multi-user.target.wants"
$SUDO ln -sf /etc/systemd/system/sandbox-actor-agent.service \
  "$ROOT/etc/systemd/system/multi-user.target.wants/sandbox-actor-agent.service"

# Masking removes the autologin console shell so the serial port is not a root login.
$SUDO ln -sf /dev/null "$ROOT/etc/systemd/system/serial-getty@ttyS0.service"

install_dagger() {
  local engine="$ROOT$DAGGER_ROOT" container
  docker pull -q "$DAGGER_ENGINE_IMAGE" >/dev/null
  container="$(docker create "$DAGGER_ENGINE_IMAGE")"
  $SUDO mkdir -p "$engine"
  docker export "$container" | $SUDO tar -x -C "$engine"
  docker rm "$container" >/dev/null
  # Emulators for building other architectures; the guest only runs its own.
  $SUDO rm -f "$engine"/usr/local/bin/buildkit-qemu-*
  $SUDO install -D -m 0755 "$engine/usr/local/bin/dagger" "$ROOT/usr/local/libexec/dagger"

  if [ -z "$KERNEL_CONFIG" ]; then
  # The CI kernel has no xt_comment, and CNI labels every rule it adds with one. Comments
  # carry no meaning to netfilter, so the engine's iptables drops them instead.
  $SUDO mv "$engine/usr/bin/xtables-legacy-multi" "$engine/usr/bin/xtables-legacy-multi.real"
  $SUDO tee "$engine/usr/bin/xtables-legacy-multi" >/dev/null <<'SCRIPT'
#!/bin/sh
cmd="$(basename "$0")"
[ "$cmd" = xtables-legacy-multi ] && { cmd="$1"; shift; }
n=$#
while [ "$n" -gt 0 ]; do
  arg="$1"; shift; n=$((n - 1))
  if [ "$arg" = "-m" ] && [ "${1:-}" = comment ]; then
    shift; n=$((n - 1))
    continue
  fi
  if [ "$arg" = "--comment" ]; then
    shift; n=$((n - 1))
    continue
  fi
  set -- "$@" "$arg"
done
exec /usr/bin/xtables-legacy-multi.real "$cmd" "$@"
SCRIPT
  $SUDO chmod 0755 "$engine/usr/bin/xtables-legacy-multi"

  fi

  # The engine runs from its own image filesystem, as it would in a container, and its
  # socket is the one path it shares with the guest.
  $SUDO tee "$ROOT/etc/tmpfiles.d/dagger.conf" >/dev/null <<'CONF'
d /run/dagger 0755 root root -
CONF
  $SUDO tee "$ROOT/etc/systemd/system/dagger-engine.service" >/dev/null <<UNIT
[Unit]
Description=Dagger engine
After=local-fs.target

[Service]
Type=simple
RootDirectory=${DAGGER_ROOT}
MountAPIVFS=yes
BindPaths=/run/dagger
MountFlags=private
ExecStart=/usr/local/bin/dagger-engine --config /etc/dagger/engine.toml
Delegate=yes
KillMode=mixed
LimitNOFILE=1048576
UNIT

  # Not started at boot: the proxy settings it pulls through arrive with each command.
  $SUDO tee "$ROOT/usr/local/bin/dagger-engine-up" >/dev/null <<'SCRIPT'
#!/bin/sh
# Starts the Dagger engine with this command's proxy settings and waits for its socket.
set -eu
vars=""
for name in HTTP_PROXY HTTPS_PROXY NO_PROXY http_proxy https_proxy no_proxy; do
  eval "value=\${$name:-}"
  [ -n "$value" ] && vars="$vars $name=$value"
done
# shellcheck disable=SC2086
[ -n "$vars" ] && systemctl set-environment $vars
systemctl restart dagger-engine
for _ in $(seq 1 120); do
  [ -S /run/dagger/engine.sock ] && exit 0
  systemctl is-failed -q dagger-engine && break
  sleep 0.5
done
journalctl -u dagger-engine --no-pager -n 50 >&2
exit 1
SCRIPT
  $SUDO chmod 0755 "$ROOT/usr/local/bin/dagger-engine-up"

  $SUDO tee "$ROOT/usr/local/bin/dagger" >/dev/null <<'SCRIPT'
#!/bin/sh
export _EXPERIMENTAL_DAGGER_RUNNER_HOST="${_EXPERIMENTAL_DAGGER_RUNNER_HOST:-unix:///run/dagger/engine.sock}"
export DO_NOT_TRACK="${DO_NOT_TRACK:-1}"
export DAGGER_NO_NAG="${DAGGER_NO_NAG:-1}"
exec /usr/local/libexec/dagger "$@"
SCRIPT
  $SUDO chmod 0755 "$ROOT/usr/local/bin/dagger"
}

if [ "$WITH_DAGGER" -eq 1 ]; then
  install_dagger
fi

rm -f "$OUT.part"
truncate -s "$SIZE" "$OUT.part"
$SUDO mkfs.ext4 -q -F -L rootfs -d "$ROOT" "$OUT.part"
$SUDO chown "$(id -u):$(id -g)" "$OUT.part"
mv "$OUT.part" "$OUT"
echo "wrote $OUT"
