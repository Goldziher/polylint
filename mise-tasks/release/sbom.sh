#!/usr/bin/env bash
#MISE description="Write the CycloneDX SBOM of a release binary, or of every component a plan builds"
#USAGE arg "<kind>" help="binary or components" { choices "binary" "components"; }
#USAGE arg "<triple>" help="Rust target triple the artifact is built for"
#USAGE arg "<out>" help="binary: the SBOM file to write; components: the directory for <component>.cdx.json"
#USAGE arg "[args]..." help="binary: the Cargo.toml, then cargo feature flags; components: the plan.json"
set -euo pipefail

# mise run release:sbom binary <triple> <out> <Cargo.toml> [--no-default-features --features ...]
# mise run release:sbom components <triple> <out-dir> <plan.json>
#
# The SBOMs attached to a release (ADR 0057), CycloneDX 1.5 JSON. A Rust artifact is described by
# cargo-cyclonedx for its package, target and features, from the committed Cargo.lock; a Go sidecar
# (alef `producer_language = "executable"`) by cyclonedx-gomod for its GOOS/GOARCH. `components` writes
# one SBOM per row the plan marks `build`; reused components keep the SBOM of the release that built them.
#
# cargo-cyclonedx writes a file beside every workspace member's Cargo.toml, so the run uses a name of its
# own, keeps the producer's file and deletes the rest. Both tools only read metadata; neither builds.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "${ROOT_DIR}"

KIND="${1:?usage: release:sbom binary|components <triple> <out> ...}"
TRIPLE="${2:?usage: release:sbom binary|components <triple> <out> ...}"
OUT="${3:?usage: release:sbom binary|components <triple> <out> ...}"
shift 3

die() {
  echo "release:sbom: $*" >&2
  exit 1
}

# An SBOM that lists no component describes nothing: reject it rather than ship it.
check_sbom() {
  jq -e '.bomFormat == "CycloneDX" and (.metadata.component.name // "") != "" and ((.components // []) | length) > 0' \
    "$1" >/dev/null || die "$1 is not a CycloneDX SBOM with components"
}

rust_sbom() {
  local manifest="$1" out="$2" name produced
  shift 2
  [[ -f "${manifest}" ]] || die "no ${manifest}"
  name="poly-release-sbom-$$"
  cargo cyclonedx --manifest-path "${manifest}" --format json --spec-version 1.5 --target "${TRIPLE}" \
    --override-filename "${name}" "$@"
  produced="$(dirname "${manifest}")/${name}.json"
  [[ -f "${produced}" ]] || die "cargo cyclonedx wrote no ${produced}"
  mkdir -p "$(dirname "${out}")"
  mv "${produced}" "${out}"
  find "${ROOT_DIR}" -name "${name}.json" -not -path '*/target/*' -delete
  check_sbom "${out}"
}

go_sbom() {
  local module="$1" out="$2" goos goarch
  [[ -f "${module}/go.mod" ]] || die "no Go module at ${module}"
  case "${TRIPLE}" in
  x86_64-unknown-linux-gnu) goos=linux goarch=amd64 ;;
  aarch64-unknown-linux-gnu) goos=linux goarch=arm64 ;;
  aarch64-apple-darwin) goos=darwin goarch=arm64 ;;
  x86_64-pc-windows-msvc) goos=windows goarch=amd64 ;;
  *) die "no GOOS/GOARCH for ${TRIPLE}" ;;
  esac
  mkdir -p "$(dirname "${out}")"
  # The same build environment as mise-tasks/sidecar/build.sh, so the module graph is the one shipped.
  CGO_ENABLED=0 GOOS="${goos}" GOARCH="${goarch}" \
    cyclonedx-gomod app -json -output-version 1.5 -licenses -main . -output "${out}" "${module}"
  check_sbom "${out}"
}

# One field of a [[crates.components]] block in alef.toml. Plain awk, like components:release.
component_field() {
  awk -v want="$1" -v field="$2" '
    /^\[/ { name = ""; next }
    /^name = "/ { name = $0; sub(/^name = "/, "", name); sub(/".*$/, "", name); next }
    name == want && index($0, field " = \"") == 1 {
      value = substr($0, length(field) + 5); sub(/".*$/, "", value); print value; exit
    }
  ' alef.toml
}

case "${KIND}" in
binary)
  manifest="${1:?release:sbom binary needs the Cargo.toml}"
  shift
  rust_sbom "${manifest}" "${OUT}" "$@"
  echo "wrote ${OUT}"
  ;;

components)
  plan="${1:?release:sbom components needs the plan.json}"
  [[ -f "${plan}" ]] || die "no plan at ${plan}"
  mkdir -p "${OUT}"
  count=0
  while IFS= read -r component; do
    producer_dir="$(component_field "${component}" producer_dir)"
    if [[ "$(component_field "${component}" producer_language)" == "executable" ]]; then
      [[ -n "${producer_dir}" ]] || die "executable component ${component} has no producer_dir"
      go_sbom "${producer_dir}" "${OUT}/${component}.cdx.json"
    else
      rust_sbom "${producer_dir:-crates/poly-engines-${component}-component}/Cargo.toml" "${OUT}/${component}.cdx.json"
    fi
    count=$((count + 1))
  done < <(jq -r --arg t "${TRIPLE}" '.rows[] | select(.action == "build" and .target == $t) | .component' "${plan}")
  built="$(jq --arg t "${TRIPLE}" '[.rows[] | select(.action == "build" and .target == $t)] | length' "${plan}")"
  [[ "${count}" == "${built}" ]] || die "${TRIPLE} plans ${built} build(s), wrote ${count} SBOM(s)"
  echo "wrote ${count} component SBOM(s) for ${TRIPLE} into ${OUT}"
  ;;

*)
  die "unknown kind ${KIND} (binary or components)"
  ;;
esac
