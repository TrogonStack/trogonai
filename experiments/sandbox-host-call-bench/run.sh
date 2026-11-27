#!/usr/bin/env bash
# Builds the guest, the AOT guest (when /dev/kvm is present) and the host harness, then prints a
# Markdown table. CI appends it to the job summary.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
cd "$here"

toolchain=1.96.0
# The revision trogon-sandbox-hyperlight-worker pins.
hl_rev=28b6c052eaf2c0eade8c73de48c530e29eccc422
# hyperlight-wasm's build script compiles its guest runtime for a custom target, which stable Rust
# only accepts with this set (see trogon-sandbox-hyperlight-worker/README.md).
export RUSTC_BOOTSTRAP=1
export RUSTUP_TOOLCHAIN=$toolchain
rustup toolchain install "$toolchain" --profile minimal -c rust-src -t wasm32-unknown-unknown >&2

out=${RUNNER_TEMP:-$(mktemp -d)}/sandbox-host-call-bench
mkdir -p "$out"

cargo build --release --manifest-path guest/Cargo.toml --target wasm32-unknown-unknown >&2
wasm=$here/guest/target/wasm32-unknown-unknown/release/bench_guest.wasm

features=()
aot=()
if [ -r /dev/kvm ] && [ -w /dev/kvm ]; then
  cargo install --locked --root "$out/tools" \
    --git https://github.com/yordis/hyperlight-wasm.git --rev "$hl_rev" hyperlight-wasm-aot >&2
  "$out/tools/bin/hyperlight-wasm-aot" compile "$wasm" "$out/guest.aot" >&2
  features=(--features hyperlight)
  aot=("$out/guest.aot")
else
  echo "no usable /dev/kvm: hyperlight rows are skipped" >&2
fi

cargo build --release --manifest-path host/Cargo.toml "${features[@]}" >&2

echo "### Sandbox host-call bench"
echo
echo "Host: $(nproc) vCPU, $(grep -m1 'model name' /proc/cpuinfo | cut -d: -f2- | xargs), kernel $(uname -r), /dev/kvm: $([ -e /dev/kvm ] && echo yes || echo no)"
echo
echo "| host | metric | n | p50 | p99 | mean |"
echo "|---|---|---|---|---|---|"
host/target/release/bench-host all "$wasm" "${aot[@]}"
