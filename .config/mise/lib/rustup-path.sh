# shellcheck shell=bash
# Sourced by the tasks that run cargo dylint. mise puts its own cargo (command
# wrappers and shims) ahead of the rustup proxies on PATH, and that cargo stays
# on the mise Rust toolchain. Dylint builds the repo lint library with whatever
# `cargo` PATH resolves, so it gets the stable toolchain instead of the pinned
# nightly in dylints/trogonai_dylint/rust-toolchain.toml and fails with
# "can't find crate for rustc_driver".

# rustup_proxy_path
#
# Prints PATH with the mise command wrapper and shim directories removed and
# the rustup proxy directory first, so `cargo` resolves to the rustup proxy and
# honours the toolchain file. Prints PATH unchanged when rustup's proxies are
# not where CARGO_HOME says.
rustup_proxy_path() {
  local proxies="${CARGO_HOME:-$HOME/.cargo}/bin"
  if [[ ! -x "$proxies/rustup" ]]; then
    printf '%s\n' "$PATH"
    return
  fi
  local entry kept=()
  local IFS=:
  for entry in $PATH; do
    case "$entry" in
      */mise/command-wrappers/* | */mise/command-wrappers | */mise/shims | "$proxies") ;;
      *) kept+=("$entry") ;;
    esac
  done
  printf '%s\n' "$proxies:${kept[*]}"
}
