#!/usr/bin/env bash
#MISE description="Build poly with the dhat-heap feature and write a heap profile (dhat-heap.json)"
#USAGE arg "<corpus>" help="Directory or file poly runs against"
#USAGE flag "-o --output <dir>" help="Directory for dhat-heap.json" default="target/profile"
#USAGE arg "[poly_args]..." var=#true help="poly arguments after --; default: lint --no-cache --workspace never"
# Heap-profile poly over a corpus with dhat (`dhat-heap` feature on poly-cli).
#
# Usage: mise run profile:dhat CORPUS [-- poly args]
# Builds poly-cli (core engines only; the bundled binary has its own main and no dhat hook) with
# `--profile profiling --features dhat-heap`. dhat writes dhat-heap.json when main returns; a path that
# exits through process::exit (broken pipe, SIGINT) writes nothing. View it at
# https://nnethercote.github.io/dh_view/dh_view.html (the file stays local).
set -euo pipefail
shopt -s inherit_errexit

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

corpus="${usage_corpus:?usage: dhat.sh CORPUS}"
[[ -e "$corpus" ]] || {
  echo "no such path: $corpus" >&2
  exit 1
}
out="${usage_output:-target/profile}"
mkdir -p "$out"
out="$(cd "$out" && pwd)"
corpus="$(cd "$(dirname "$corpus")" && pwd)/$(basename "$corpus")"

cargo build --profile profiling -p poly-cli --features dhat-heap
bin="$ROOT_DIR/target/profiling/poly"

poly_args=()
if [[ -n "${usage_poly_args:-}" ]]; then
  read -ra poly_args <<<"${usage_poly_args}"
else
  poly_args=(lint --no-cache --workspace never)
fi

cache_home="$(mktemp -d)"
trap 'rm -rf "$cache_home"' EXIT
rm -f "$out/dhat-heap.json"
(cd "$out" && POLY_CACHE_HOME="$cache_home" "$bin" "${poly_args[@]}" "$corpus") || true
[[ -s "$out/dhat-heap.json" ]] || {
  echo "no dhat-heap.json written to $out" >&2
  exit 1
}
echo "heap profile written to $out/dhat-heap.json"
