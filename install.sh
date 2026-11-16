#!/bin/sh
# poly installer for Linux and macOS: downloads the prebuilt binary for this host,
# verifies it against the release's sha256sums.txt and installs it atomically.
# Re-run any time to update. Run `install.sh --help` for options.

set -eu

REPO="polylint/polylint"
# The pre-transfer identity, which signed the attestations of v0.35.0 and earlier.
LEGACY_REPO="Goldziher/polylint"
BINARIES="poly"

VERSION="${POLY_VERSION:-latest}"
INSTALL_DIR="${POLY_INSTALL_DIR:-${HOME}/.local/bin}"
NO_MODIFY_PATH="${POLY_NO_MODIFY_PATH:-}"
VERIFY_ATTESTATION="${POLY_VERIFY_ATTESTATION:-}"
ARCHIVE_CACHE="${POLY_ARCHIVE_CACHE:-}"

if [ -t 1 ]; then
  C_GREEN=$(printf '\033[0;32m')
  C_YELLOW=$(printf '\033[1;33m')
  C_RED=$(printf '\033[0;31m')
  C_DIM=$(printf '\033[2m')
  C_OFF=$(printf '\033[0m')
else
  C_GREEN='' C_YELLOW='' C_RED='' C_DIM='' C_OFF=''
fi

info() { printf '%s✓%s %s\n' "$C_GREEN" "$C_OFF" "$1"; }
warn() { printf '%s⚠%s %s\n' "$C_YELLOW" "$C_OFF" "$1" >&2; }
die() {
  printf '%s✗%s %s\n' "$C_RED" "$C_OFF" "$1" >&2
  exit 1
}

usage() {
  cat <<'EOF'
Usage: install.sh [OPTIONS] [VERSION]

Installs poly (and the polylint alias) from a GitHub release.

Options:
  --version VERSION   Release to install, e.g. 0.35.0 or v0.35.0 (default: latest)
  --bin-dir DIR       Install directory (default: $HOME/.local/bin)
  --no-modify-path    Do not edit shell startup files; print the PATH line instead
  --verify-attestation
                      Also verify the archive's build provenance attestation with
                      `gh attestation verify` (needs the GitHub CLI); fail if it does not verify
  -h, --help          Show this help

Environment:
  POLY_VERSION              Same as --version
  POLY_INSTALL_DIR          Same as --bin-dir
  POLY_NO_MODIFY_PATH       Any non-empty value: same as --no-modify-path
  POLY_VERIFY_ATTESTATION   1: same as --verify-attestation
  POLY_ARCHIVE_CACHE        Directory to reuse a downloaded archive from and keep it in; a
                            cached archive is checked against sha256sums.txt like a download

A bare positional argument is also taken as the version.
EOF
  exit 0
}

while [ $# -gt 0 ]; do
  case "$1" in
  -h | --help) usage ;;
  --no-modify-path) NO_MODIFY_PATH=1 ;;
  --verify-attestation) VERIFY_ATTESTATION=1 ;;
  --version)
    shift
    [ $# -gt 0 ] || die "--version needs an argument"
    VERSION="$1"
    ;;
  --version=*) VERSION="${1#*=}" ;;
  --bin-dir)
    shift
    [ $# -gt 0 ] || die "--bin-dir needs an argument"
    INSTALL_DIR="$1"
    ;;
  --bin-dir=*) INSTALL_DIR="${1#*=}" ;;
  -*) die "unknown option: $1 (try --help)" ;;
  *) VERSION="$1" ;;
  esac
  shift
done

case "$INSTALL_DIR" in
"" | -*) die "invalid install directory: '${INSTALL_DIR}'" ;;
esac

command -v curl >/dev/null 2>&1 || die "curl is required"
command -v tar >/dev/null 2>&1 || die "tar is required"

case "$VERIFY_ATTESTATION" in
"" | 0) VERIFY_ATTESTATION="" ;;
1) command -v gh >/dev/null 2>&1 || die "attestation verification needs the GitHub CLI (gh) on PATH" ;;
*) die "POLY_VERIFY_ATTESTATION must be 1 or unset" ;;
esac

