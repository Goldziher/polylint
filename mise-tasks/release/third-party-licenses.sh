#!/usr/bin/env bash
#MISE description="Write THIRD-PARTY-LICENSES.md: every crate in the five workspaces plus ATTRIBUTIONS.md"
#USAGE arg "[out]" help="Output file (default: THIRD-PARTY-LICENSES.md)"
set -euo pipefail

# mise run release:third-party-licenses [out]
#
# The release archives, npm platform packages and wheels ship the binary; the binary statically links
# MIT/Apache/BSD crates and data under other licences, whose terms ask for their notices to travel with
# it. This lists every non-workspace crate resolved by `cargo metadata --locked` in the root workspace and
# the four components workspaces (name, version, licence expression, repository), sorted and de-duplicated
# so two runs on one lockfile are byte-identical, then appends ATTRIBUTIONS.md verbatim: it carries the
# vendored-code credits and the tsrs NOTICE, which must accompany the binary.
#
# The table lists licence *expressions* as declared by each crate; it is not a legal review. Licence texts
# stay in each crate's own source. A crate declaring only a licence file shows `see licence file`.
# Needs cargo and jq.

root="$(cd "$(dirname "$0")/../.." && pwd)"
out="${1:-THIRD-PARTY-LICENSES.md}"
[[ "$out" == /* ]] || out="$PWD/$out"

for tool in cargo jq; do
  command -v "$tool" > /dev/null || {
    echo "error: $tool is required" >&2
    exit 1
  }
done
[[ -f "$root/ATTRIBUTIONS.md" ]] || {
  echo "error: $root/ATTRIBUTIONS.md not found" >&2
  exit 1
}

manifests=(
  Cargo.toml
  components/Cargo.toml
  components-pyrefly/Cargo.toml
  components-secrets/Cargo.toml
  components-tsrs/Cargo.toml
)

rows="$(mktemp)"
trap 'rm -f "$rows"' EXIT
for manifest in "${manifests[@]}"; do
  [[ -f "$root/$manifest" ]] || {
    echo "error: missing $manifest" >&2
    exit 1
  }
  cargo metadata --format-version 1 --locked --manifest-path "$root/$manifest" |
    jq -r '.packages[] | select(.source != null) |
      [.name, .version,
       ((.license // (if .license_file then "see licence file" else "UNKNOWN" end)) | gsub("\\|"; "/")),
       (.repository // .homepage // "")] | @tsv' >> "$rows"
done

count="$(LC_ALL=C sort -u "$rows" | wc -l | tr -d ' ')"
[[ "$count" -gt 0 ]] || {
  echo "error: cargo metadata listed no third-party crates" >&2
  exit 1
}

{
  echo "# Third-party licenses"
  echo
  echo "poly is MIT licensed. The binary links the crates below, each under the licence it declares, and"
  echo "carries data and code credited in the attributions that follow the table."
  echo
  echo "## Crates"
  echo
  echo "| Crate | Version | License | Repository |"
  echo "| --- | --- | --- | --- |"
  LC_ALL=C sort -u "$rows" | awk -F'\t' '{ printf "| %s | %s | %s | %s |\n", $1, $2, $3, $4 }'
  echo
  echo "## Attributions"
  echo
  cat "$root/ATTRIBUTIONS.md"
} > "$out"
echo "wrote $out ($count crate versions)" >&2
