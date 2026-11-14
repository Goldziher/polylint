#!/usr/bin/env bash
#MISE description="Gate: components/, components-pyrefly/ and components-secrets/ Cargo.lock files are current for the root crates they build by path"
# components/, components-pyrefly/ and components-secrets/ depend on root crates (poly-engines, poly-engine-api, ...) by path, so a dependency
# change in one of them stales their Cargo.lock without any file under components/ changing.
# CI builds components/ with --locked, which then fails; this catches it at commit time.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
status=0
for dir in components components-pyrefly components-secrets; do
  if ! cargo metadata --locked --format-version 1 --manifest-path "${root}/${dir}/Cargo.toml" >/dev/null; then
    echo "check:components-lock: ${dir}/Cargo.lock is stale; refresh it with" >&2
    echo "  cargo metadata --format-version 1 --manifest-path ${dir}/Cargo.toml > /dev/null" >&2
    status=1
  fi
done
exit "$status"
