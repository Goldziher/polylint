#!/usr/bin/env bash
#MISE description="Release build of the downloadable components on alef's content-addressed plan (previous-lock, plan, build, package, lock)"
#USAGE arg "<step>" help="Release step" { choices "previous-lock" "public-keys" "fetch-alef" "fetch-mise" "fetch-jq" "fetch-rustup-init" "plan" "build" "package" "stage" "unstage" "sign" "lock"; }
#USAGE arg "[args]..." help="plan, build, package, stage, unstage and sign take <triple>; lock takes <plans> <records> <out>"
# Release build of the downloadable components, on alef's content-addressed plan:
#
#   mise run components:release previous-lock                  fetch the lock the newest earlier release shipped and
#                                                              verify it (sha256sums.txt, attestation, signing keys)
#   mise run components:release public-keys                    print alef.toml's trusted public keys as JSON {id: key}
#   mise run components:release fetch-alef                     download and checksum-verify the musl alef release
#                                                              binary (for the manylinux container); prints its path
#   mise run components:release fetch-mise                     download and checksum-verify the musl mise release
#                                                              binary (for the manylinux container); prints its path
#   mise run components:release fetch-jq                       download and checksum-verify a static jq (for the container)
#   mise run components:release fetch-rustup-init              download and checksum-verify rustup-init (for the container)
#   mise run components:release plan    <triple>               fingerprint each component's inputs; mark each
#                                                              build or reuse against that lock
#   mise run components:release build   <triple>               compile the rows marked build (thin LTO, stripped)
#   mise run components:release package <triple>               package them as unsigned archives and records
#   mise run components:release stage   <triple>               tar the unsigned archives and the plan for the
#                                                              signing job, and write their digest manifest
#   mise run components:release unstage <triple>               check a staged tar and plan against the digest
#                                                              manifest and put the archives back where sign reads them
#   mise run components:release sign    <triple>               sign them with `alef component sign`, verify
#   mise run components:release lock    <plans> <records> <out>  merge every triple's plan, then write the lock,
#                                                              carrying the reuse rows over verbatim
#
# publish.yaml runs plan, build and package inside a manylinux_2_28 container on linux gnu, through the
# musl mise that fetch-mise verified (MISE_TASK_RUN_AUTO_INSTALL=false: the container installs no tools).
# plan, build, package and stage run in the build job, which never sees the signing key; unstage and sign
# run in a separate signing job on a fresh runner that builds nothing (ADR 0060): `alef component sign`
# loads no alef.toml and runs no cargo, rustc or go (alef ADR-0026). Everything lands under
# target/components-release/: previous-lock.json (`{}` when there is none), tools/, plan.json (merged, by
# `lock`) and, per triple, build/ (shared cargo target dir), plan/plan.json, unsigned/ (package),
# unstaged/ and digests.sha256 (stage), and dist/ (signed archives and *.record.json to upload).
#
# Environment:
#   GH_TOKEN, GITHUB_REPOSITORY     read the previous release (previous-lock)
#   RELEASE_TAG                     tag being released (previous-lock skips it)
#   REBUILD_COMPONENTS              "true" ignores the previous lock, so every component is rebuilt
#   POLY_COMPONENT_SIGNING_KEY      Ed25519 private key, PKCS#8 PEM or base64 (sign); never defaulted
#   RUST_TOOLCHAIN                  exact rust version (e.g. 1.99.1) `rustc` must report (plan, build, package)
#   VECTORSCAN_BUILD_FROM_SOURCE    "1" builds vectorscan from source (cmake and Boost); folded into the input hash
#   BUILD_IMAGE                     container image the steps run in, folded into the input hash; when set, ALEF
#                                   must be the binary `fetch-alef` verified, since the image installs no tools
#                                   and its glibc is too old for the gnu build
#   ALEF                            alef binary to use instead of the version pinned in mise.toml
#
# The plan, --plan and --reuse commands need the alef release that ships them (alef #869); the pin in
# mise.toml selects it, and `fetch-alef` needs the musl binaries of #874.
#
# The fetch-* steps check each download against a sha256 pinned below, not against a digest read from the
# release at download time: the signing job runs these binaries with the key, so trusting whatever the
# upstream release serves on the day would make its integrity the upstream account's. Bumping a version
# means bumping its digests (`sha256sum` of each asset, cross-checked with the release's own digest).

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "${ROOT_DIR}"

