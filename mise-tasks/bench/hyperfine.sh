#!/usr/bin/env bash
#MISE description="Wall-time and peak-RSS scenarios for one or two poly binaries over a corpus, via hyperfine"
#USAGE arg "<corpus>" help="Directory to lint and format"
#USAGE arg "<out_dir>" help="Directory for the per-binary JSON exports and summary.md"
#USAGE flag "--old <poly>" help="Baseline poly binary"
#USAGE flag "--new <poly>" help="Candidate poly binary (the only binary when --old is absent)"
#USAGE flag "--warmup <n>" help="Warmup runs per scenario (env POLY_BENCH_WARMUP, default 3)"
#USAGE flag "--runs <n>" help="Timed runs per scenario (env POLY_BENCH_RUNS, default 30)"
#USAGE flag "--allow-missing-components" help="Benchmark even when the binary cannot load its downloadable components"
# Run poly under hyperfine: startup, cold lint, cold fmt, warm lint, warm fmt, touched-but-identical
# lint, pre-commit hooks (when the corpus has a [hooks] config) and thread scaling.
#
# Usage: mise run bench:hyperfine CORPUS OUT_DIR --new target/release/poly [--old /path/to/old/poly]
# Every run uses -N -i (lint findings exit non-zero) and a POLY_CACHE_HOME in a fresh temp dir, so the
# real cache is never read or written. With both binaries a side-by-side markdown table (mean, sigma,
# delta) is written to OUT_DIR/summary.md. The corpus is scratch: the touched scenario rewrites every
# file's mtime.
#
# A binary that cannot load its components (a core build with no embedded lock, a missing dev lock)
# skips the tier-1 engines silently and benchmarks nothing useful, so the run is refused unless
# --allow-missing-components is given. Build a binary with every engine linked: docs/benchmarking.md.
set -euo pipefail
shopt -s inherit_errexit

usage="usage: hyperfine.sh CORPUS OUT_DIR [--old POLY] --new POLY [--warmup N] [--runs N] [--allow-missing-components]"
old="" new="" corpus="" out="" allow_missing=false
warmup="${POLY_BENCH_WARMUP:-3}"
runs="${POLY_BENCH_RUNS:-30}"
positional=()
while (($#)); do
  case "$1" in
  --old)
    old="${2:?--old needs a path}"
    shift
    ;;
  --new)
    new="${2:?--new needs a path}"
    shift
    ;;
  --warmup)
    warmup="${2:?--warmup needs a number}"
    shift
    ;;
  --runs)
    runs="${2:?--runs needs a number}"
    shift
    ;;
  --allow-missing-components) allow_missing=true ;;
  -*)
    echo "unknown flag: $1 ($usage)" >&2
    exit 2
    ;;
  *) positional+=("$1") ;;
  esac
  shift
