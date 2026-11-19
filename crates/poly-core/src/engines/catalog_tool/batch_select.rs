//! Running a batch through a tool that takes only some of its members, and putting the answers back.

use crate::engine::SourceFile;

/// The members of `batch` that satisfy `keep`, and where each came from.
pub(super) fn select(batch: &[SourceFile], keep: impl Fn(&SourceFile) -> bool) -> (Vec<&SourceFile>, Vec<usize>) {
    batch
        .iter()
        .enumerate()
        .filter(|(_, src)| keep(src))
        .map(|(index, src)| (src, index))
        .unzip()
}

/// Spread `answers` (one per selected member) back to their positions in a batch
/// of `len`, with `otherwise` for every member that was not selected.
pub(super) fn scatter<T>(
    len: usize,
    positions: &[usize],
    answers: &mut impl Iterator<Item = T>,
    otherwise: impl Fn() -> T,
) -> Vec<T> {
    let mut out: Vec<T> = (0..len).map(|_| otherwise()).collect();
    for &position in positions {
        if let Some(answer) = answers.next() {
            out[position] = answer;
        }
    }
    out
}
