#!/usr/bin/env bash
#MISE description="Clone the pinned public corpora (or copy local repos) the hyperfine benchmarks run against"
#USAGE arg "<dir>" help="Directory to put the corpora into"
#USAGE flag "--large" help="Also clone the large opt-in corpora (kubernetes, vscode, home-assistant)"
#USAGE flag "--latest" help="Fetch each default-branch tip instead of the pinned commit (numbers are not comparable across days)"
#USAGE flag "--local <dir>..." var=#true help="Copy these local repositories (git archive HEAD) instead of cloning"
# Fetch a pinned set of public repositories into DIR for `mise run bench:hyperfine`.
#
# Usage: mise run bench:corpus DIR [--large] [--latest]
#        mise run bench:corpus DIR --local PATH [PATH...]
# Each corpus is fetched at the commit in `corpora`, so two runs on different days lint the same
# bytes. --latest takes the branch tip instead and prints the commit it got. --local copies
# `git archive HEAD` of each given repository, so the source tree is never touched and uncommitted
# changes are not benchmarked. Repin by editing the SHAs: `git ls-remote <repo> <branch>`.
set -euo pipefail
shopt -s inherit_errexit

usage="usage: corpus.sh DIR [--large] [--latest] | corpus.sh DIR --local PATH..."
dir=""
large=false
latest=false
local_mode=false
locals=()
while (($#)); do
  case "$1" in
  --large) large=true ;;
  --latest) latest=true ;;
  --local) local_mode=true ;;
  -*)
    echo "unknown flag: $1 ($usage)" >&2
    exit 2
    ;;
  *)
    if [[ -z "$dir" ]]; then
      dir="$1"
    elif [[ "$local_mode" == true ]]; then
      locals+=("$1")
    else
      echo "unexpected argument: $1 ($usage)" >&2
      exit 2
    fi
    ;;
  esac
  shift
done
[[ -n "$dir" ]] || {
  echo "$usage" >&2
  exit 2
}
mkdir -p "$dir"

if [[ "$local_mode" == true ]]; then
  [[ ${#locals[@]} -gt 0 ]] || {
    echo "--local needs at least one repository ($usage)" >&2
    exit 2
  }
  for src in "${locals[@]}"; do
    git -C "$src" rev-parse --git-dir >/dev/null 2>&1 || {
      echo "not a git repository: $src" >&2
      exit 1
    }
    name="$(basename "$(git -C "$src" rev-parse --show-toplevel)")"
    target="$dir/$name"
    if [[ -e "$target" ]]; then
      echo "skip $name: $target exists"
      continue
    fi
    mkdir -p "$target"
    git -C "$src" archive HEAD | tar -x -C "$target"
    # A bench run may rewrite files (`poly fmt --fix`), so give the copy its own repository.
    git init --quiet "$target"
    echo "$name $(git -C "$src" rev-parse HEAD) (local copy of $src)"
  done
  exit 0
fi

# name|repository|branch|commit pinned 2026-11-15
corpora=(
  "book|https://github.com/rust-lang/book|main|2771df165f092a4fbb9f46b5b2e7450f19010adb"
  "django|https://github.com/django/django|main|f24f38c6b2825f15628c07bf2892f1664eed9361"
)
if [[ "$large" == true ]]; then
  corpora+=(
    "kubernetes|https://github.com/kubernetes/kubernetes|master|04d3cc7a37391af9ee4363e2c0f7ae907c9c800e"
    "vscode|https://github.com/microsoft/vscode|main|d03edc4b18d687c45af067195bad9544b5b07475"
    "home-assistant|https://github.com/home-assistant/core|dev|0fc2cd50ca147c23a466b0b7b614884f59f2d853"
  )
fi

for entry in "${corpora[@]}"; do
  IFS='|' read -r name repo branch sha <<<"$entry"
  target="$dir/$name"
  if [[ -d "$target/.git" ]]; then
    echo "skip $name: already cloned at $(git -C "$target" rev-parse HEAD)"
    continue
  fi
  ref="$sha"
  [[ "$latest" == true ]] && ref="$branch"
  git init --quiet "$target"
  git -C "$target" remote add origin "$repo"
  git -C "$target" fetch --quiet --depth 1 origin "$ref"
  git -C "$target" checkout --quiet FETCH_HEAD
  got="$(git -C "$target" rev-parse HEAD)"
  if [[ "$latest" != true && "$got" != "$sha" ]]; then
    echo "$name: fetched $got, pinned $sha" >&2
    exit 1
  fi
  echo "$name $got (requested: $ref)"
done
