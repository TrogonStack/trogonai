#!/bin/sh
set -eu

umoci_bin=$1
mkfs_bin=$2
layout=$3
disk_bytes=$4
build_dir=$5

truncate -s "$disk_bytes" "$build_dir/scratch.ext4"
"$mkfs_bin" -q -F "$build_dir/scratch.ext4"
mkdir "$build_dir/mount"
mount -o loop,nodev,nosuid,noexec "$build_dir/scratch.ext4" "$build_dir/mount"
trap 'umount "$build_dir/mount"' EXIT

root="$build_dir/mount/rootfs"
"$umoci_bin" raw unpack --image "$layout:source" "$root"
reserved="$root/.sandbox"
if [ -L "$reserved" ] || { [ -e "$reserved" ] && [ ! -d "$reserved" ]; }; then
  echo 'image .sandbox path must be a real directory' >&2
  exit 1
fi
rm -rf -- "$reserved"
mkdir "$reserved"
cp -- "$build_dir/agent" "$reserved/agent"
cp -- "$build_dir/init" "$reserved/init"
chmod 0755 "$reserved" "$reserved/agent" "$reserved/init"

truncate -s "$disk_bytes" "$build_dir/rootfs.ext4"
"$mkfs_bin" -q -F -L rootfs -d "$root" "$build_dir/rootfs.ext4"
umount "$build_dir/mount"
trap - EXIT
rm -f "$build_dir/scratch.ext4" "$build_dir/agent" "$build_dir/init"
rmdir "$build_dir/mount"
