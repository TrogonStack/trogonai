#!/usr/bin/env bash
# Times a Python hello world in a Firecracker microVM, the tier hluk would replace: a cold boot, and
# a restore from a snapshot taken once Python is up, each until the guest has sent "ok" back over
# vsock. Prints a Markdown table in hluk.sh's format; a failure becomes a "failed" row. Needs x86_64
# Linux with a writable /dev/kvm, sudo (to unpack the rootfs), curl and python3.
set -uo pipefail

here=$(cd "$(dirname "$0")" && pwd)
. "$here/lib.sh"

fc_version=v1.17.0
fc_sha256=06094a1108ae9e82aa4c23a775aa92758f53f1175d422270d9d6162cb9ade558
# The kernel and Ubuntu 24.04 rootfs that trogon-sandbox-firecracker-worker's
# scripts/build-guest-rootfs.sh uses, pinned to the same digests.
ci_base=https://s3.amazonaws.com/spec.ccfc.min/firecracker-ci/v1.15/x86_64
kernel_sha256=e20e46d0c36c55c0d1014eb20576171b3f3d922260d9f792017aeff53af3d4f2
squashfs_sha256=68321e0482baeb3844dafe8a6b08a6902401a7afc41fbfd8c3d9ea08aadd244f
host="firecracker $fc_version"

work=$(mktemp -d "${RUNNER_TEMP:-/tmp}/fc.XXXX")
cd "$work" || exit 1

fetch() { # url sha256 out
  curl -fsSL -o "$3" "$1" && echo "$2  $3" | sha256sum -c --quiet
}

# The guest is the CI rootfs with agent.py as init and the hello world, as a read-only ext4 drive.
setup() {
  fetch "https://github.com/firecracker-microvm/firecracker/releases/download/$fc_version/firecracker-$fc_version-x86_64.tgz" \
    "$fc_sha256" fc.tgz || return 1
  tar -xzf fc.tgz && mv "release-$fc_version-x86_64/firecracker-$fc_version-x86_64" firecracker || return 1
  fetch "$ci_base/vmlinux-6.1.155" "$kernel_sha256" vmlinux || return 1
  fetch "$ci_base/ubuntu-24.04.squashfs" "$squashfs_sha256" rootfs.squashfs || return 1
  command -v unsquashfs >/dev/null || sudo apt-get install -y -q squashfs-tools >&2 || return 1
  sudo unsquashfs -q -d root rootfs.squashfs >&2 || return 1
  [ -x root/usr/bin/python3 ] || { echo "the CI rootfs has no /usr/bin/python3" >&2; return 1; }
  sudo install -m 0755 "$here/firecracker/agent.py" root/fc-agent &&
    echo 'print("ok")' | sudo tee root/hello.py >/dev/null &&
    truncate -s 2G rootfs.ext4 &&
    sudo mkfs.ext4 -q -F -d root rootfs.ext4 >&2 &&
    sudo chown "$(id -u)" rootfs.ext4 && sudo rm -rf root rootfs.squashfs
}

table_header "Python hello world in Firecracker ($fc_version, 1 vCPU, 256 MiB, kernel 6.1.155, Ubuntu 24.04 rootfs)"
if ! setup >&2 2>"$lib_tmp/setup.err"; then
  failed "$host" "setup" "$(tail -c 300 "$lib_tmp/setup.err")"
  exit 0
fi
"$work/firecracker" --version >&2
python3 -I -u "$here/firecracker/drive.py" --firecracker firecracker --kernel vmlinux --rootfs rootfs.ext4 \
  --work "$work" --runs "$runs" |
  while IFS=$'\t' read -r kind metric rest; do
    case $kind in
      row) row "$host" "$metric" $rest ;;
      failed) failed "$host" "$metric" "$rest" ;;
    esac
  done
