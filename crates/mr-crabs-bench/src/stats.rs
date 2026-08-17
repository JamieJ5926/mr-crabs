//! Aggregation arithmetic shared by the S12 gate.
//!
//! The Python gate driver (`verification/tools/release_gate.py`) mirrors
//! these exact rules (median of an odd/even sample, nearest-rank
//! percentiles) and validates itself against the same fixture the Rust
//! tests use, so cross-language drift is caught by either side's tests.

/// Median of `values` (sorted copy; odd n takes the middle, even n the
/// integer average of the two middles). `None` for an empty sample.
pub fn median(values: &mut [u64]) -> Option<u64> {
    if values.is_empty() {
        return None;
    }
    values.sort_unstable();
    let n = values.len();
    if n % 2 == 1 {
        Some(values[n / 2])
    } else {
        let a = u128::from(values[n / 2 - 1]);
        let b = u128::from(values[n / 2]);
        Some(((a + b) / 2) as u64)
    }
}

/// Nearest-rank percentile of an already-sorted ascending sample.
///
/// Rank is `ceil(p/100 * n)`, 1-based; `None` for an empty sample or a
/// percentile outside `0..=100`. Matches the driver's implementation.
pub fn percentile(sorted_ascending: &[u64], p: f64) -> Option<u64> {
    if sorted_ascending.is_empty() || !(0.0..=100.0).contains(&p) {
        return None;
    }
    let rank_f = (p / 100.0) * sorted_ascending.len() as f64;
    let rank = rank_f.ceil().max(1.0) as usize;
    let index = rank.min(sorted_ascending.len()) - 1;
    Some(sorted_ascending[index])
}

#[cfg(test)]
mod tests {
    use super::{median, percentile};

    /// Values mirrored from verification/corpus/replay/aggregate-math-fixture.json.
    #[test]
    fn median_odd_sample() {
        assert_eq!(median(&mut [5, 1, 4, 2, 3]), Some(3));
    }

    #[test]
    fn median_even_sample() {
        // (5 + 8) / 2 = 6 (integer average of the two middle values).
        assert_eq!(median(&mut [8, 3, 5, 10]), Some(6));
    }

    #[test]
    fn median_empty_and_single() {
        assert_eq!(median(&mut []), None);
        assert_eq!(median(&mut [7]), Some(7));
    }

    #[test]
    fn nearest_rank_percentiles_n5() {
        let sorted = [1, 2, 3, 4, 5];
        assert_eq!(percentile(&sorted, 50.0), Some(3));
        assert_eq!(percentile(&sorted, 90.0), Some(5));
        assert_eq!(percentile(&sorted, 95.0), Some(5));
        assert_eq!(percentile(&sorted, 99.0), Some(5));
    }

    #[test]
    fn nearest_rank_percentiles_n5_dup() {
        let sorted = [10, 20, 30, 40, 50];
        assert_eq!(percentile(&sorted, 50.0), Some(30));
        assert_eq!(percentile(&sorted, 95.0), Some(50));
        assert_eq!(percentile(&sorted, 99.0), Some(50));
    }

    #[test]
    fn nearest_rank_percentiles_n1() {
        let sorted = [7];
        assert_eq!(percentile(&sorted, 50.0), Some(7));
        assert_eq!(percentile(&sorted, 95.0), Some(7));
        assert_eq!(percentile(&sorted, 99.0), Some(7));
    }

    #[test]
    fn percentile_rejects_invalid_input() {
        assert_eq!(percentile(&[], 50.0), None);
        assert_eq!(percentile(&[1, 2], 100.1), None);
        assert_eq!(percentile(&[1, 2], -0.1), None);
    }
}
