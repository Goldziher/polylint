#!/usr/bin/env bash
#MISE description="Bump the poly version across every shipped surface and regenerate the plugin outputs"
#USAGE arg "<version>" help="MAJOR.MINOR.PATCH or MAJOR.MINOR.PATCH-rc.N, without the leading v"
# Atomically bump the poly version across every shipped surface, then regenerate the
# ai-rulez plugin outputs and assert the plugin manifests carry the new version.
# Usage: mise run release:bump <version>
#   Cargo.toml                            [workspace.package] version
#   npm-package/package.json              version + every optionalDependencies pin
#   npm-package/platforms/*/package.json  version (one per release target)
#   pip-package/pyproject.toml            [project] version
#   pip-package/polylint/__init__.py      __version__
#   plugin/.ai-rulez/config.plugins.toml                 [plugin] version
#   .claude-plugin/*.json                 generated — asserted to match
#   plugin/.hermes/                       generated Hermes wheel and plugin (pyproject.toml version, __version__,
#                                         both plugin.yaml files) — asserted to match
#   README.md, website/.../installation.mdx, website/.../cli.mdx
#                                         the pinned-release examples (POLY_VERSION=, the Action's version:, the
#                                         `poly --version` output); left alone for a pre-release
#   components/Cargo.toml                 [workspace.package] version (poly-bundled and the poly-engines-* crates
#                                         inherit it); components/Cargo.lock is refreshed with the root lock.
#   components-pyrefly/Cargo.toml         the same for the pyrefly workspace and its lock.
#   components-secrets/Cargo.toml         the same for the secret-scanning workspace and its lock.
#   components-tsrs/Cargo.toml            the same for the TypeScript type-checking workspace and its lock.
#   components-r/Cargo.toml               the same for the R (air and jarl) workspace and its lock.
#                                         alef.toml reads the component version from Cargo.toml (version_from)

set -euo pipefail

VERSION="${1:?usage: mise run release:bump <version>}"

if ! [[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-rc\.[0-9]+)?$ ]]; then
  echo "error: version must be MAJOR.MINOR.PATCH or MAJOR.MINOR.PATCH-rc.N (got '$VERSION')" >&2
  exit 1
fi

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO_ROOT"

echo "→ Cargo.toml [workspace.package] → $VERSION"
sed -i.bak -E "s/^version = \"[^\"]+\"$/version = \"$VERSION\"/" Cargo.toml
rm Cargo.toml.bak
# Fail loudly if the substitution did not take: a silent no-op (moved key, added
# comment, changed quoting) would otherwise ship a binary reporting the old version.
if ! grep -qxF "version = \"$VERSION\"" Cargo.toml; then
  echo "error: Cargo.toml [workspace.package] version bump did not apply" >&2
  exit 1
fi

echo "→ components/Cargo.toml [workspace.package] → $VERSION"
sed -i.bak -E "s/^version = \"[^\"]+\"$/version = \"$VERSION\"/" components/Cargo.toml
rm components/Cargo.toml.bak
if ! grep -qxF "version = \"$VERSION\"" components/Cargo.toml; then
  echo "error: components/Cargo.toml [workspace.package] version bump did not apply" >&2
  exit 1
fi

echo "→ components-pyrefly/Cargo.toml [workspace.package] → $VERSION"
sed -i.bak -E "s/^version = \"[^\"]+\"$/version = \"$VERSION\"/" components-pyrefly/Cargo.toml
rm components-pyrefly/Cargo.toml.bak
if ! grep -qxF "version = \"$VERSION\"" components-pyrefly/Cargo.toml; then
  echo "error: components-pyrefly/Cargo.toml [workspace.package] version bump did not apply" >&2
  exit 1
fi