# HTTPS only, TLS 1.2 or newer, on the first request and on every redirect.
fetch() {
  curl --proto '=https' --proto-redir '=https' --tlsv1.2 -fsSL "$@"
}

uname_s=$(uname -s)
uname_m=$(uname -m)

case "$uname_m" in
x86_64 | amd64) arch="x86_64" ;;
aarch64 | arm64) arch="aarch64" ;;
*) die "unsupported architecture: $uname_m" ;;
esac

ext="tar.gz"
case "$uname_s" in
Darwin)
  [ "$arch" = "aarch64" ] || die "no prebuilt binary for Intel macOS (x86_64); build from source: https://github.com/${REPO}#installation"
  target="${arch}-apple-darwin"
  ;;
Linux)
  if (ldd --version 2>&1 | grep -qi musl) || [ -e /lib/ld-musl-"${uname_m}".so.1 ]; then
    [ "$arch" = "x86_64" ] || die "no prebuilt binary for ${arch} Linux with musl libc; build from source: https://github.com/${REPO}#installation"
    target="${arch}-unknown-linux-musl"
  else
    target="${arch}-unknown-linux-gnu"
  fi
  ;;
MINGW* | MSYS* | CYGWIN* | Windows_NT)
  die "Windows detected. Use the PowerShell installer:
  irm https://raw.githubusercontent.com/${REPO}/main/install.ps1 | iex"
  ;;
*) die "unsupported OS: $uname_s" ;;
esac

info "Platform: ${C_DIM}${target}${C_OFF}"

if [ "$VERSION" = "latest" ]; then
  info "Resolving latest release..."
  effective=$(fetch -o /dev/null -w '%{url_effective}' \
    "https://github.com/${REPO}/releases/latest" 2>/dev/null || true)
  case "$effective" in
  */releases/tag/*) VERSION="${effective##*/tag/}" ;;
  *)
    VERSION=$(fetch "https://api.github.com/repos/${REPO}/releases/latest" 2>/dev/null |
      grep -o '"tag_name"[^,]*' | head -1 | cut -d'"' -f4)
    ;;
  esac
  [ -n "$VERSION" ] || die "could not resolve the latest release"
fi

# X.Y.Z[-pre.release], matched without grep so an embedded newline cannot pass.
valid_version() {
  case "$1" in
  "" | *[!0-9A-Za-z.-]*) return 1 ;;
  esac
  core="${1%%-*}"
  pre="${1#"$core"}"
  case "$core" in
  *[!0-9.]* | .* | *. | *..* | *.*.*.*) return 1 ;;
  [0-9]*.[0-9]*.[0-9]*) ;;
  *) return 1 ;;
  esac
  [ -z "$pre" ] && return 0
  pre="${pre#-}"
  case "$pre" in
  "" | .* | *. | *..* | -*) return 1 ;;
  esac
  return 0
}

valid_version "${VERSION#v}" || die "invalid version: $(printf '%s' "$VERSION" | tr -c '0-9A-Za-z.+-' '?')"

tag="v${VERSION#v}"
version="${tag#v}"

asset="poly-${version}-${target}.${ext}"
base="https://github.com/${REPO}/releases/download/${tag}"

previous=""
if [ -x "${INSTALL_DIR}/poly" ]; then
  previous=$("${INSTALL_DIR}/poly" --version 2>/dev/null | awk '{print $2}' || true)
  # `poly --version` prints `poly 0.35.0 (release build v0.35.0, release)`.
  printf '%s\n' "$previous" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+([.+-][0-9A-Za-z.+-]+)?$' || previous=""
fi

if [ -n "$previous" ] && [ "$previous" = "$version" ]; then
  info "poly ${version} is already installed in ${INSTALL_DIR} — re-installing."
elif [ -n "$previous" ]; then
  info "Updating poly ${previous} → ${version}"
else
  info "Installing poly ${version}"
fi

