#!/usr/bin/env bash
#MISE description="Fail when a website page links to the stale /poly/ base path (the site is served under /polylint)"
# Starlight serves the site under `base: /polylint` (website/astro.config.mjs), so a root-relative
# `/poly/...` link resolves to a 404. Links must use the `/polylint/` base or a relative path.
set -euo pipefail
shopt -s inherit_errexit

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
content="$root/website/src/content"
[[ -d "$content" ]] || {
  echo "docs:check-links: $content does not exist; refusing to report a pass" >&2
  exit 2
}
# Zero pages scanned would read as "no stale links", so refuse that too.
pages="$(find "$content" -type f \( -name '*.md' -o -name '*.mdx' \) | wc -l | tr -d ' ')"
if [[ "$pages" -eq 0 ]]; then
  echo "docs:check-links: no .md/.mdx pages under $content; refusing to report a pass" >&2
  exit 2
fi

set +e
stale="$(grep -rnE '\]\(/poly/|href="/poly/' "$content")"
status=$?
set -e
# grep exits 1 for "no match"; anything else is a read failure, not a pass.
if [[ "$status" -gt 1 ]]; then
  echo "docs:check-links: grep failed (exit $status) over $content" >&2
  exit 2
fi
if [[ "$status" -eq 0 ]]; then
  count="$(grep -c . <<<"$stale")"
  echo "docs:check-links: $count line(s) with a stale /poly/ link; the site base is /polylint, so use /polylint/... or a relative link:" >&2
  echo "${stale//"$root/"/}" >&2
  exit 1
fi
echo "docs:check-links: $pages page(s) scanned, no stale /poly/ links"
