#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<USAGE
Usage: $0 --source-dir DIR --base-config PATH --out-dir DIR [options]

Build an x86_64 Amazon Linux microVM kernel with built-in container networking.
Use a clean source checkout matching a Firecracker guest kernel configuration.

  --source-dir DIR     Linux sources containing Makefile and scripts/kconfig
  --base-config PATH   matching Firecracker x86_64 guest configuration
  --out-dir DIR        separate, disposable kernel build directory
  --jobs N             compiler concurrency (default: 2)
  --timeout-seconds N  total configure/build budget (default: 900)
  --cross-compile PFX  cross compiler prefix, e.g. x86_64-linux-gnu-
USAGE
}

SOURCE=""
BASE=""
OUTPUT=""
JOBS=2
DEADLINE=900
CROSS=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --source-dir|--base-config|--out-dir|--jobs|--timeout-seconds|--cross-compile)
      [ "$#" -ge 2 ] || { usage >&2; exit 2; }
      case "$1" in
        --source-dir) SOURCE="$2" ;;
        --base-config) BASE="$2" ;;
        --out-dir) OUTPUT="$2" ;;
        --jobs) JOBS="$2" ;;
        --timeout-seconds) DEADLINE="$2" ;;
        --cross-compile) CROSS="$2" ;;
      esac
      shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) usage >&2; exit 2 ;;
  esac
done
[ -n "$SOURCE" ] && [ -n "$BASE" ] && [ -n "$OUTPUT" ] || { usage >&2; exit 2; }
[[ "$JOBS" =~ ^[0-9]+$ ]] && (( JOBS >= 1 && JOBS <= 64 )) || exit 2
[[ "$DEADLINE" =~ ^[0-9]+$ ]] && (( DEADLINE >= 1 && DEADLINE <= 86400 )) || exit 2
for tool in make timeout realpath readelf sha256sum; do
  command -v "$tool" >/dev/null || { echo "missing required tool: $tool" >&2; exit 1; }
done
SOURCE="$(realpath -e -- "$SOURCE")"
BASE="$(realpath -e -- "$BASE")"
mkdir -p -- "$OUTPUT"
OUTPUT="$(realpath -e -- "$OUTPUT")"
[ "$OUTPUT" != "$SOURCE" ] && [[ "$OUTPUT/" != "$SOURCE/"* ]] || {
  echo 'build output must be outside the kernel source tree' >&2; exit 1;
}
[ -f "$SOURCE/Makefile" ] && [ -x "$SOURCE/scripts/kconfig/merge_config.sh" ] || {
  echo 'source directory is not a Linux kernel checkout' >&2; exit 1;
}
FRAGMENT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)/container-kernel.config"

# Configuration normalization can silently drop symbols with unsatisfied dependencies.
# Reject that result before spending time compiling an unusable guest kernel.
export SOURCE BASE OUTPUT JOBS CROSS FRAGMENT
timeout -k 10s "${DEADLINE}s" bash -euo pipefail -c '
  export ARCH=x86_64 CROSS_COMPILE="$CROSS" KCONFIG_CONFIG="$OUTPUT/.config"
  cd "$OUTPUT"
  "$SOURCE/scripts/kconfig/merge_config.sh" -m -O "$OUTPUT" "$BASE" "$FRAGMENT"
  make -C "$SOURCE" O="$OUTPUT" olddefconfig
  missing=0
  while IFS= read -r setting; do
    if ! grep -Fxq -- "$setting" "$OUTPUT/.config"; then
      echo "kernel configuration dropped required setting: $setting" >&2
      missing=1
    fi
  done < "$FRAGMENT"
  [ "$missing" -eq 0 ] || exit 1
  make -C "$SOURCE" O="$OUTPUT" -j "$JOBS" vmlinux
'
LC_ALL=C readelf -h "$OUTPUT/vmlinux" | grep -Eq 'Machine:.*Advanced Micro Devices X86-64' || {
  echo 'kernel output is not an x86_64 ELF image' >&2; exit 1;
}
sha256sum "$OUTPUT/vmlinux" "$OUTPUT/.config"