KEY_ID="poly-release-2"
LOCK_ASSET="poly-components.lock.json"
# The repository that signed the attestations of v0.35.0 and earlier, before the move to the polylint
# organization.
LEGACY_RELEASE_REPOSITORY="Goldziher/polylint"
# Stripped at link time. A dylib must never be stripped after linking: that removes the exported
# symbols the host looks up.
export RUSTFLAGS="-C strip=symbols"
export CARGO_PROFILE_RELEASE_LTO=thin

# The same inputs must reach plan, build and package: alef refuses to build or package once they
# hash differently from the plan. RUSTFLAGS and CARGO_PROFILE_* are fingerprinted from the environment.
# VECTORSCAN_BUILD_FROM_SOURCE=1, which publish.yaml sets in the manylinux containers only, makes the
# secrets component compile vectorscan instead of fetching the prebuilt archive (glibc 2.32, #112); an
# unset variable is left out of the fingerprint, so the other targets keep their hashes.
EXTRA=(--extra "key-id=${KEY_ID}" --extra "image=${BUILD_IMAGE:-host}" --env VECTORSCAN_BUILD_FROM_SOURCE)

STEP="${1:?usage: mise run components:release previous-lock|public-keys|fetch-alef|fetch-mise|fetch-jq|fetch-rustup-init|plan|build|package|stage|unstage|sign|lock ...}"

COMPONENTS_OUT="${ROOT_DIR}/target/components-release"
PREVIOUS_LOCK="${COMPONENTS_OUT}/previous-lock.json"

usage() {
  echo "usage: mise run components:release previous-lock | public-keys | fetch-alef | fetch-mise | fetch-jq | fetch-rustup-init | plan|build|package|stage|unstage|sign <triple> | lock <plans> <records> <out>" >&2
  exit 1
}

pinned_alef_version() {
  local version
  version="$(sed -n 's/^"github:xberg-io\/alef"[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' "${ROOT_DIR}/mise.toml")"
  [[ -n "${version}" ]] || {
    echo "components-release: no \"github:xberg-io/alef\" pin in mise.toml" >&2
    exit 1
  }
  echo "${version}"
}

# Tool pins: version and per-architecture sha256 of every binary the release containers run.
ALEF_PIN_VERSION="0.130.1"
MISE_PIN_VERSION="2026.10.24"
JQ_VERSION="1.8.2"
RUSTUP_VERSION="1.29.3"

# The pinned sha256 of <tool> for <arch> (x86_64 or aarch64).
pinned_sha256() {
  case "$1/$2" in
  alef/x86_64) echo "b572a2d13b387fa0d85b7ad9f61dce3e2dbae4fdd7862485a6895820d532f639" ;;
  alef/aarch64) echo "a22694869bd806abff1750501fbf2ebb4f54b7c8356e6b8de9ee5b5d235bea71" ;;
  mise/x86_64) echo "257bc6521a2401470fc38cf6be45bbf5783c0443c80298b61fb13f3354f7e06e" ;;
  mise/aarch64) echo "c1391f191cce6019ead500b6b22eb50f73bb0ba48f64619cc66bb27ff42840d0" ;;
  jq/x86_64) echo "b1c22172dd303f3be49e935aa56aa48a8b7a46e0bc838b4997d3bb451495870f" ;;
  jq/aarch64) echo "8b85c817833814ddca00a144c33705546355afccf0cf39b188f3cdb48b852309" ;;
  rustup-init/x86_64) echo "b1c8cb14e30889e7e2999e6b69e62d9e9b9ffc0f4df8180564d306d9680e6c55" ;;
  rustup-init/aarch64) echo "9f608c142e0e38a78957d0661ba1dfaf0b6fb37184cfaa4b9a0aa57af6184282" ;;
  *)
    echo "components-release: no pinned sha256 for $1 on $2" >&2
    exit 1
    ;;
  esac
}

