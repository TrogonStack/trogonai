#!/usr/bin/env bash
# Times a Python hello world as a Linux binary in a Hyperlight micro-VM (hyperlight-unikraft's
# `hluk`, restored from its snapshot) against native Python and a Docker container, and prints
# Markdown table rows. A step that fails becomes a "failed" row, since hluk's CLI is young and a
# refusal is itself a result. Needs x86_64 Linux with a writable /dev/kvm, Docker, and curl.
set -uo pipefail

runs=${HLUK_RUNS:-30}
version=${HLUK_VERSION:-v0.21.0}
python_image=python:3.13-slim
bin=${RUNNER_TEMP:-$(mktemp -d)}/hluk-bin
export PATH=$bin:$PATH

row() { # host metric samples-in-µs...
  local host=$1 metric=$2; shift 2
  printf '%s\n' "$@" | sort -n | awk -v host="$host" -v metric="$metric" '
    function fmt(us) { return us < 1000 ? sprintf("%.0f µs", us) : sprintf("%.2f ms", us / 1000) }
    { v[NR] = $1; sum += $1 }
    END {
      p50 = v[int(0.5 * NR + 0.999)]; p99 = v[int(0.99 * NR + 0.999)]
      printf "| %s | %s | %d | %s | %s | %s |\n", host, metric, NR, fmt(p50), fmt(p99), fmt(sum / NR)
    }'
}
failed() { echo "| $1 | $2 | — | failed: $3 | | |"; }

# Runs "$@" $runs times (output discarded) and prints one row; the first failure ends the series.
series() { # host metric cmd...
  local host=$1 metric=$2; shift 2
  local samples=() start end
  for _ in $(seq "$runs"); do
    start=$(date +%s%N)
    if ! "$@" >/dev/null 2>"$bin/err"; then
      failed "$host" "$metric" "$(tr '|\n' '  ' <"$bin/err" | cut -c1-160)"
      return
    fi
    end=$(date +%s%N)
    samples+=($(((end - start) / 1000)))
  done
  row "$host" "$metric" "${samples[@]}"
}

once() { # host metric cmd...: a single timed run
  local host=$1 metric=$2 start end; shift 2
  start=$(date +%s%N)
  if "$@" >/dev/null 2>"$bin/err"; then
    end=$(date +%s%N)
    row "$host" "$metric" $(((end - start) / 1000))
  else
    failed "$host" "$metric" "$(tr '|\n' '  ' <"$bin/err" | cut -c1-160)"
  fi
}

mkdir -p "$bin"
curl -fsSL "https://raw.githubusercontent.com/hyperlight-dev/hyperlight-unikraft/$version/install.sh" \
  | HLUK_VERSION=$version HLUK_INSTALL_DIR=$bin sh >&2
{ hluk --version; hluk --help; hluk run --help; } >&2 || true

work=$(mktemp -d)
cd "$work" || exit 1
docker pull -q "$python_image" >&2

echo
echo "| host | metric | n | p50 | p99 | mean |"
echo "|---|---|---|---|---|---|"
echo 'print("ok")' >hello.py
series "native" "python3 hello.py" python3 hello.py
series "docker ($python_image)" "docker run --rm … python hello.py" \
  docker run --rm -v "$work:/w:ro" "$python_image" python /w/hello.py

# Project flow: `hluk init` pulls the Python rootfs; the first run boots and saves a snapshot,
# later runs restore it.
if hluk init hello --template python >&2; then
  cp hello.py hello/main.py
  (
    cd hello || exit 1
    ls -la >&2
    once "hluk $version (project)" "first run: boot + snapshot save" hluk run
    series "hluk $version (project)" "run, restored from snapshot" hluk run
  )
else
  failed "hluk $version (project)" "hluk init --template python" "see job log"
fi

# One-shot flow: a published runtime image by name, "warm by default".
once "hluk $version (--runtime)" "first run" hluk run --runtime python hello.py
series "hluk $version (--runtime)" "later runs" hluk run --runtime python hello.py
