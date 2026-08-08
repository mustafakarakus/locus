//! Benchmark statistics helpers (U-012): percentile computation over recorded
//! latencies. Percentiles are computed with the nearest-rank method, matching
//! how the performance budget p50/p95/p99 targets in `TECHSTACK.md` are stated.

/// A percentile summary of a latency sample.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Percentiles {
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
}

/// Computes the `q`-th percentile (0.0..=1.0) of a sorted, non-empty sample.
///
/// Uses nearest-rank: the percentile is the smallest sample value at rank
/// `ceil(q * n)`.
pub fn percentile(sorted: &[f64], q: f64) -> f64 {
    assert!(!sorted.is_empty(), "percentile of an empty sample");
    assert!((0.0..=1.0).contains(&q), "percentile must be in [0,1]");
    let rank = (q * sorted.len() as f64).ceil() as usize;
    sorted[(rank.max(1) - 1).min(sorted.len() - 1)]
}

/// Summarizes a latency sample (milliseconds) into p50/p95/p99.
///
/// The input is consumed and sorted in place; pass in an owned copy if the
/// original order matters. Durations that are already seconds should be
/// converted to milliseconds before calling.
pub fn summarize(latencies_ms: &mut [f64]) -> Percentiles {
    latencies_ms.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    Percentiles {
        p50_ms: percentile(latencies_ms, 0.50),
        p95_ms: percentile(latencies_ms, 0.95),
        p99_ms: percentile(latencies_ms, 0.99),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_rank_matches_reference() {
        let mut samples = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let p = summarize(&mut samples);
        assert_eq!(p.p50_ms, 3.0);
        assert_eq!(p.p95_ms, 5.0);
        assert_eq!(p.p99_ms, 5.0);
    }

    #[test]
    fn single_sample_percentiles_equal_the_value() {
        let mut samples = vec![7.5];
        let p = summarize(&mut samples);
        assert_eq!(p.p50_ms, 7.5);
        assert_eq!(p.p95_ms, 7.5);
        assert_eq!(p.p99_ms, 7.5);
    }

    #[test]
    fn summarize_sorts_in_place() {
        let mut samples = vec![100.0, 1.0, 50.0];
        let p = summarize(&mut samples);
        assert_eq!(samples, vec![1.0, 50.0, 100.0]);
        assert_eq!(p.p50_ms, 50.0);
        assert_eq!(p.p95_ms, 100.0);
    }

    #[test]
    #[should_panic]
    fn percentile_of_empty_panics() {
        percentile(&[], 0.95);
    }
}