echo "→ components-secrets/Cargo.toml [workspace.package] → $VERSION"
sed -i.bak -E "s/^version = \"[^\"]+\"$/version = \"$VERSION\"/" components-secrets/Cargo.toml
rm components-secrets/Cargo.toml.bak
if ! grep -qxF "version = \"$VERSION\"" components-secrets/Cargo.toml; then
  echo "error: components-secrets/Cargo.toml [workspace.package] version bump did not apply" >&2
  exit 1
fi

echo "→ components-r/Cargo.toml [workspace.package] → $VERSION"
sed -i.bak -E "s/^version = \"[^\"]+\"$/version = \"$VERSION\"/" components-r/Cargo.toml
rm components-r/Cargo.toml.bak
if ! grep -qxF "version = \"$VERSION\"" components-r/Cargo.toml; then
  echo "error: components-r/Cargo.toml [workspace.package] version bump did not apply" >&2
  exit 1
fi

echo "→ components-tsrs/Cargo.toml [workspace.package] → $VERSION"
sed -i.bak -E "s/^version = \"[^\"]+\"$/version = \"$VERSION\"/" components-tsrs/Cargo.toml
rm components-tsrs/Cargo.toml.bak
if ! grep -qxF "version = \"$VERSION\"" components-tsrs/Cargo.toml; then
  echo "error: components-tsrs/Cargo.toml [workspace.package] version bump did not apply" >&2
  exit 1
fi

# The alef-generated component crates pin their own version and their poly-engines dependency, so
# regenerate them before resolving: otherwise neither lockfile can resolve the bumped workspace.
# Then refresh both lockfiles so the tagged build is reproducible. A failure here is fatal: a stale
# lock fails every --locked release build. SKIP_CARGO_UPDATE=1 skips both (the task's own tests).
if [[ "${SKIP_CARGO_UPDATE:-0}" != "1" ]]; then
  echo "→ alef component crates → $VERSION"
  "${ALEF:-alef}" generate --skip-compile >/dev/null
  echo "→ Cargo.lock, components/Cargo.lock, components-pyrefly/Cargo.lock, components-r/Cargo.lock, components-secrets/Cargo.lock and components-tsrs/Cargo.lock"
  cargo update --workspace --quiet
  cargo update --workspace --manifest-path components/Cargo.toml --quiet
  cargo update --workspace --manifest-path components-pyrefly/Cargo.toml --quiet
  cargo update --workspace --manifest-path components-secrets/Cargo.toml --quiet
  cargo update --workspace --manifest-path components-r/Cargo.toml --quiet
  cargo update --workspace --manifest-path components-tsrs/Cargo.toml --quiet
fi

echo "→ npm-package → $VERSION"
# jq rather than sed: the umbrella package carries the version in seven places
# (its own, plus one exact pin per platform package) and a textual substitution
# that matched six of them would publish a package resolving its binary from the
# previous release. Rewrite them structurally, then assert every one below.
npm_tmp="$(mktemp)"
jq --arg v "$VERSION" '.version = $v | .optionalDependencies |= with_entries(.value = $v)' \
  npm-package/package.json >"$npm_tmp"
mv "$npm_tmp" npm-package/package.json