done
[[ ${#positional[@]} -eq 2 ]] || {
  echo "$usage" >&2
  exit 2
}
corpus="${positional[0]}"
out="${positional[1]}"
[[ -n "$new" ]] || {
  echo "--new is required ($usage)" >&2
  exit 2
}

command -v hyperfine >/dev/null || {
  echo "hyperfine is not installed" >&2
  exit 1
}
for bin in $old $new; do
  [[ -x "$bin" ]] || {
    echo "not an executable: $bin" >&2
    exit 1
  }
done
[[ -d "$corpus" ]] || {
  echo "not a directory: $corpus" >&2
  exit 1
}
corpus="$(cd "$corpus" && pwd)"
mkdir -p "$out"
out="$(cd "$out" && pwd)"

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

hf=(hyperfine -N -i --warmup "$warmup" --runs "$runs")

check_components() {
  local poly="$1" log
  log="$(mktemp)"
  "$poly" lint --no-cache --workspace never "$corpus" >"$log" 2>&1 || true
  if grep -q "embeds no component lock" "$log"; then
    if [[ "$allow_missing" == true ]]; then
      echo "warning: $poly embeds no component lock; tier-1 engines are not exercised" >&2
    else
      echo "error: $poly prints 'embeds no component lock' for $corpus: its downloadable engines cannot load," >&2
      echo "so the numbers would skip them. Build a component-enabled binary (docs/benchmarking.md) or" >&2
      echo "pass --allow-missing-components." >&2
      rm -f "$log"
      exit 1
    fi
  fi
  rm -f "$log"
}

peak_rss() {
  local file="$1" label="$2"
  shift 2
  local log
  log="$(mktemp)"
  "${time_cmd[@]}" "$@" >/dev/null 2>"$log" || true
  echo "peak RSS [$label]: $(grep -E 'maximum resident set size|Maximum resident set size' "$log" | sed 's/^[[:space:]]*//')" |
    tee -a "$file"
  rm -f "$log"
}

run_binary() {
  local label="$1" poly="$2" dir="$out/$1"
  mkdir -p "$dir"
  : >"$dir/rss.txt"
  echo "######## $label: $poly"
  check_components "$poly"

  echo "== startup"
  "${hf[@]}" --export-json "$dir/startup.json" "$poly --version" "$poly config show"
  peak_rss "$dir/rss.txt" startup "$poly" --version

  echo "== cold lint"
  "${hf[@]}" --export-json "$dir/cold-lint.json" "$poly lint --no-cache --workspace never $corpus"
  peak_rss "$dir/rss.txt" cold-lint "$poly" lint --no-cache --workspace never "$corpus"

  echo "== cold fmt"
  "${hf[@]}" --export-json "$dir/cold-fmt.json" "$poly fmt --no-cache $corpus"
  peak_rss "$dir/rss.txt" cold-fmt "$poly" fmt --no-cache "$corpus"

  echo "== warm lint"
  rm -rf "${cache_home:?}"/*
  "$poly" lint --workspace never "$corpus" >/dev/null 2>&1 || true
  "${hf[@]}" --export-json "$dir/warm-lint.json" "$poly lint --workspace never $corpus"
  peak_rss "$dir/rss.txt" warm-lint "$poly" lint --workspace never "$corpus"

  echo "== warm fmt"
  "$poly" fmt "$corpus" >/dev/null 2>&1 || true
  "${hf[@]}" --export-json "$dir/warm-fmt.json" "$poly fmt $corpus"
  peak_rss "$dir/rss.txt" warm-fmt "$poly" fmt "$corpus"

  # Same bytes, new mtime on every file: the stat fast path misses and the content digest decides.
  echo "== touched-but-identical lint"
  "$poly" lint --workspace never "$corpus" >/dev/null 2>&1 || true
  "${hf[@]}" --prepare "find $corpus -type f -not -path '*/.git/*' -exec touch {} +" \
    --export-json "$dir/touched-lint.json" "$poly lint --workspace never $corpus"

  if [[ -f "$corpus/poly.toml" ]] && grep -q '^\[hooks' "$corpus/poly.toml"; then
    echo "== pre-commit hooks"
    "${hf[@]}" --export-json "$dir/hooks.json" \
      "bash -c 'cd $corpus && POLY_TRUST=1 $poly hooks run pre-commit --all-files'"
  else
    echo "== pre-commit hooks: skipped, $corpus/poly.toml has no [hooks]"
  fi

  echo "== thread scaling"
  "${hf[@]}" --parameter-list jobs "$jobs_list" --export-json "$dir/threads.json" \
    "$poly lint --no-cache --workspace never -j {jobs} $corpus"
  peak_rss "$dir/rss.txt" "threads-$cpus" "$poly" lint --no-cache --workspace never -j "$cpus" "$corpus"
}

[[ -z "$old" ]] || run_binary old "$old"
run_binary new "$new"

summarize() {
  python3 -I - "$out" "${old:-}" "$new" "$corpus" <<'PY'
import json, math, pathlib, sys

out, old_bin, new_bin, corpus = pathlib.Path(sys.argv[1]), sys.argv[2], sys.argv[3], sys.argv[4]


def stats(r):
    """Wall-clock stats of one result: hyperfine 2 nests them under `summary`, 1.x keeps them flat."""
    wall = r.get("summary", {}).get("time_wall_clock", r)
    inst = r.get("summary", {}).get("instructions") or {}
    return {
        "mean": wall["mean"],
        "stddev": wall["stddev"],
        "min": wall["min"],
        "max": wall["max"],
        "instr": inst.get("mean"),
    }


def load(label, binary):
    rows = {}
    for path in sorted((out / label).glob("*.json")):
        for r in json.loads(path.read_text())["results"]:
            name = r["command"].replace(binary, "poly").replace(corpus, "<corpus>")
            if r.get("parameters"):
                jobs = r["parameters"]["jobs"]
                name = "threads -j " + str(jobs["value"] if isinstance(jobs, dict) else jobs)
            rows[f"{path.stem}: {name}"] = stats(r)
    return rows


def cell(r):
    return f"{r['mean'] * 1000:.1f} ± {r['stddev'] * 1000:.1f} ms"


def instr_delta(o, n):
    if o["instr"] and n["instr"]:
        return f"{(n['instr'] - o['instr']) / o['instr'] * 100:+.1f}%"
    return "n/a"


new = load("new", new_bin)
lines = []
if old_bin:
    old = load("old", old_bin)
    lines += [
        "| scenario | old (mean ± σ) | new (mean ± σ) | wall delta | | instructions delta |",
        "|---|---|---|---|---|---|",
    ]
    for key in new:
        if key not in old:
            continue
        o, n = old[key], new[key]
        delta = (n["mean"] - o["mean"]) / o["mean"] * 100
        noise = math.hypot(o["stddev"], n["stddev"])
        mark = "within noise" if abs(n["mean"] - o["mean"]) < noise else ("faster" if delta < 0 else "slower")
        lines.append(f"| {key} | {cell(o)} | {cell(n)} | {delta:+.1f}% | {mark} | {instr_delta(o, n)} |")
else:
    lines += ["| scenario | mean ± σ | min | max |", "|---|---|---|---|"]
    for key, n in new.items():
        lines.append(f"| {key} | {cell(n)} | {n['min'] * 1000:.1f} ms | {n['max'] * 1000:.1f} ms |")
text = "\n".join(lines) + "\n"
(out / "summary.md").write_text(text)
print(text)
PY
}
summarize
echo "JSON exports and summary.md written to $out"
