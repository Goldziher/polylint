---
description: Apply poly lint --fix and fmt --fix, then re-check and report what changed and what remains
priority: high
usage: /poly-fix [paths]
---

<!--
AI-RULEZ :: GENERATED FILE — DO NOT EDIT
Content-Hash: blake3:6c4829d259e62209da08c9fa072488da04fcdcd1360080aad2402dc09087ad14
Source-Hash: blake3:ea06cdb8524784872da741d339272b44a5c4d9291a0a23fde7e56c89c63eefbe
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