# The host architecture in the spelling the pins use.
host_arch() {
  case "$(uname -m)" in
  x86_64 | amd64) echo "x86_64" ;;
  aarch64 | arm64) echo "aarch64" ;;
  *)
    echo "components-release: no pinned tools for $(uname -m)" >&2
    exit 1
    ;;
  esac
}

# Download <url> over HTTPS only and check it against <sha256>. Prints the path of the verified file,
# inside a scratch directory the caller removes.
download_pinned() {
  local url="$1" expected="$2" scratch file actual
  scratch="$(mktemp -d)"
  file="${scratch}/${url##*/}"
  curl --proto '=https' --tlsv1.2 -fsSL --retry 3 -o "${file}" "${url}"
  actual="$(sha256_of "${file}")"
  [[ "${actual}" == "${expected}" ]] || {
    echo "components-release: ${url} is sha256:${actual}, the pin is sha256:${expected}" >&2
    rm -rf "${scratch}"
    exit 1
  }
  echo "${file}"
}

# The musl release binary is static, so it runs on the manylinux_2_28 glibc where the gnu one does not.
# The pin must be the alef mise.toml selects, so the host and the containers fingerprint with one alef.
fetch_alef() {
  local version arch asset dir archive
  version="$(pinned_alef_version)"
  [[ "${version}" == "${ALEF_PIN_VERSION}" ]] || {
    echo "components-release: mise.toml pins alef ${version}, the fetch pin is ${ALEF_PIN_VERSION}; bump ALEF_PIN_VERSION and its sha256s" >&2
    exit 1
  }
  arch="$(host_arch)"
  asset="alef-${arch}-unknown-linux-musl.tar.gz"
  dir="${COMPONENTS_OUT}/tools/alef-${version}-${arch}-unknown-linux-musl"
  archive="$(download_pinned "https://github.com/xberg-io/alef/releases/download/v${version}/${asset}" \
    "$(pinned_sha256 alef "${arch}")")"
  rm -rf "${dir}"
  mkdir -p "${dir}"
  tar -xzf "${archive}" -C "${dir}" --strip-components=1
  rm -rf "$(dirname "${archive}")"
  [[ -x "${dir}/alef" ]] || {
    echo "components-release: ${asset} holds no alef binary" >&2
    exit 1
  }
  echo "${dir}/alef"
}

# The same for mise, so the manylinux container can run this very task. The musl build is static, so it
# runs on the container's old glibc. It only runs file tasks there, so it need not match the host's mise.
fetch_mise() {
  local arch mise_arch asset dir archive
  arch="$(host_arch)"
  case "${arch}" in
  x86_64) mise_arch="x64" ;;
  aarch64) mise_arch="arm64" ;;
  esac
  asset="mise-v${MISE_PIN_VERSION}-linux-${mise_arch}-musl.tar.gz"
  dir="${COMPONENTS_OUT}/tools/mise-${MISE_PIN_VERSION}-linux-${mise_arch}-musl"
  archive="$(download_pinned "https://github.com/jdx/mise/releases/download/v${MISE_PIN_VERSION}/${asset}" \
    "$(pinned_sha256 mise "${arch}")")"
  rm -rf "${dir}"
  mkdir -p "${dir}"
  tar -xzf "${archive}" -C "${dir}" --strip-components=2 mise/bin/mise
  rm -rf "$(dirname "${archive}")"
  [[ -x "${dir}/mise" ]] || {
    echo "components-release: ${asset} holds no mise binary" >&2
    exit 1
  }
  echo "${dir}/mise"
}

