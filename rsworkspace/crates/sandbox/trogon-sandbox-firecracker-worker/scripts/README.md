# Guest rootfs

`build-guest-rootfs.sh` turns the Firecracker CI Ubuntu 24.04 squashfs into an ext4 rootfs that
starts `sandbox-actor-agent` on a guest vsock port at boot. Run it on a Linux host with sudo,
passing a musl-linked agent built with
`mise exec -- cargo build --release -p sandbox-actor-agent --target x86_64-unknown-linux-musl`.
For an arm64 guest (for example a KVM check on an Apple silicon Linux VM), build the agent for
`aarch64-unknown-linux-musl` and pass `--arch aarch64`; the script then fetches the aarch64
Firecracker CI squashfs and kernel, pinned to their own digests.

```bash
./build-guest-rootfs.sh --agent-bin path/to/sandbox-actor-agent --out guest/rootfs.ext4 \
  --size 2G --kernel-out guest/vmlinux
```

The downloaded squashfs and kernel are pinned to SHA-256 digests in the script and verified on
every run, cached copies included; a mismatch deletes the file and fails the build. When bumping
the CI artifact version, update the URL and both digests together. A squashfs passed with
`--squashfs` is the caller's own, so the script refuses it unless you also give its expected
digest with `--squashfs-sha256 <hex>` (or `SANDBOX_SQUASHFS_SHA256`). The check runs before
anything is unpacked, a mismatch fails the build, and the file is left untouched.

The unit writes `/run/ate/actor-id` before the agent starts, from `ate.actor_id=<id>` on the
kernel command line, falling back to the machine id. The host reaches the agent through the
hybrid vsock Unix socket: send `CONNECT 8110\n`, read `OK <port>`, then speak HTTP.


## Container kernel

`build-guest-kernel.sh` takes an existing Amazon Linux microVM source checkout and
its matching Firecracker x86_64 base configuration. It uses the upstream kernel
configuration merger and `make`, then rejects required settings lost during
`olddefconfig`. The fragment builds container networking, `xt_comment`, nftables,
namespace and cgroup support into the image, so the guest does not need matching
kernel modules.

Run on Linux with a native or cross compiler, `make`, `flex`, `bison`, `bc`, ELF and
OpenSSL development libraries, binutils and GNU `timeout`:

```sh
./build-guest-kernel.sh \
  --source-dir /work/linux \
  --base-config /work/firecracker/resources/guest_configs/microvm-kernel-ci-x86_64-6.1.config \
  --out-dir /work/guest-kernel \
  --jobs 2 --timeout-seconds 900
```

For an ARM64 build host, add `--cross-compile x86_64-linux-gnu-`. The output must
be outside the kernel source directory. It contains `vmlinux` and the normalized
`.config`; the script prints their SHA-256 digests. The build runs within its
configured deadline. Keep the source revision, base configuration and output
hashes with deployment evidence.

To build a Dagger rootfs for that kernel, pass its normalized configuration as
`--kernel-config /work/guest-kernel/.config` to `build-guest-rootfs.sh`. This keeps
the engine's native iptables binary. Omit `--kernel-out`, which downloads the
older CI kernel that still needs the compatibility wrapper. Boot with the
matching custom `vmlinux`; configuration validation alone does not prove a
container pipeline works in a guest.

Inside Docker, use a Linux filesystem or named volume for `--cache-dir`. The
rootfs population step needs nofollow symlink xattrs; a macOS shared mount can
return `ENOENT` for a dangling symlink and make `mkfs.ext4 -d` fail. The finished
ext4 image can still be written to a shared output directory.
