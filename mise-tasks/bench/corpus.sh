#!/usr/bin/env bash
#MISE description="Clone the pinned public corpora the hyperfine benchmarks run against"
#USAGE arg "<dir>" help="Directory to clone the corpora into"
#USAGE flag "--large" help="Also clone the large opt-in corpora (kubernetes, vscode, home-assistant)"
# Shallow-clone a pinned set of public repositories into DIR for `mise run bench:hyperfine`.
#
# Usage: mise run bench:corpus DIR [--large]
# Each corpus is fetched at the commit named in `corpora`. A name with REF=main is unpinned: pin it
# to a commit SHA before using its numbers to compare two builds.
set -euo pipefail
shopt -s inherit_errexit

dir="${1:?usage: corpus.sh DIR [--large]}"
large=false
[[ "${2:-}" == "--large" ]] && large=true

# name|repository|ref
corpora=(
  "book|https://github.com/rust-lang/book|main"
  "django|https://github.com/django/django|main"
)
if [[ "$large" == true ]]; then
  corpora+=(
    "kubernetes|https://github.com/kubernetes/kubernetes|master"
    "vscode|https://github.com/microsoft/vscode|main"
    "home-assistant|https://github.com/home-assistant/core|dev"
  )
fi

mkdir -p "$dir"
for entry in "${corpora[@]}"; do
  IFS='|' read -r name repo ref <<<"$entry"
  target="$dir/$name"
  if [[ -d "$target/.git" ]]; then
    echo "skip $name: already cloned"
    continue
  fi
  git init --quiet "$target"
  git -C "$target" remote add origin "$repo"
  git -C "$target" fetch --quiet --depth 1 origin "$ref"
  git -C "$target" checkout --quiet FETCH_HEAD
  echo "$name $(git -C "$target" rev-parse HEAD) (requested ref: $ref)"
done