# jq, which the release steps read plans with and the manylinux image does not ship. jq's release binaries
# are static, so the host fetches and verifies one and the containers run it from the mount rather than
# installing a package.
fetch_jq() {
  local arch jq_arch dir file
  arch="$(host_arch)"
  case "${arch}" in
  x86_64) jq_arch="amd64" ;;
  aarch64) jq_arch="arm64" ;;
  esac
  dir="${COMPONENTS_OUT}/tools/jq-${JQ_VERSION}-linux-${jq_arch}"
  file="$(download_pinned "https://github.com/jqlang/jq/releases/download/jq-${JQ_VERSION}/jq-linux-${jq_arch}" \
    "$(pinned_sha256 jq "${arch}")")"
  rm -rf "${dir}"
  mkdir -p "${dir}"
  mv "${file}" "${dir}/jq"
  rm -rf "$(dirname "${file}")"
  chmod 0755 "${dir}/jq"
  echo "${dir}/jq"
}

# rustup-init for the manylinux containers, instead of piping https://sh.rustup.rs into a shell. The gnu
# build targets glibc 2.17, so it runs in manylinux_2_28; rustup then checks every toolchain component
# against the channel manifest's sha256.
fetch_rustup_init() {
  local arch dir file
  arch="$(host_arch)"
  dir="${COMPONENTS_OUT}/tools/rustup-init-${RUSTUP_VERSION}-${arch}"
  file="$(download_pinned \
    "https://static.rust-lang.org/rustup/archive/${RUSTUP_VERSION}/${arch}-unknown-linux-gnu/rustup-init" \
    "$(pinned_sha256 rustup-init "${arch}")")"
  rm -rf "${dir}"
  mkdir -p "${dir}"
  mv "${file}" "${dir}/rustup-init"
  rm -rf "$(dirname "${file}")"
  chmod 0755 "${dir}/rustup-init"
  echo "${dir}/rustup-init"
}

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d' ' -f1
  else
    shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

alef() {
  if [[ -n "${ALEF:-}" ]]; then
    "${ALEF}" "$@"
  elif [[ -n "${BUILD_IMAGE:-}" ]]; then
    echo "components-release: BUILD_IMAGE is set but ALEF is not; run fetch-alef on the host first" >&2
    exit 1
  else
    command alef "$@"
  fi
}

check_rustc() {
  : "${RUST_TOOLCHAIN:?${STEP} needs RUST_TOOLCHAIN (an exact rust version)}"
  local actual
  actual="$(rustc --version)"
  [[ "${actual}" == "rustc ${RUST_TOOLCHAIN} "* ]] || {
    echo "components-release: rustc is '${actual}', expected ${RUST_TOOLCHAIN}" >&2
    exit 1
  }
}

# The components alef.toml configures for a target, one per line. alef refuses to plan a component for a target it
# does not list, so the plan names exactly these (a component may ship for fewer targets than the rest). Plain awk,
# because the manylinux container has no tomllib.
components_for_target() {
  awk -v t="$1" '
    /^\[\[crates\.components\]\]/ { name = ""; next }
    /^name = "/ { name = $0; sub(/^name = "/, "", name); sub(/".*$/, "", name); next }
    /^targets = / && index($0, "\"" t "\"") && name != "" { print name }
  ' alef.toml
}

# Number of rows the plan marks `build`.
build_rows() {
  jq '[.rows[] | select(.action == "build")] | length' "$1"
}

