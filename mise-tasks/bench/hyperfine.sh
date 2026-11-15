#!/usr/bin/env bash
#MISE description="Wall-time and peak-RSS scenarios for a poly binary over a corpus, via hyperfine"
#USAGE arg "<poly>" help="Path to the poly binary"
#USAGE arg "<corpus>" help="Directory to lint and format"
#USAGE arg "<out_dir>" help="Directory for the per-scenario JSON exports"
# Run poly under hyperfine: startup, cold lint, cold fmt, warm lint and thread scaling.
#
# Usage: mise run bench:hyperfine POLY CORPUS OUT_DIR
# Every run uses -N -i --warmup 2 --runs 10 (-i: lint findings exit non-zero) and a POLY_CACHE_HOME
# in a fresh temp dir, so the real cache is never read or written. Peak RSS of one run per scenario is printed from /usr/bin/time.
# Compare builds by instructions retired plus wall time, the median of three invocations.
set -euo pipefail
shopt -s inherit_errexit

poly="${1:?usage: hyperfine.sh POLY CORPUS OUT_DIR}"
corpus="${2:?usage: hyperfine.sh POLY CORPUS OUT_DIR}"
out="${3:?usage: hyperfine.sh POLY CORPUS OUT_DIR}"

command -v hyperfine >/dev/null || {
  echo "hyperfine is not installed" >&2
  exit 1
}
[[ -x "$poly" ]] || {
  echo "not an executable: $poly" >&2
  exit 1
}
[[ -d "$corpus" ]] || {
  echo "not a directory: $corpus" >&2
  exit 1
}
mkdir -p "$out"

cache_home="$(mktemp -d)"
trap 'rm -rf "$cache_home"' EXIT
export POLY_CACHE_HOME="$cache_home"

if [[ "$(uname -s)" == Darwin ]]; then
  time_cmd=(/usr/bin/time -l)
  cpus="$(sysctl -n hw.ncpu)"
else
  time_cmd=(/usr/bin/time -v)
  cpus="$(nproc)"
fi
jobs_list="1,2,4,8,$cpus"

hf=(hyperfine -N -i --warmup 2 --runs 10)

peak_rss() {
  local label="$1"
  shift
  local log
  log="$(mktemp)"
  "${time_cmd[@]}" "$@" >/dev/null 2>"$log" || true
  echo "peak RSS [$label]: $(grep -E 'maximum resident set size|Maximum resident set size' "$log" | sed 's/^[[:space:]]*//')"
  rm -f "$log"
}

echo "== startup"
"${hf[@]}" --export-json "$out/startup.json" "$poly --version" "$poly config show"
peak_rss startup "$poly" --version

echo "== cold lint"
"${hf[@]}" --export-json "$out/cold-lint.json" "$poly lint --no-cache --workspace never $corpus"
peak_rss cold-lint "$poly" lint --no-cache --workspace never "$corpus"

echo "== cold fmt"
"${hf[@]}" --export-json "$out/cold-fmt.json" "$poly fmt --no-cache $corpus"
peak_rss cold-fmt "$poly" fmt --no-cache "$corpus"

echo "== warm lint"
"$poly" lint --workspace never "$corpus" >/dev/null || true
"${hf[@]}" --export-json "$out/warm-lint.json" "$poly lint --workspace never $corpus"
peak_rss warm-lint "$poly" lint --workspace never "$corpus"

echo "== thread scaling"
"${hf[@]}" --parameter-list jobs "$jobs_list" --export-json "$out/threads.json" \
  "$poly lint --no-cache --workspace never -j {jobs} $corpus"
peak_rss "threads-$cpus" "$poly" lint --no-cache --workspace never -j "$cpus" "$corpus"

echo "JSON exports written to $out"
