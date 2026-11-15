#!/usr/bin/env bash
#MISE description="Build poly with the profiling profile and record a samply CPU profile"
#USAGE arg "<corpus>" help="Directory or file poly runs against"
#USAGE flag "--cli" help="Profile poly-cli alone (core engines only) instead of the bundled binary"
#USAGE flag "-o --output <file>" help="Profile output" default="target/profile/poly-samply.json.gz"
#USAGE arg "[poly_args]..." var=#true help="poly arguments after --; default: lint --no-cache --workspace never"
# Record a samply profile of poly over a corpus with line tables kept (`[profile.profiling]`).
#
# Usage: mise run profile:samply CORPUS [--cli] [-- poly args]
# samply is not pinned in mise.toml (like hyperfine); install it with `cargo binstall samply` or
# `brew install samply`. The default build is the bundled binary (components/ workspace), the only one
# that runs every tier-1 engine without downloaded components. A cold run is profiled: the cache is
# pointed at a fresh temp dir. View the result with `samply load <file>`.
set -euo pipefail
shopt -s inherit_errexit

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

command -v samply >/dev/null || {
  echo "samply is not installed (cargo binstall samply)" >&2
  exit 1
}

corpus="${usage_corpus:?usage: samply.sh CORPUS}"
[[ -e "$corpus" ]] || {
  echo "no such path: $corpus" >&2
  exit 1
}
out="${usage_output:-target/profile/poly-samply.json.gz}"
mkdir -p "$(dirname "$out")"

if [[ "${usage_cli:-false}" == true ]]; then
  cargo build --profile profiling -p poly-cli
  bin="$ROOT_DIR/target/profiling/poly"
else
  cargo build --profile profiling --manifest-path components/Cargo.toml -p poly-bundled
  bin="$ROOT_DIR/components/target/profiling/poly"
fi
[[ -x "$bin" ]] || {
  echo "build produced no binary at $bin" >&2
  exit 1
}

poly_args=()
if [[ -n "${usage_poly_args:-}" ]]; then
  read -ra poly_args <<<"${usage_poly_args}"
else
  poly_args=(lint --no-cache --workspace never)
fi

cache_home="$(mktemp -d)"
trap 'rm -rf "$cache_home"' EXIT
POLY_CACHE_HOME="$cache_home" samply record --save-only -o "$out" -- "$bin" "${poly_args[@]}" "$corpus" || true
echo "profile written to $out (samply load $out)"