# alef.toml's trusted public keys ([crates.component_distribution.public_keys]) as a JSON object {id: key}.
# Plain awk: the manylinux container has no tomllib.
public_keys() {
  awk '
    /^\[/ { in_keys = ($0 == "[crates.component_distribution.public_keys]"); next }
    in_keys && /^[A-Za-z0-9_.-]+[[:space:]]*=[[:space:]]*"[^"]*"/ {
      id = $0; sub(/[[:space:]]*=.*/, "", id)
      key = $0; sub(/^[^"]*"/, "", key); sub(/".*$/, "", key)
      printf "%s\t%s\n", id, key
    }
  ' alef.toml | jq -Rn '[inputs | split("\t") | {(.[0]): .[1]}] | add // {}'
}

# The digest sha256sums.txt records for <asset>, or nothing. Its paths carry the ./ that `find .` wrote.
listed_sha256() {
  awk -v a="$2" '{ n = $NF; sub(/^[*]/, "", n); sub(/^\.\//, "", n); if (n == a) print $1 }' "$1" | head -1
}

# Check a previous release's lock before anything reuses its entries: its bytes must be the ones that
# release's sha256sums.txt lists, its build-provenance attestation (releases since attestations began)
# must verify against this repository's publish workflow, and every key an entry is signed with must be a
# key alef.toml still trusts, with the same value, not just the same id.
verify_previous_lock() {
  local tag="$1" lock="$2" scratch expected actual digest attested keys
  scratch="$(mktemp -d)"
  gh release download "${tag}" --repo "${GITHUB_REPOSITORY}" --pattern sha256sums.txt --dir "${scratch}"
  expected="$(listed_sha256 "${scratch}/sha256sums.txt" "${LOCK_ASSET}")"
  rm -rf "${scratch}"
  actual="$(sha256_of "${lock}")"
  [[ -n "${expected}" && "${expected}" == "${actual}" ]] || {
    echo "components-release: ${tag}'s ${LOCK_ASSET} is sha256:${actual}; its sha256sums.txt lists '${expected}'" >&2
    exit 1
  }
  digest="sha256:${actual}"
  # Only a 404 means "no attestation"; any other failure (auth, network) must not read as one.
  if attested="$(gh api "repos/${GITHUB_REPOSITORY}/attestations/${digest}" --jq '.attestations | length' 2>&1)"; then
    :
  elif [[ "${attested}" == *'"status":"404"'* ]]; then
    attested=0
  else
    echo "components-release: cannot list attestations for ${tag}'s ${LOCK_ASSET}: ${attested}" >&2
    exit 1
  fi
  if [[ "${attested}" -gt 0 ]]; then
    # Releases published before the move to the polylint organization (v0.35.0 and earlier) were
    # signed as Goldziher/polylint, and a certificate keeps the identity it was issued under.
    local signer verified=""
    for signer in "${GITHUB_REPOSITORY}" "${LEGACY_RELEASE_REPOSITORY}"; do
      if gh attestation verify "${lock}" --repo "${signer}" \
        --signer-workflow "${signer}/.github/workflows/publish.yaml" >/dev/null 2>&1; then
        verified="${signer}"
        break
      fi
    done
    [[ -n "${verified}" ]] || {
      echo "components-release: ${tag}'s ${LOCK_ASSET} has an attestation that does not verify" >&2
      exit 1
    }
    echo "previous lock: attestation verified (${verified})"
  else
    echo "::warning::${tag}'s ${LOCK_ASSET} has no build-provenance attestation (released before attestations); checked against its sha256sums.txt only"
  fi
}

# Whether every entry of a verified previous lock is signed by a key alef.toml still trusts, by id and
# value. After a key rotation it is not, and its entries must not be reused: the caller rebuilds every
# component instead of failing, since the lock itself passed the integrity checks above.
previous_lock_keys_trusted() {
  local lock="$1" keys
  keys="$(public_keys)"
  jq -e --argjson keys "${keys}" '
    . as $lock
    | ($lock.artifacts | length) > 0
    and all($lock.artifacts[]; .key_id as $k | ($keys[$k] // null) != null and $lock.public_keys[$k] == $keys[$k])
  ' "${lock}" >/dev/null
}

# The newest other published release that shipped a lock decides: an older one says nothing about
# what the newest shipped. The file is `{}` when there is none, so every component builds. A failing
# gh call (auth, network) is an error, never "no lock".
previous_lock() {
  : "${RELEASE_TAG:?previous-lock needs RELEASE_TAG}"
  : "${GITHUB_REPOSITORY:?previous-lock needs GITHUB_REPOSITORY}"
  mkdir -p "${COMPONENTS_OUT}"
  echo '{}' >"${PREVIOUS_LOCK}"
  if [[ "${REBUILD_COMPONENTS:-false}" == "true" ]]; then
    echo "rebuild requested: ignoring the previous lock"
    return
  fi
  local scratch tags tag assets
  tags="$(gh release list --repo "${GITHUB_REPOSITORY}" --exclude-drafts --exclude-pre-releases --limit 20 \
    --json tagName --jq '.[].tagName')"
  scratch="$(mktemp -d)"
  while read -r tag; do
    [[ -n "${tag}" && "${tag}" != "${RELEASE_TAG}" ]] || continue
    assets="$(gh release view "${tag}" --repo "${GITHUB_REPOSITORY}" --json assets --jq '.assets[].name')"
    if grep -qx "${LOCK_ASSET}" <<<"${assets}"; then
      gh release download "${tag}" --repo "${GITHUB_REPOSITORY}" --pattern "${LOCK_ASSET}" --dir "${scratch}"
      verify_previous_lock "${tag}" "${scratch}/${LOCK_ASSET}"
      if previous_lock_keys_trusted "${scratch}/${LOCK_ASSET}"; then
        mv "${scratch}/${LOCK_ASSET}" "${PREVIOUS_LOCK}"
        echo "previous lock: ${tag}"
      else
        echo "::warning::${tag}'s lock is signed with a key alef.toml no longer trusts (rotated); rebuilding every component"
        jq '{public_keys, key_ids: ([.artifacts[].key_id] | unique)}' "${scratch}/${LOCK_ASSET}" >&2
      fi
      break
    fi
  done <<<"${tags}"
  rm -rf "${scratch}"
  jq -e 'has("artifacts")' "${PREVIOUS_LOCK}" >/dev/null 2>&1 || [[ "$(cat "${PREVIOUS_LOCK}")" == '{}' ]] || {
    echo "components-release: ${PREVIOUS_LOCK} is neither a component lock nor the empty marker" >&2
    exit 1
  }
  has_previous_lock || echo "no reusable ${LOCK_ASSET} from an earlier release: building every component"
}

has_previous_lock() {
  [[ -f "${PREVIOUS_LOCK}" ]] && jq -e 'has("artifacts")' "${PREVIOUS_LOCK}" >/dev/null 2>&1
}

if [[ "${STEP}" == "previous-lock" ]]; then
  previous_lock
  exit 0
fi

if [[ "${STEP}" == "public-keys" ]]; then
  public_keys
  exit 0
fi

if [[ "${STEP}" == "fetch-alef" ]]; then
  fetch_alef
  exit 0
fi

if [[ "${STEP}" == "fetch-mise" ]]; then
  fetch_mise
  exit 0
fi

if [[ "${STEP}" == "fetch-jq" ]]; then
  fetch_jq
  exit 0
fi

if [[ "${STEP}" == "fetch-rustup-init" ]]; then
  fetch_rustup_init
  exit 0
fi

if [[ "${STEP}" == "lock" ]]; then
  PLANS="${2:?usage: mise run components:release lock <plans> <records> <out>}"
  RECORDS="${3:?usage: mise run components:release lock <plans> <records> <out>}"
  OUTPUT="${4:?usage: mise run components:release lock <plans> <records> <out>}"
  mkdir -p "${COMPONENTS_OUT}" "${RECORDS}"
  MERGED="${COMPONENTS_OUT}/plan.json"
  plan_files=()
  while IFS= read -r file; do plan_files+=("${file}"); done < <(find "${PLANS}" -name plan.json | sort)
  [[ ${#plan_files[@]} -gt 0 ]] || {
    echo "components-release: no plan.json under ${PLANS}" >&2
    exit 1
  }
  jq -s '{schema_version: .[0].schema_version, debug: .[0].debug, rows: (map(.rows) | add)}' "${plan_files[@]}" >"${MERGED}"
  if has_previous_lock; then
    alef component lock --input "${RECORDS}" --output "${OUTPUT}" --reuse "${PREVIOUS_LOCK}" --plan "${MERGED}"
  else
    [[ "$(build_rows "${MERGED}")" == "$(jq '.rows | length' "${MERGED}")" ]] || {
      echo "components-release: the plan reuses components but there is no previous lock" >&2
      exit 1
    }
    alef component lock --input "${RECORDS}" --output "${OUTPUT}"
  fi
  exit 0
fi

TARGET="${2:-}"
[[ -n "${TARGET}" ]] || usage

OUT="${COMPONENTS_OUT}/${TARGET}"
BUILD="${OUT}/build"
DIST="${OUT}/dist"
PLAN="${OUT}/plan/plan.json"
# Where `package --unsigned` writes the unsigned archives and records, which `sign` reads.
UNSIGNED="${OUT}/unsigned"
STAGED="${OUT}/unstaged"
DIGESTS="${OUT}/digests.sha256"

# Fail unless <dir> holds exactly <expected> component archives and as many records.
count_archives() {
  local records archives
  records="$(find "$1" -maxdepth 1 -name '*.record.json' | grep -c . || true)"
  archives="$(find "$1" -maxdepth 1 -name 'poly-component-*.tar.gz' | grep -c . || true)"
  [[ "${records}" == "$2" && "${archives}" == "$2" ]] || {
    echo "components-release: ${TARGET} plans $2 build(s), $1 holds ${archives} archive(s) and ${records} record(s)" >&2
    exit 1
  }
}

case "${STEP}" in
plan)
  check_rustc
  rm -rf "${OUT}"
  mkdir -p "${DIST}" "$(dirname "${PLAN}")"
  previous=()
  ! has_previous_lock || previous=(--previous-lock "${PREVIOUS_LOCK}")
  selected=()
  while IFS= read -r name; do selected+=(--component "${name}"); done < <(components_for_target "${TARGET}")
  [[ ${#selected[@]} -gt 0 ]] || {
    echo "components-release: alef.toml lists no component for ${TARGET}" >&2
    exit 1
  }
  alef component plan --target "${TARGET}" "${selected[@]}" ${previous[@]+"${previous[@]}"} "${EXTRA[@]}" --output "${PLAN}"
  rows="$(jq '.rows | length' "${PLAN}")"
  [[ "${rows}" -gt 0 ]] || {
    echo "components-release: alef.toml lists no component for ${TARGET}" >&2
    exit 1
  }
  [[ "$(jq --arg t "${TARGET}" '[.rows[] | select(.target != $t)] | length' "${PLAN}")" == "0" ]] || {
    echo "components-release: the plan has rows for a target other than ${TARGET}" >&2
    exit 1
  }
  jq -r '.rows[] | "\(.component) \(.target) \(.action) \(.input_hash)"' "${PLAN}"
  ;;

build)
  check_rustc
  [[ -f "${PLAN}" ]] || {
    echo "components-release: no plan at ${PLAN}; run the plan step first" >&2
    exit 1
  }
  if [[ "$(build_rows "${PLAN}")" -gt 0 ]]; then
    alef component build --plan "${PLAN}" --target-dir "${BUILD}" "${EXTRA[@]}"
  fi
  ;;

package)
  check_rustc
  [[ -f "${PLAN}" ]] || {
    echo "components-release: no plan at ${PLAN}; run the plan step first" >&2
    exit 1
  }
  expected="$(build_rows "${PLAN}")"
  rm -rf "${UNSIGNED}"
  mkdir -p "${UNSIGNED}"
  if [[ "${expected}" -gt 0 ]]; then
    alef component package --plan "${PLAN}" --target-dir "${BUILD}" --output "${UNSIGNED}" --unsigned "${EXTRA[@]}"
  fi
  count_archives "${UNSIGNED}" "${expected}"
  ;;

stage)
  [[ -f "${PLAN}" && -d "${UNSIGNED}" ]] || {
    echo "components-release: stage needs ${PLAN} and ${UNSIGNED}; run the plan, build and package steps first" >&2
    exit 1
  }
  count_archives "${UNSIGNED}" "$(build_rows "${PLAN}")"
  rm -rf "${STAGED}"
  mkdir -p "${STAGED}"
  # One tar, so the signing job gets exactly the files this job packaged.
  tar -cf "${STAGED}/archives.tar" -C "${UNSIGNED}" .
  cp "${PLAN}" "${STAGED}/plan.json"
  {
    printf '%s  %s\n' "$(sha256_of "${STAGED}/archives.tar")" archives.tar
    printf '%s  %s\n' "$(sha256_of "${STAGED}/plan.json")" plan.json
  } >"${DIGESTS}"
  cat "${DIGESTS}"
  ;;

unstage)
  [[ -f "${DIGESTS}" && -f "${STAGED}/archives.tar" && -f "${STAGED}/plan.json" ]] || {
    echo "components-release: unstage needs ${DIGESTS}, ${STAGED}/archives.tar and ${STAGED}/plan.json" >&2
    exit 1
  }
  [[ "$(awk '{ print $2 }' "${DIGESTS}" | sort | tr '\n' ' ')" == "archives.tar plan.json " ]] || {
    echo "components-release: ${DIGESTS} must list exactly archives.tar and plan.json" >&2
    exit 1
  }
  while read -r digest name; do
    actual="$(sha256_of "${STAGED}/${name}")"
    [[ "${actual}" == "${digest}" ]] || {
      echo "components-release: staged ${name} is sha256:${actual}, the build job recorded sha256:${digest}" >&2
      exit 1
    }
  done <"${DIGESTS}"
  rm -rf "${UNSIGNED}"
  mkdir -p "${UNSIGNED}" "$(dirname "${PLAN}")"
  cp "${STAGED}/plan.json" "${PLAN}"
  tar -xf "${STAGED}/archives.tar" -C "${UNSIGNED}"
  count_archives "${UNSIGNED}" "$(build_rows "${PLAN}")"
  echo "unstaged the archives for ${TARGET}, digests verified"
  ;;

sign)
  [[ -f "${PLAN}" && -d "${UNSIGNED}" ]] || {
    echo "components-release: sign needs ${PLAN} and ${UNSIGNED}; run the unstage step first" >&2
    exit 1
  }
  expected="$(build_rows "${PLAN}")"
  rm -rf "${DIST}"
  mkdir -p "${DIST}"
  if [[ "${expected}" -gt 0 ]]; then
    : "${POLY_COMPONENT_SIGNING_KEY:?sign needs POLY_COMPONENT_SIGNING_KEY (Ed25519 private key)}"
    public_key="$(public_keys | jq -er --arg id "${KEY_ID}" '.[$id]')" || {
      echo "components-release: alef.toml configures no public key for ${KEY_ID}" >&2
      exit 1
    }
    # --public-key refuses a private key that is not the configured key's other half; --plan refuses
    # an archive with no matching build row.
    alef component sign --input "${UNSIGNED}" --output "${DIST}" --plan "${PLAN}" \
      --key-env POLY_COMPONENT_SIGNING_KEY --key-id "${KEY_ID}" --public-key "${public_key}"
    alef component verify --input "${DIST}"
  fi
  count_archives "${DIST}" "${expected}"
  ls -l "${DIST}"
  ;;

*)
  usage
  ;;
esac
