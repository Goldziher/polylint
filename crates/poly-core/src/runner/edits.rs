//! Autofix edit application: committing a diagnostic's `fix` edits to a file's
//! contents.
//!
//! Split out of `runner.rs` so the runner keeps to the pipeline itself
//! (discover -> cache -> engine -> report) and edit arithmetic stays one concern
//! per file.

use std::collections::BTreeSet;

use crate::engine::Edit;

/// Apply autofix edit groups to `content`, one group per diagnostic.
///
/// Each group is the full `fix` vec of one [`Diagnostic`](crate::engine::Diagnostic) and is applied
/// **atomically**: all of its edits apply, or none do.
///
/// Selection rules (right-to-left):
/// 1. Any group whose own edits overlap each other internally is discarded
///    (prevents corrupted output from a malformed backend fix).
/// 2. Groups are attempted rightmost-first.  If any edit in a group overlaps
///    bytes already committed by a previously-applied group, the
///    entire group is skipped; the convergence loop in `lint_one` will retry
///    it on the next pass once those diagnostics have been re-evaluated.
///
/// Returns the rewritten text and how many groups were committed, or `None` if
/// no edit was applied. The count is what lets the report say how many issues a
/// `--fix` run resolved rather than only how many survived it.
pub(super) fn apply_edits(content: &str, edit_groups: &[&[Edit]]) -> Option<(String, usize)> {
    apply_edits_indexed(content, edit_groups).map(|(result, applied)| (result, applied.len()))
}

/// [`apply_edits`], additionally naming which groups were committed (their positions in
/// `edit_groups`, ascending), for a caller that must drop exactly the diagnostics it resolved.
pub(super) fn apply_edits_indexed(content: &str, edit_groups: &[&[Edit]]) -> Option<(String, Vec<usize>)> {
    let mut valid: Vec<(usize, &[Edit])> = edit_groups
        .iter()
        .copied()
        .enumerate()
        .filter(|(_, g)| !g.is_empty() && !has_internal_overlap(g))
        .collect();
    valid.sort_by_key(|(_, g)| std::cmp::Reverse(g.iter().map(|e| e.end_byte).max().unwrap_or(0)));

    let mut committed: BTreeSet<(usize, usize)> = BTreeSet::new();
    let mut accepted: Vec<Accepted<'_>> = Vec::new();
    let mut applied: Vec<usize> = Vec::new();

    'groups: for (index, group) in &valid {
        for e in *group {
            if e.start_byte > e.end_byte
                || e.end_byte > content.len()
                || !content.is_char_boundary(e.start_byte)
                || !content.is_char_boundary(e.end_byte)
                || overlaps_committed(&committed, e)
            {
                continue 'groups;
            }
        }

        let seq = applied.len();
        for e in *group {
            committed.insert((e.start_byte, e.end_byte));
            accepted.push(Accepted {
                start: e.start_byte,
                end: e.end_byte,
                replacement: &e.replacement,
                seq,
            });
        }
        applied.push(*index);
    }

    if applied.is_empty() {
        return None;
    }

    // At one position a pure insertion lands before the replacement that starts there, and of two
    // insertions the later-committed group's text comes first.
    accepted.sort_unstable_by_key(|a| (a.start, a.start != a.end, std::cmp::Reverse(a.seq)));
    let added: usize = accepted.iter().map(|a| a.replacement.len()).sum();
    let removed: usize = accepted.iter().map(|a| a.end - a.start).sum();
    let mut result = String::with_capacity(content.len() + added - removed);
    let mut pos = 0;
    for a in &accepted {
        result.push_str(&content[pos..a.start]);
        result.push_str(a.replacement);
        pos = a.end;
    }
    result.push_str(&content[pos..]);

    applied.sort_unstable();
    Some((result, applied))
}

/// An edit that passed validation, queued for the single forward rewrite.
struct Accepted<'a> {
    start: usize,
    end: usize,
    replacement: &'a str,
    /// Commit order of the owning group; breaks ties between insertions at one position.
    seq: usize,
}

