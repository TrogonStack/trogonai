#!/usr/bin/env bash
# Records the TypeScript settings goldens on the host the job runs on, and a widened scan of each boundary, into OUT.
# usage: capture.sh OUT
set -euo pipefail
out=$1
here=$(cd "$(dirname "$0")" && pwd)
work=$(mktemp -d)
version=0.3.323
integrity=sha512-GXIczMzzceNWB+bIImK59hWOQoBWZtSw+AESPc/qbAWmmHM8Wrqb5KvTwWZ07CmjyC/KcPC47nOLxxyHlWqKPA==
mkdir -p "$out" "$work/pkg"
(cd "$work" && npm pack --silent "@anthropic-ai/claude-agent-sdk@$version" > pack.txt)
archive="$work/$(tail -n 1 "$work/pack.txt")"
actual="sha512-$(openssl dgst -sha512 -binary "$archive" | openssl base64 -A)"
if [ "$actual" != "$integrity" ]; then
  echo "the package does not match its pinned integrity" >&2
  exit 1
fi
tar -xzf "$archive" -C "$work/pkg"
package="$work/pkg/package"
node=$(command -v node)
platform=$(python3 -I -c 'import platform, sys; print(f"{sys.platform}-{platform.machine()}")')
python3 -I "$here/widen.py" "$here/ts_settings_depth_goldens.py" "$here/scan_depth.py" merge,index_compile_inside,copy_104
python3 -I "$here/widen.py" "$here/ts_managed_settings_goldens.py" "$here/scan_managed.py" origin_drop,machine_drop,partial_drop
for run in 1 2; do
  python3 -I "$here/ts_settings_depth_goldens.py" "$node" "$package" "$out/ts_settings_depth_${platform}_run${run}.json"
  python3 -I "$here/ts_managed_settings_goldens.py" "$node" "$package" "$out/ts_managed_settings_${platform}_run${run}.json"
done
for run in 1 2 3; do
  python3 -I "$here/scan_depth.py" "$node" "$package" "$out/scan_depth_${platform}_run${run}.json"
  python3 -I "$here/scan_managed.py" "$node" "$package" "$out/scan_managed_${platform}_run${run}.json"
done