for manifest in npm-package/platforms/*/package.json; do
  npm_tmp="$(mktemp)"
  jq --arg v "$VERSION" '.version = $v' "$manifest" >"$npm_tmp"
  mv "$npm_tmp" "$manifest"
done

# jq re-prints the whole document in its own style, so a structural rewrite also
# reformats the parts it did not change — it expands the inlined arrays these
# manifests keep, which poly's own formatter then wants to collapse again. Left
# alone, every release commit failed the dogfood job on a file the bump itself
# had just touched. Reformat here rather than after the fact: `poly fmt` is the
# authority on this repo's formatting and this repo dogfoods it.
if command -v poly >/dev/null 2>&1; then
  poly fmt --fix npm-package >/dev/null ||
    echo "warn: poly fmt over npm-package failed; run it before committing"
else
  echo "warn: poly not on PATH; run 'poly fmt --fix npm-package' before committing"
fi

echo "→ pip-package → $VERSION"
sed -i.bak -E "s/^version = \"[^\"]+\"$/version = \"$VERSION\"/" pip-package/pyproject.toml
rm pip-package/pyproject.toml.bak
sed -i.bak -E "s/^__version__ = \"[^\"]+\"$/__version__ = \"$VERSION\"/" pip-package/polylint/__init__.py
rm pip-package/polylint/__init__.py.bak

# Documentation examples that show a pinned release. A pre-release is never the documented pin, so an
# rc bump leaves them alone. The assertions below fail when an example changes shape and stops matching.
DOC_INSTALL="website/src/content/docs/start/installation.mdx"
DOC_CLI="website/src/content/docs/reference/cli.mdx"
if [[ "$VERSION" == *-* ]]; then
  echo "→ documentation examples: left alone for a pre-release"
else
  echo "→ documentation examples → $VERSION"
  semver='[0-9]+\.[0-9]+\.[0-9]+'
  for doc in README.md "$DOC_INSTALL" "$DOC_CLI"; do
    sed -i.bak -E \
      -e "s/POLY_VERSION=${semver}/POLY_VERSION=${VERSION}/g" \
      -e "s/version: v${semver} # omit for the latest release/version: v${VERSION} # omit for the latest release/" \
      -e "s/poly ${semver} \\((release|dev) build v${semver}/poly ${VERSION} (\\1 build v${VERSION}/g" \
      "$doc"
    rm "$doc.bak"
  done
fi

echo "→ plugin/.ai-rulez/config.plugins.toml [plugin] → $VERSION"
VERSION="$VERSION" perl -0pi -e \
  's/(\[plugin\][^\[]*?\nversion\s*=\s*")[^"]+(")/$1$ENV{VERSION}$2/s' plugin/.ai-rulez/config.plugins.toml
# Confirm the [plugin] version now reads $VERSION before we regenerate from it.
# Scoped to the [plugin] block: a bare `grep -qxF 'version = "X"'` passes on any
# matching line in the file — including the top-level schema `version` — so it
# would report success on a bump that never landed.
plugin_block_version="$(awk '/^\[plugin\]/{inblock=1; next} /^\[/{inblock=0} inblock && /^version[[:space:]]*=/{gsub(/.*= *"|".*/, ""); print; exit}' plugin/.ai-rulez/config.plugins.toml)"
if [[ "$plugin_block_version" != "$VERSION" ]]; then
  echo "error: plugin/.ai-rulez/config.plugins.toml [plugin] version is '$plugin_block_version', expected '$VERSION'" >&2
  exit 1
fi

echo "→ regenerating ai-rulez plugin outputs"
AI_RULEZ="${AI_RULEZ:-ai-rulez}"
"$AI_RULEZ" generate --plugin
"$AI_RULEZ" verify --plugin
mise run plugin:sync-skills

# Every bundle `--plugin` emits is shipped, so nothing is pruned here. The set is
# controlled by `[plugin] runtimes` in plugin/.ai-rulez/config.plugins.toml, which is the single
# place a harness is added or dropped.

echo
echo "Validating wrapper-package versions..."
validation_failed=0

fail() {
  echo "✗ $1"
  validation_failed=1
}

npm_version="$(jq -r '.version' npm-package/package.json)"
[[ "$npm_version" == "$VERSION" ]] ||
  fail "npm-package/package.json: expected $VERSION, got $npm_version"

# Every pin must move with the package it points at. A pin left behind resolves
# to the previous release's binary under the new version — a fault no test run
# and no `pnpm install` surfaces, because the install still succeeds.
while read -r pin; do
  [[ "$pin" == "$VERSION" ]] ||
    fail "npm-package/package.json optionalDependencies: expected $VERSION, got $pin"
done < <(jq -r '.optionalDependencies[]' npm-package/package.json)

