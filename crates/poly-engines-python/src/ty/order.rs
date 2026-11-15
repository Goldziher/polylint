//! Restoring the serial report order from results that finish in any order.

/// Order `results` by the index each carries, so a parallel check reports as a serial one does.
pub(super) fn merge_by_index<T>(mut results: Vec<(usize, T)>) -> Vec<T> {
    results.sort_unstable_by_key(|(index, _)| *index);
    results.into_iter().map(|(_, value)| value).collect()
}

#[cfg(test)]
mod tests {
    use super::merge_by_index;

    #[test]
    fn shuffled_completion_order_merges_to_serial_order() {
        let shuffled = vec![(3, "d"), (0, "a"), (4, "e"), (2, "c"), (1, "b")];
        assert_eq!(merge_by_index(shuffled), ["a", "b", "c", "d", "e"]);
    }

    #[test]
    fn empty_input_merges_to_nothing() {
        assert!(merge_by_index(Vec::<(usize, u8)>::new()).is_empty());
    }
}
