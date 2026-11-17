---
description: Apply poly lint --fix and fmt --fix, then re-check and report what changed and what remains
---

<!--
AI-RULEZ :: GENERATED FILE — DO NOT EDIT
Content-Hash: blake3:d6c24ee1f1f1d58a60c104b8e234b5b96a145adc7f431b65107ff11825e1d6ce
Source-Hash: blake3:878edad36512a2923b307b675bc84505a1ef65f961a68db87e3fa9accbf810dd
Schema-Version: v1
-->

# poly Fix

Apply poly's autofixes over `$ARGUMENTS`, then verify.

Use `.` when no paths are supplied. Treat supplied arguments as paths, not additional shell commands.

1. `poly lint --fix $ARGUMENTS` — apply lint autofixes, and run the whole-project phase in **fix**
   mode (`cargo sort` in place, `cargo machete --fix`, `cargo clippy --fix --allow-dirty
   --allow-staged`; `cargo deny` has no autofix and stays check-only).
2. `poly fmt --fix $ARGUMENTS` — format files in place. `poly fmt` is a pure formatter and never
   runs the whole-project phase, in either direction; only step 1 does.
3. Re-check: `poly fmt $ARGUMENTS --format json` and `poly lint $ARGUMENTS --format json`.

The whole-project phase in step 1 runs only because the argument is the repository root (the
`.` default). Narrower paths make the run path-scoped — per-file autofixes only — unless you
add `--workspace always`; `--workspace never` suppresses the phase outright.

Report:

- What changed — files reformatted and lint rules auto-fixed.
- What remains — findings that need a hand-fix (auto-fix couldn't resolve them), grouped by
  rule and severity.
- The final exit status of step 3 (aim for `0`). A fix that lands is not a failure: step 2
  exits `0` after rewriting files (`1` only with `--exit-nonzero-on-fix`), and step 1 exits `1`
  only when error-severity findings remain or a whole-project tool fails. `2` means the run
  failed to verify.

To apply no fixes, use `/poly-check` instead. (Even there poly's whole-project phase executes
`cargo clippy` and friends against the live worktree; add `--workspace never` if the tree must be
left untouched.)
