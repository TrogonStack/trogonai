#!/usr/bin/env bash
# Builds the guest, the AOT guest (when /dev/kvm is usable) and the host harness, then prints a
# Markdown table. CI appends it to the job summary. REQUIRE_HYPERLIGHT=1 fails instead of skipping
# the Hyperlight rows when there is no usable /dev/kvm.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
cd "$here"

# The repository's toolchain, and the hyperlight-wasm revision host/Cargo.toml pins (the AOT
# compiler must come from the same revision as the runtime).
toolchain=$(sed -n 's/^channel = "\(.*\)"/\1/p' ../../rsworkspace/rust-toolchain.toml)
hl_rev=$(sed -n 's/^hyperlight-wasm = .*rev = "\([0-9a-f]*\)".*/\1/p' host/Cargo.toml)
export RUSTUP_TOOLCHAIN=$toolchain
rustup toolchain install "$toolchain" --profile minimal -c rust-src -t wasm32-unknown-unknown >&2

if [ -r /dev/kvm ] && [ -w /dev/kvm ]; then kvm=yes; else kvm=no; fi
if [ "$kvm" = no ] && [ "${REQUIRE_HYPERLIGHT:-0}" = 1 ]; then
  echo "REQUIRE_HYPERLIGHT=1 but /dev/kvm is missing or not read-writable" >&2
  exit 1
fi

cargo build --locked --release --manifest-path guest/Cargo.toml --target wasm32-unknown-unknown >&2
wasm=$here/guest/target/wasm32-unknown-unknown/release/bench_guest.wasm

host_build=(cargo build --locked --release --manifest-path host/Cargo.toml)
aot=()
if [ "$kvm" = yes ]; then
  # Lives under host/target so the CI cache keeps it between runs.
  tools=$here/host/target/tools
  [ -x "$tools/bin/hyperlight-wasm-aot" ] || cargo install --locked --root "$tools" \
    --git https://github.com/yordis/hyperlight-wasm.git --rev "$hl_rev" hyperlight-wasm-aot >&2
  "$tools/bin/hyperlight-wasm-aot" compile "$wasm" "$here/host/target/guest.aot" >&2
  aot=("$here/host/target/guest.aot")
  # hyperlight-wasm's build script writes its guest runtime under target/ outside cargo's
  # tracking; a restored cache can keep the script's fingerprint but drop that file, and cargo
  # then never reruns the script. Clean the package so the script runs again.
  [ -f host/target/hyperlight-wasm-runtime/x86_64-hyperlight-none/release/hyperlight-wasm-runtime ] \
    || cargo clean --release --manifest-path host/Cargo.toml -p hyperlight-wasm >&2
  # hyperlight-wasm's build script compiles its guest runtime for a custom target, which stable
  # Rust only accepts with RUSTC_BOOTSTRAP (see trogon-sandbox-hyperlight-worker/README.md).
  host_build=(env RUSTC_BOOTSTRAP=1 "${host_build[@]}" --features hyperlight)
else
  echo "no usable /dev/kvm: hyperlight rows are skipped" >&2
fi
"${host_build[@]}" >&2

echo "### Sandbox host-call bench"
echo
echo "Host: $(nproc) vCPU, $(grep -m1 'model name' /proc/cpuinfo | cut -d: -f2- | xargs), kernel $(uname -r), usable /dev/kvm: $kvm"
echo
echo "| host | metric | n | p50 | p99 | mean |"
echo "|---|---|---|---|---|---|"
host/target/release/bench-host all "$wasm" "${aot[@]}"
