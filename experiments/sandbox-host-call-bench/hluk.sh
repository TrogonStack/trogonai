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

mkdir -p "$bin"
if ! curl -fsSL "https://raw.githubusercontent.com/hyperlight-dev/hyperlight-unikraft/$version/install.sh" \
  | HLUK_VERSION=$version HLUK_INSTALL_DIR=$bin sh >&2; then
  table_header "hluk"
  failed "hluk $version" "install" "see job log"
  exit 0
fi
{ hluk --version; hluk templates; hluk run --help; } >&2 || true

work=$(mktemp -d)
cd "$work" || exit 1
docker pull -q "$python_image" >&2
docker pull -q "$node_image" >&2
# numpy and pandas for the native side of the agent-image rows.
python3 -m venv venv >&2 && venv/bin/pip install -q numpy pandas >&2

echo 'print("ok")' >hello.py
# 21 standard-library modules, a mix of pure Python and C extensions, the kind a tool script imports.
cat >imports.py <<'EOF'
import argparse, asyncio, csv, dataclasses, decimal, difflib, email.mime.multipart, fractions
import http.client, ipaddress, json, logging, pathlib, statistics, tarfile, typing, unittest
import urllib.request, uuid, xml.etree.ElementTree, zipfile
print("ok")
EOF
echo 'import numpy, pandas; print("ok")' >numpy_pandas.py
echo 'console.log("ok")' >hello.js
# No network or log driver: neither of the other rows pays for a bridge or a log file.
docker_run=(docker run --rm --network none --log-driver none -v "$work:/w:ro")

# A project from `hluk init --template`, running $script. The first run boots and saves the warm
# snapshot, later runs restore it. Extra arguments are appended to the manifest's [run] table.
hluk_project() { # dir template script [manifest lines...]
  local dir=$1 template=$2 script=$3; shift 3
  hluk init "$dir" --template "$template" >&2 || return 1
  cp "$script" "$dir/$(sed -n 's/^script = "\(.*\)"/\1/p' "$dir/hluk.toml")"
  printf '%s\n' "$@" >>"$dir/hluk.toml"
}
in_dir() { (cd "$1" && shift && "$@"); }
project_rows() { # dir label
  once "hluk $version ($2)" "first run: boot + snapshot save" in_dir "$1" hluk run
  series "hluk $version ($2)" "run, restored from snapshot" in_dir "$1" hluk run
}

table_header 'Python hello world: print("ok")'
series "native" "python3 hello.py" python3 hello.py
series "docker ($python_image)" "docker run --rm … python hello.py" "${docker_run[@]}" "$python_image" python /w/hello.py
if hluk_project hello python hello.py; then
  project_rows hello "project, python"
else
  failed "hluk $version (project, python)" "hluk init --template python" "see job log"
fi
# One-shot flow: a published runtime image by name, "warm by default".
once "hluk $version (--runtime python)" "first run" hluk run --runtime python hello.py
series "hluk $version (--runtime python)" "later runs" hluk run --runtime python hello.py

table_header "Import-heavy Python: 21 stdlib modules"
series "native" "python3 imports.py" python3 imports.py
series "docker ($python_image)" "docker run --rm … python imports.py" "${docker_run[@]}" "$python_image" python /w/imports.py
# Same rootfs as above, so this restores the snapshot the hello world saved and imports per run.
series "hluk $version (--runtime python)" "restored, imports at run time" hluk run --runtime python imports.py
# warm_exec runs the imports before the snapshot, so a restore has them loaded.
if hluk_project imports python imports.py "warm_exec = \"$(sed -n '1,3p' imports.py | tr '\n' ';')\""; then
  project_rows imports "project, python, imports in warm_exec"
else
  failed "hluk $version (project, python)" "hluk init with warm_exec" "see job log"
fi

table_header "numpy + pandas import (hluk agent image)"
series "native (venv)" "python3 numpy_pandas.py" venv/bin/python numpy_pandas.py
# The agent image's driver imports numpy and pandas at boot, so the snapshot has them loaded.
if hluk_project agent agent numpy_pandas.py; then
  project_rows agent "project, agent"
else
  failed "hluk $version (project, agent)" "hluk init --template agent" "see job log"
fi

table_header 'Node.js hello world: console.log("ok")'
series "native ($(node --version))" "node hello.js" node hello.js
series "docker ($node_image)" "docker run --rm … node hello.js" "${docker_run[@]}" "$node_image" node /w/hello.js
once "hluk $version (--runtime node)" "first run" hluk run --runtime node hello.js
series "hluk $version (--runtime node)" "later runs" hluk run --runtime node hello.js

table_header "Parallel Python hello world: $par_n at a time, $par_rounds batches"
parallel_series "native" "python3 hello.py" python3 hello.py
parallel_series "docker ($python_image)" "docker run --rm … python hello.py" "${docker_run[@]}" "$python_image" python /w/hello.py
parallel_series "hluk $version (--runtime python)" "restored run" hluk run --runtime python hello.py