tmp=$(mktemp -d)
staged=""
trap 'rm -rf "$tmp"; [ -z "$staged" ] || rm -f "$staged"' EXIT

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | awk '{print $1}'
  elif command -v openssl >/dev/null 2>&1; then
    openssl dgst -sha256 "$1" | awk '{print $NF}'
  else
    return 1
  fi
}

fetch -o "${tmp}/sha256sums.txt" "${base}/sha256sums.txt" 2>/dev/null ||
  die "could not download ${base}/sha256sums.txt — refusing to install an unverified binary"
expected=$(awk -v a="$asset" '{n=$NF; sub(/^[*]/,"",n); sub(/^\.\//,"",n); if (n==a) print $1}' \
  "${tmp}/sha256sums.txt" | head -1)
[ -n "$expected" ] || die "no checksum entry for ${asset} — refusing to install unverified binary"
sha256_of "${tmp}/sha256sums.txt" >/dev/null ||
  die "no sha256 tool (sha256sum/shasum/openssl) found — refusing to install unverified binary"

# A cached archive is used only when it still hashes to the release's checksum; anything else
# is downloaded again.
cached=""
if [ -n "$ARCHIVE_CACHE" ] && [ -f "${ARCHIVE_CACHE}/${asset}" ]; then
  if [ "$(sha256_of "${ARCHIVE_CACHE}/${asset}")" = "$expected" ]; then
    cp "${ARCHIVE_CACHE}/${asset}" "${tmp}/${asset}"
    cached=1
    info "Using the cached ${C_DIM}${ARCHIVE_CACHE}/${asset}${C_OFF}"
  else
    warn "the cached ${ARCHIVE_CACHE}/${asset} does not match sha256sums.txt; downloading it again"
  fi
fi
if [ -z "$cached" ]; then
  info "Downloading ${C_DIM}${base}/${asset}${C_OFF}"
  fetch -o "${tmp}/${asset}" "${base}/${asset}" ||
    die "failed to download ${base}/${asset}: check the version and your network (does release ${tag} ship ${target}?)"
fi

actual=$(sha256_of "${tmp}/${asset}" || true)
[ "$expected" = "$actual" ] || die "checksum mismatch for ${asset} (expected ${expected}, got ${actual})"
info "Checksum verified"

# The build provenance attestation ties the archive to this repository's release workflow
# (releases before the attestations began have none, so this fails for them). Releases
# published before the repository moved to the polylint organization (v0.35.0 and earlier)
# were signed as LEGACY_REPO, and a certificate keeps the identity it was issued under.
if [ -n "$VERIFY_ATTESTATION" ]; then
  attested=""
  for signer in "$REPO" "$LEGACY_REPO"; do
    if gh attestation verify "${tmp}/${asset}" --repo "$signer" \
      --signer-workflow "${signer}/.github/workflows/publish.yaml" >/dev/null 2>&1; then
      attested="$signer"
      break
    fi
  done
  [ -n "$attested" ] ||
    die "the build provenance attestation of ${asset} did not verify — refusing to install"
  info "Build provenance attestation verified (${attested})"
fi

if [ -n "$ARCHIVE_CACHE" ]; then
  mkdir -p "$ARCHIVE_CACHE"
  cp "${tmp}/${asset}" "${ARCHIVE_CACHE}/${asset}"
fi

