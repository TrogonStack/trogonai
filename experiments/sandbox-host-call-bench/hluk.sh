#!/usr/bin/env bash
# Times Linux binaries in a Hyperlight micro-VM (hyperlight-unikraft's `hluk`, restored from its
# warm snapshot) against the same work native and in a Docker container, and prints one Markdown
# table per workload. A step that fails becomes a "failed" row, since hluk's CLI is young and a
# refusal is itself a result. Needs x86_64 Linux with a writable /dev/kvm, Docker, Node.js, curl.
set -uo pipefail

here=$(cd "$(dirname "$0")" && pwd)
. "$here/lib.sh"

version=${HLUK_VERSION:-v0.21.0}
python_image=python:3.13-slim
node_image=node:22-slim
bin=${RUNNER_TEMP:-$(mktemp -d)}/hluk-bin
export PATH=$bin:$PATH

work=$(mktemp -d)
cd "$work" || exit 1
# Independent untimed setup, overlapped; numpy and pandas are for the native agent-image rows.
docker pull -q "$python_image" >&2 &
docker pull -q "$node_image" >&2 &
{ python3 -m venv venv && venv/bin/pip install -q numpy pandas; } >&2 &

mkdir -p "$bin"
if ! curl -fsSL "https://raw.githubusercontent.com/hyperlight-dev/hyperlight-unikraft/$version/install.sh" \
  | HLUK_VERSION=$version HLUK_INSTALL_DIR=$bin sh >&2; then
  table_header "hluk"
  failed "hluk $version" "install" "see job log"
  exit 0
fi
{ hluk --version; hluk templates; hluk run --help; } >&2 || true
wait

echo 'print("ok")' >hello.py
# Standard-library modules, a mix of pure Python and C extensions, the kind a tool script imports.
modules="argparse, asyncio, csv, dataclasses, decimal, difflib, email.mime.multipart, fractions, \
http.client, ipaddress, json, logging, pathlib, statistics, tarfile, typing, unittest, \
urllib.request, uuid, xml.etree.ElementTree, zipfile"
printf 'import %s\nprint("ok")\n' "$modules" >imports.py
echo 'import numpy, pandas; print("ok")' >numpy_pandas.py
echo 'console.log("ok")' >hello.js
# No network or log driver: neither of the other rows pays for a bridge or a log file.
projects=0
docker_run=(docker run --rm --network none --log-driver none -v "$work:/w:ro")

# A project from `hluk init --template` running $script, with extra lines appended to the
# manifest's [run] table. The first run boots and saves the warm snapshot, later runs restore it.
project_rows() { # label template script [manifest lines...]
  local host="hluk $version ($1)" template=$2 script=$3
  local dir=project-$((++projects))
  shift 3
  if ! hluk init "$dir" --template "$template" >&2; then
    failed "$host" "hluk init --template $template" "see job log"
    return
  fi
  cp "$script" "$dir/$(sed -n 's/^script = "\(.*\)"/\1/p' "$dir/hluk.toml")"
  printf '%s\n' "$@" >>"$dir/hluk.toml"
  (
    cd "$dir" || exit 1
    once "$host" "first run: boot + snapshot save" hluk run
    series "$host" "run, restored from snapshot" hluk run
  )
}

# One-shot flow: a published runtime image by name, "warm by default".
runtime_rows() { # runtime script
  once "hluk $version (--runtime $1)" "first run" hluk run --runtime "$1" "$2"
  series "hluk $version (--runtime $1)" "later runs" hluk run --runtime "$1" "$2"
}

table_header 'Python hello world: print("ok")'
series "native" "python3 hello.py" python3 hello.py
series "docker ($python_image)" "docker run --rm … python hello.py" "${docker_run[@]}" "$python_image" python /w/hello.py
project_rows "project, python" python hello.py
runtime_rows python hello.py

table_header "Import-heavy Python: $(($(tr -cd , <<<"$modules" | wc -c) + 1)) stdlib modules"
series "native" "python3 imports.py" python3 imports.py
series "docker ($python_image)" "docker run --rm … python imports.py" "${docker_run[@]}" "$python_image" python /w/imports.py
# Same rootfs as above, so this restores the snapshot the hello world saved and imports per run.
series "hluk $version (--runtime python)" "restored, imports at run time" hluk run --runtime python imports.py
# warm_exec runs the imports before the snapshot, so a restore has them loaded.
project_rows "project, python, imports in warm_exec" python imports.py "warm_exec = \"import $modules\""

table_header "numpy + pandas import (hluk agent image)"
series "native (venv)" "python3 numpy_pandas.py" venv/bin/python numpy_pandas.py
# The agent image's driver imports numpy and pandas at boot, so the snapshot has them loaded.
project_rows "project, agent" agent numpy_pandas.py

table_header 'Node.js hello world: console.log("ok")'
series "native ($(node --version))" "node hello.js" node hello.js
series "docker ($node_image)" "docker run --rm … node hello.js" "${docker_run[@]}" "$node_image" node /w/hello.js
runtime_rows node hello.js

table_header "Parallel Python hello world: $par_n at a time, $par_rounds batches"
parallel_series "native" "python3 hello.py" python3 hello.py
parallel_series "docker ($python_image)" "docker run --rm … python hello.py" "${docker_run[@]}" "$python_image" python /w/hello.py
parallel_series "hluk $version (--runtime python)" "restored run" hluk run --runtime python hello.py
