# Sourced by hluk.sh and firecracker.sh: Markdown table rows in host/src/main.rs's format, and
# timing loops whose failures become "failed" rows rather than ending the script.

runs=${RUNS:-30}
lib_tmp=$(mktemp -d)
trap 'rm -rf "$lib_tmp"' EXIT

table_header() { # title
  printf '\n#### %s\n\n| host | metric | n | p50 | p99 | mean |\n|---|---|---|---|---|---|\n' "$1"
}

# Nearest-rank percentiles and the same units as host/src/main.rs's `percentile` and `fmt`.
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
failed() { echo "| $1 | $2 | — | failed: $(tr '|\n' '  ' <<<"$3" | cut -c1-160) | | |"; }

# Runs "$@" $runs times (output discarded) and prints one row; the first failure ends the series.
# Timestamps come from $EPOCHREALTIME (microseconds, no `date` process inside the sample).
series() { # host metric cmd...
  local host=$1 metric=$2; shift 2
  local samples=() start end
  for _ in $(seq "$runs"); do
    start=${EPOCHREALTIME/./}
    if ! "$@" >/dev/null 2>"$lib_tmp/err"; then
      failed "$host" "$metric" "$(<"$lib_tmp/err")"
      return
    fi
    end=${EPOCHREALTIME/./}
    samples+=($((end - start)))
  done
  row "$host" "$metric" "${samples[@]}"
}

once() { runs=1 series "$@"; }

# Starts "$@" $par_n at a time, waits for the batch, $par_rounds times. Prints the per-run latency
# under that load and the throughput over all batches (barrier waits included).
par_n=${PAR_N:-8}
par_rounds=${PAR_ROUNDS:-5}
parallel_series() { # host metric cmd...
  local host=$1 metric=$2; shift 2
  local dir start end
  dir=$(mktemp -d "$lib_tmp/par.XXXX")
  start=${EPOCHREALTIME/./}
  for r in $(seq "$par_rounds"); do
    for i in $(seq "$par_n"); do
      (
        s=${EPOCHREALTIME/./}
        "$@" >/dev/null 2>"$dir/err.$r.$i" && echo $((${EPOCHREALTIME/./} - s)) >"$dir/t.$r.$i"
      ) &
    done
    wait
  done
  end=${EPOCHREALTIME/./}
  local total=$((par_n * par_rounds)) ok
  ok=$(find "$dir" -name 't.*' | wc -l)
  if [ "$ok" -ne "$total" ]; then
    failed "$host" "$metric, $par_n concurrent" "$ok/$total ok: $(cat "$dir"/err.* | head -c 400)"
    return
  fi
  row "$host" "$metric, per run with $par_n concurrent" $(cat "$dir"/t.*)
  awk -v host="$host" -v metric="$metric" -v n="$par_n" -v total="$total" -v us=$((end - start)) \
    'BEGIN { printf "| %s | %s, throughput in batches of %d | %d | %.1f runs/s | | |\n", host, metric, n, total, total / (us / 1e6) }'
}