tar xzf "${tmp}/${asset}" -C "$tmp"
mkdir -p "$INSTALL_DIR"
for binary in $BINARIES; do
  [ -f "${tmp}/${binary}" ] || die "expected binary ${binary} missing from ${asset}"
  # Stage inside the destination directory (same filesystem), finish every
  # modification on the staged copy, then rename(2) it over the destination.
  # `mv` within one filesystem is a rename, which is atomic: a concurrent reader
  # sees either the old binary or the new one, never a truncated or absent file.
  # That matters because poly's git hooks are installed per repository but resolve
  # `poly` from PATH globally and fail closed — an upgrade that unlinks or
  # truncates the destination blocks every commit on the machine for as long as it
  # runs. Anything that must touch the binary (chmod here; on macOS a `codesign`
  # or `xattr` step, should one ever be added) belongs before the rename, never
  # after: an in-place edit of the live destination reopens exactly that window,
  # and on macOS an unsigned or half-signed binary is SIGKILLed on exec.
  staged="${INSTALL_DIR}/${binary}.tmp.$$"
  rm -f "$staged"
  cp "${tmp}/${binary}" "$staged" || die "failed to stage ${binary} in ${INSTALL_DIR}"
  chmod 0755 "$staged" || die "failed to set the exec bit on ${staged}"
  mv -f "$staged" "${INSTALL_DIR}/${binary}" || die "failed to install ${binary} into ${INSTALL_DIR}"
  staged=""
done

# `polylint` is an alias for the same executable, not a second binary: the
# package is published as `polylint` on PyPI and `@polylint/polylint` on npm
# (the unscoped `poly` name is taken on both), so someone who installed it under
# that name will reasonably type `polylint`. A relative symlink keeps the two in
# lock-step through an upgrade and survives the directory being moved. It is
# staged and renamed for the same reason the binary is — replacing a live entry
# in place opens a window where a git hook resolving it from PATH fails closed.
staged="${INSTALL_DIR}/polylint.tmp.$$"
rm -f "$staged"
if ln -s poly "$staged" 2>/dev/null; then
  mv -f "$staged" "${INSTALL_DIR}/polylint" || die "failed to install the polylint alias into ${INSTALL_DIR}"
else
  # A filesystem without symlinks (some network and Windows-hosted mounts).
  # A copy is a worse alias — it goes stale on the next upgrade — so fall back
  # to it only here, and say so rather than pretending the alias is a link.
  cp "${INSTALL_DIR}/poly" "$staged" || die "failed to stage the polylint alias in ${INSTALL_DIR}"
  chmod 0755 "$staged" || die "failed to set the exec bit on ${staged}"
  mv -f "$staged" "${INSTALL_DIR}/polylint" || die "failed to install the polylint alias into ${INSTALL_DIR}"
  warn "symlinks are unavailable here, so polylint was installed as a copy of poly"
fi
staged=""

info "Installed poly (and the polylint alias) → ${INSTALL_DIR}"
case "$target" in
*-linux-musl) ;; # the bundled build: every engine is linked in
*) info "Engines download as signed components on first use; run 'poly components sync' in a repository to fetch them ahead (offline or CI)." ;;
esac

add_path_line='export PATH="'"${INSTALL_DIR}"':$PATH"'
zdotdir="${ZDOTDIR:-$HOME}"
case "${SHELL:-}" in
*/fish) shell_path_line="fish_add_path ${INSTALL_DIR}" ;;
*) shell_path_line="$add_path_line" ;;
esac
case ":${PATH}:" in
*":${INSTALL_DIR}:"*)
  :
  ;;
*)
  if [ -n "$NO_MODIFY_PATH" ]; then
    warn "${INSTALL_DIR} is not on your PATH. Add it with:"
    printf '    %s\n' "$shell_path_line"
  else
    modified=""
    for rc in "${HOME}/.profile" "${HOME}/.bashrc" "${zdotdir}/.zshrc"; do
      [ -e "$rc" ] || continue
      if ! grep -qF "$INSTALL_DIR" "$rc" 2>/dev/null; then
        {
          printf '\n# added by the poly installer\n%s\n' "$add_path_line"
        } >>"$rc"
        modified="${modified} ${rc}"
      fi
    done
    if [ -n "$modified" ]; then
      info "Added ${INSTALL_DIR} to PATH in:${modified}"
      warn "Restart your shell or run: ${shell_path_line}"
    else
      warn "${INSTALL_DIR} is not on your PATH. Add it with:"
      printf '    %s\n' "$shell_path_line"
    fi
  fi
  ;;
esac

printf '%s\n' "${C_GREEN}poly ${version} is ready.${C_OFF} Run 'poly --help' to get started."