/// Whether `e` intersects an already-committed range. Touching ranges and an insertion at the
/// boundary of a range do not conflict; an insertion strictly inside one does.
fn overlaps_committed(committed: &BTreeSet<(usize, usize)>, e: &Edit) -> bool {
    committed
        .range(..(e.end_byte, 0))
        .next_back()
        .is_some_and(|&(_, end)| end > e.start_byte)
}

/// Returns `true` when any two edits in `group` have overlapping byte ranges.
///
/// O(n²) — acceptable because fix groups are tiny (1–4 edits in practice).
fn has_internal_overlap(group: &[Edit]) -> bool {
    for (i, a) in group.iter().enumerate() {
        for b in group.iter().skip(i + 1) {
            let intersects = a.start_byte < b.end_byte && b.start_byte < a.end_byte;
            let same_point_insert =
                a.start_byte == a.end_byte && b.start_byte == b.end_byte && a.start_byte == b.start_byte;
            if intersects || same_point_insert {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edit(start: usize, end: usize, rep: &str) -> Edit {
        Edit {
            start_byte: start,
            end_byte: end,
            replacement: rep.to_owned(),
        }
    }
    /// Two diagnostics, each with two non-overlapping edits; all four apply.
    #[test]
    fn multi_edit_two_groups_apply_atomically() {
        let content = "hello world foo";
        let group_a = vec![edit(6, 11, "earth"), edit(12, 15, "bar")];
        let group_b = vec![edit(0, 5, "hey")];

        let (result, applied) =
            apply_edits(content, &[group_a.as_slice(), group_b.as_slice()]).expect("should produce output");
        assert_eq!(result, "hey earth bar");
        assert_eq!(applied, 2, "both diagnostics' fixes were committed");
    }

    /// A diagnostic whose edits overlap each other is skipped entirely.
    #[test]
    fn overlapping_edits_within_group_are_skipped() {
        let content = "abcdefgh";
        let bad_group = vec![edit(2, 6, "X"), edit(4, 8, "Y")];

        let result = apply_edits(content, &[bad_group.as_slice()]);
        assert!(result.is_none(), "overlapping group must produce no output");
    }

    /// When two groups from different diagnostics conflict, the leftward group
    /// is deferred (not corrupted).
    #[test]
    fn cross_group_conflict_defers_leftward_group() {
        let content = "abcde";
        let group_a = vec![edit(3, 5, "XX")];
        let group_b = vec![edit(2, 4, "YY")];

        let (result, applied) = apply_edits(content, &[group_a.as_slice(), group_b.as_slice()])
            .expect("should produce output from group A");
        assert_eq!(result, "abcXX");
        assert_eq!(
            applied, 1,
            "the deferred group must not be counted as fixed — it is retried next pass"
        );
    }

    #[test]
    fn non_overlapping_edits_pass_internal_check() {
        let group = vec![edit(0, 5, "a"), edit(5, 10, "b")];
        assert!(!has_internal_overlap(&group));
    }

    #[test]
    fn adjacent_edits_are_not_overlapping() {
        let group = vec![edit(0, 5, "a"), edit(5, 10, "b")];
        assert!(!has_internal_overlap(&group));
    }

    #[test]
    fn touching_edits_with_overlap_detected() {
        let group = vec![edit(0, 6, "a"), edit(4, 10, "b")];
        assert!(has_internal_overlap(&group));
    }

    /// Reference: edits applied right-to-left with `replace_range`.
    fn naive(content: &str, edits: &[Edit]) -> String {
        let mut sorted: Vec<&Edit> = edits.iter().collect();
        sorted.sort_by_key(|e| std::cmp::Reverse(e.start_byte));
        let mut out = content.to_string();
        for e in sorted {
            out.replace_range(e.start_byte..e.end_byte, &e.replacement);
        }
        out
    }

    #[test]
    fn many_single_byte_deletions_match_the_naive_result() {
        let content: String = (0..1_000_000u32).map(|i| char::from(b'a' + (i % 26) as u8)).collect();
        let edits: Vec<Edit> = (0..10_000).map(|i| edit(i * 100 + 7, i * 100 + 8, "")).collect();
        let groups: Vec<&[Edit]> = edits.chunks(1).collect();
        let (result, applied) = apply_edits(&content, &groups).expect("edits apply");
        assert_eq!(applied, 10_000);
        assert_eq!(result, naive(&content, &edits));
        assert_eq!(result.len(), content.len() - 10_000);
    }

    #[test]
    fn insert_at_start_and_end() {
        let edits = vec![edit(0, 0, "<"), edit(3, 3, ">")];
        let groups: Vec<&[Edit]> = edits.chunks(1).collect();
        let (result, applied) = apply_edits("abc", &groups).unwrap();
        assert_eq!(result, "<abc>");
        assert_eq!(applied, 2);
    }

    #[test]
    fn adjacent_edits_in_one_group() {
        let group = vec![edit(0, 2, "X"), edit(2, 4, "YY"), edit(4, 5, "")];
        let (result, _) = apply_edits("abcde", &[group.as_slice()]).unwrap();
        assert_eq!(result, naive("abcde", &group));
        assert_eq!(result, "XYY");
    }

    #[test]
    fn empty_replacement_deletes() {
        let group = vec![edit(1, 4, "")];
        let (result, _) = apply_edits("abcde", &[group.as_slice()]).unwrap();
        assert_eq!(result, "ae");
    }

    #[test]
    fn multibyte_edits_on_char_boundaries() {
        let content = "h\u{e9}llo \u{4e16}\u{754c}!";
        let group = vec![edit(1, 3, "e"), edit(7, 10, "w"), edit(10, 13, "")];
        let (result, _) = apply_edits(content, &[group.as_slice()]).unwrap();
        assert_eq!(result, "hello w!");
    }

    #[test]
    fn edit_inside_a_multibyte_char_is_rejected() {
        let group = vec![edit(2, 3, "x")];
        assert!(apply_edits("h\u{e9}llo", &[group.as_slice()]).is_none());
    }

    #[test]
    fn out_of_range_and_inverted_edits_are_rejected() {
        let past_end = vec![edit(1, 9, "x")];
        let inverted = vec![edit(3, 1, "x")];
        assert!(apply_edits("abc", &[past_end.as_slice(), inverted.as_slice()]).is_none());
    }

    /// A group sitting between the two edits of another group does not overlap it, so it is
    /// applied in the same pass.
    #[test]
    fn group_between_another_groups_edits_applies_in_the_same_pass() {
        let content = "0123456789".repeat(6);
        let outer = vec![edit(0, 2, "A"), edit(50, 52, "B")];
        let between = vec![edit(20, 22, "C")];
        let (result, applied) = apply_edits(&content, &[outer.as_slice(), between.as_slice()]).unwrap();
        assert_eq!(applied, 2);
        let all: Vec<Edit> = outer.iter().chain(between.iter()).cloned().collect();
        assert_eq!(result, naive(&content, &all));
    }

    /// A group overlapping an earlier group's committed edit is still deferred.
    #[test]
    fn group_overlapping_any_committed_edit_is_deferred() {
        let content = "0123456789".repeat(6);
        let outer = vec![edit(0, 2, "A"), edit(50, 60, "B")];
        let clash = vec![edit(20, 22, "C"), edit(52, 56, "D")];
        let (result, applied) = apply_edits(&content, &[outer.as_slice(), clash.as_slice()]).unwrap();
        assert_eq!(applied, 1);
        assert_eq!(result, naive(&content, &outer));
    }

    #[test]
    fn indexed_reports_committed_group_positions() {
        let a = vec![edit(3, 5, "XX")];
        let b = vec![edit(2, 4, "YY")];
        let c = vec![edit(0, 1, "Z")];
        let (_, applied) = apply_edits_indexed("abcde", &[a.as_slice(), b.as_slice(), c.as_slice()]).unwrap();
        assert_eq!(applied, vec![0, 2]);
    }
}