# The pinned names and the platform directories must describe the same set:
# adding a release target to one and not the other silently drops a platform.
pinned_names="$(jq -r '.optionalDependencies | keys[]' npm-package/package.json | sort)"
platform_names="$(jq -r '.name' npm-package/platforms/*/package.json | sort)"
[[ "$pinned_names" == "$platform_names" ]] ||
  fail "npm-package: optionalDependencies and platforms/ disagree on the package set"

for manifest in npm-package/platforms/*/package.json; do
  platform_version="$(jq -r '.version' "$manifest")"
  [[ "$platform_version" == "$VERSION" ]] ||
    fail "$manifest: expected $VERSION, got $platform_version"
done

grep -qxF "version = \"$VERSION\"" pip-package/pyproject.toml ||
  fail "pip-package/pyproject.toml [project] version bump did not apply"
grep -qxF "__version__ = \"$VERSION\"" pip-package/polylint/__init__.py ||
  fail "pip-package/polylint/__init__.py __version__ bump did not apply"

if [[ $validation_failed -eq 0 ]]; then
  echo "✓ npm + PyPI wrapper packages are consistent: $VERSION"
fi

if [[ "$VERSION" != *-* ]]; then
  DOC_PINS=(
    "README.md:set \`POLY_VERSION=${VERSION}\` to pin a release"
    "$DOC_INSTALL:POLY_VERSION=${VERSION} curl "
    "$DOC_INSTALL:version: v${VERSION} # omit for the latest release"
    "$DOC_INSTALL:poly ${VERSION} (release build v${VERSION}, release)"
    "$DOC_INSTALL:poly ${VERSION} (dev build v${VERSION}-"
    "$DOC_CLI:poly ${VERSION} (release build v${VERSION}, release)"
    "$DOC_CLI:poly ${VERSION} (dev build v${VERSION}-"
  )
  for entry in "${DOC_PINS[@]}"; do
    file="${entry%%:*}"
    pin="${entry#*:}"
    grep -qF -- "$pin" "$file" 2>/dev/null ||
      fail "$file: pinned-release example not found: $pin"
  done
  if [[ $validation_failed -eq 0 ]]; then
    echo "✓ Documentation examples pin $VERSION"
  fi
fi

echo
echo "Validating plugin manifest versions..."

# One entry per file `generate --plugin` emits that carries a version, and the
# jq path to it. The set is driven by `[plugin] runtimes` in
# plugin/.ai-rulez/config.plugins.toml; adding a runtime there means adding its manifest here,
# or the lock-step guarantee quietly stops covering it — a stale manifest still
# installs cleanly, which is the expensive way to find out.
PLUGIN_MANIFESTS=(
  "plugin/.claude-plugin/plugin.json:.version"
  "plugin/.codex-plugin/plugin.json:.version"
  "plugin/plugin.json:.version"
  "plugin/.cursor-plugin/plugin.json:.version"
  "plugin/.factory-plugin/plugin.json:.version"
  "plugin/gemini-extension.json:.version"
  "plugin/kimi.plugin.json:.version"
  "plugin/package.json:.version"
)

for entry in "${PLUGIN_MANIFESTS[@]}"; do
  file="${entry%%:*}"
  path="${entry#*:}"
  if [[ ! -f "$file" ]]; then
    echo "✗ $file: missing (generation did not emit it)"
    validation_failed=1
    continue
  fi
  actual="$(jq -r "$path" "$file" 2>/dev/null || echo '')"
  if [[ "$actual" != "$VERSION" ]]; then
    echo "✗ $file ($path): expected $VERSION, got $actual"
    validation_failed=1
  fi
done

# The Hermes plugin wheel is generated from [plugin].version like the manifests above, but
# carries it in a pyproject and a module, not JSON. Assert both, scoped to the version key.
hermes_pyproject="plugin/.hermes/package/pyproject.toml"
hermes_init="plugin/.hermes/package/src/poly_hermes_plugin/__init__.py"
# The Python files carry the PEP 440 spelling of a pre-release (0.36.0-rc.1 is 0.36.0rc1).
hermes_version="${VERSION/-rc./rc}"
grep -qE "^version = ['\"]${hermes_version}['\"]$" "$hermes_pyproject" 2>/dev/null ||
  fail "$hermes_pyproject [project] version: expected $hermes_version"
grep -qxF "__version__ = \"$hermes_version\"" "$hermes_init" 2>/dev/null ||
  fail "$hermes_init __version__: expected $hermes_version"
# The same generator emits a second copy of the plugin and a plugin.yaml beside each.
hermes_plugin_init="plugin/.hermes/plugins/poly/__init__.py"
grep -qxF "__version__ = \"$hermes_version\"" "$hermes_plugin_init" 2>/dev/null ||
  fail "$hermes_plugin_init __version__: expected $hermes_version"
hermes_yamls=(
  plugin/.hermes/package/src/poly_hermes_plugin/plugin.yaml
  plugin/.hermes/plugins/poly/plugin.yaml
)
for hermes_yaml in "${hermes_yamls[@]}"; do
  grep -qxF "version: \"$VERSION\"" "$hermes_yaml" 2>/dev/null ||
    fail "$hermes_yaml version: expected $VERSION"
done

if ! jq -e '.plugins | map(select(.name == "poly" and .source == "./plugin")) | length == 1' \
  .claude-plugin/marketplace.json >/dev/null; then
  fail "Claude marketplace must resolve the generated poly member bundle"
fi

# The plugin is worth nothing to a consumer without its skills and slash
# commands, and it shipped without them for several releases: `content_root` was
# set to "." so ai-rulez looked for them at the repository root, where poly has
# none. Assert the payload, not just the version.
for dir in skills plugin/commands plugin/.pi/skills plugin/.pi/prompts; do
  if [[ ! -d "$dir" ]] || [[ -z "$(ls -A "$dir" 2>/dev/null)" ]]; then
    echo "✗ $dir/: empty or missing — the plugin would ship no $dir"
    validation_failed=1
  fi
done

skill_count="$(find skills -mindepth 1 -maxdepth 1 -type d 2>/dev/null | wc -l | tr -d ' ')"
command_count="$(find plugin/commands -mindepth 1 -maxdepth 1 -name '*.md' 2>/dev/null | wc -l | tr -d ' ')"
if [[ "$skill_count" -lt 1 || "$command_count" -lt 1 ]]; then
  echo "✗ plugin content: $skill_count skills, $command_count commands"
  validation_failed=1
else
  echo "✓ Plugin content: $skill_count skills, $command_count slash commands"
fi

pi_skill_count="$(jq '.pi.skills | length' plugin/package.json)"
pi_prompt_count="$(jq '.pi.prompts | length' plugin/package.json)"
while read -r skill; do
  [[ -f "plugin/$skill/SKILL.md" ]] || fail "Pi skill missing: $skill/SKILL.md"
done < <(jq -r '.pi.skills[]' plugin/package.json)
while read -r prompt; do
  [[ -f "plugin/$prompt" ]] || fail "Pi prompt missing: $prompt"
done < <(jq -r '.pi.prompts[]' plugin/package.json)
if [[ "$pi_skill_count" != "$skill_count" || "$pi_prompt_count" != "$command_count" ]]; then
  fail "Pi payload differs: $pi_skill_count skills/$pi_prompt_count prompts; expected $skill_count/$command_count"
fi

if [[ $validation_failed -eq 0 ]]; then
  echo "✓ Plugin manifests are consistent: $VERSION"
else
  echo "error: version validation failed. Review the above and fix manually." >&2
  exit 1
fi

echo
echo "Done. Review with: git diff"
echo "Next: cargo test --workspace --no-fail-fast && git commit -am 'chore(release): v$VERSION'"
