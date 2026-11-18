# 0082 — `poly lock` Follows Moved Hook Source Tags

- Status: Accepted
- Date: 2026-11-18
- Related: ADR 0020, ADR 0038

## Context

A `[[hooks.sources]]` entry declares a `revision`: a branch, a tag or a full object ID. `poly lock` (ADR 0038)
resolved it with `git ls-remote` and recorded only the object ID, keyed by source id and URL, and kept a recorded
entry unless `--upgrade`. Floating tags are common for hook sources: ai-rulez retags `v5` on every release. A plain
`poly lock` never followed such a tag, and editing `revision = "v5"` to `"v6"` kept the old object ID silently, since
the declared revision was not part of the entry.

## Decision

- **The entry records the declared revision** as `ref`, beside the resolved `revision`. An entry without it (written
  before this change) still parses.
- **Every `poly lock` re-resolves hook source refs.** With or without `--upgrade`, each declared ref is resolved with
  `git ls-remote` (nothing is downloaded), and the entry is rewritten when the tag moved, the declared revision
  changed, or the entry has no `ref`. A declared full object ID already recorded with that `ref` needs no network.
  `--upgrade` keeps its meaning for remote `extends` refs and `latest` tools.
- **`poly lock --check` reports hook source drift online**: a recorded `ref` that differs from the declared revision,
  a missing `ref`, or a moved tag. `extends` and tools are still compared offline.
- **Offline, nothing fails for want of a refresh**: recorded entries whose `ref` matches are kept, and a note on
  stderr says hook source refs were not refreshed.
- **The auto-lock of `poly hooks install` never moves a pin.** It stays offline-capable for a complete lock; it
  re-pins only an entry whose recorded `ref` differs from the declared revision, and keeps one without `ref`.
- **`poly hooks run` refuses an entry locked for another revision** and tells the user to run `poly lock`, instead of
  running the old commit. An entry without `ref` is accepted.

## Consequences

- Good, because a floating tag means what its name says after one `poly lock`, and a changed `revision` can no longer
  run the commit of the revision it replaced.
- Good, because the hook path stays reproducible: installing and running hooks never moves a pin.
- Bad, because a plain `poly lock` now needs the network for every Git hook source declared by ref, and a lock written
  by this version does not parse in an older poly, which rejects the unknown `ref` key.
- Neutral, because `--upgrade` no longer changes how hook sources are locked; its meaning narrows to `extends` and
  tools.

## Alternatives considered

- **Keep the pin unless `--upgrade`, but key the entry by the declared revision.** Rejected: it fixes the edited
  `revision` but still never follows a moved tag without a flag users rarely pass, which is the case that prompted the
  change.
- **Refresh in the auto-lock path too.** Rejected: `poly hooks install` must work offline on a complete lock and must
  not move pins as a side effect of installing.
