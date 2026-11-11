//! A latency histogram in fixed memory: values below 64 exactly, above that
//! 32 logarithmic buckets per power of two (at most 3.2% relative error), 1,920
//! counters in all. Values are microseconds.

const EXACT: u64 = 64;
const SUB_BITS: u32 = 5;
const SUB: u64 = 1 << SUB_BITS;
const BUCKETS: usize = (EXACT + (64 - 6) * SUB) as usize;

#[derive(Clone)]
pub struct Hist {
    counts: Box<[u64; BUCKETS]>,
    n: u64,
    max: u64,
}

impl Default for Hist {
    fn default() -> Self {
        Self {
            counts: Box::new([0; BUCKETS]),
            n: 0,
            max: 0,
        }
    }
}

fn index(v: u64) -> usize {
    if v < EXACT {
        return v as usize;
    }
    let e = 63 - u64::from(v.leading_zeros());
    let m = (v >> (e - u64::from(SUB_BITS))) & (SUB - 1);
    (EXACT + (e - 6) * SUB + m) as usize
}

/// The smallest value in bucket `i`.
fn lower(i: usize) -> u64 {
    let i = i as u64;
    if i < EXACT {
        return i;
    }
    let (e, m) = ((i - EXACT) / SUB + 6, (i - EXACT) % SUB);
    (SUB + m) << (e - u64::from(SUB_BITS))
}

impl Hist {
    pub fn record(&mut self, micros: u64) {
        if let Some(c) = self.counts.get_mut(index(micros)) {
            *c += 1;
        }
        self.n += 1;
        self.max = self.max.max(micros);
    }

    pub fn count(&self) -> u64 {
        self.n
    }

    pub fn max(&self) -> u64 {
        self.max
    }

    /// The value at quantile `q` (0 to 1): the top of its bucket, never above
    /// the largest value recorded; 0 when empty.
    pub fn quantile(&self, q: f64) -> u64 {
        if self.n == 0 {
            return 0;
        }
        let rank = ((q * self.n as f64).ceil() as u64).clamp(1, self.n);
        let mut seen = 0;
        for (i, c) in self.counts.iter().enumerate() {
            seen += c;
            if seen >= rank {
                if i + 1 >= BUCKETS {
                    return self.max;
                }
                return lower(i + 1).saturating_sub(1).min(self.max);
            }
        }
        self.max
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every value's bucket holds it, and buckets tile the range.
    #[test]
    fn buckets_tile_the_range() {
        for i in 0..BUCKETS - 1 {
            assert!(lower(i + 1) > lower(i));
            assert_eq!(index(lower(i)), i);
            assert_eq!(index(lower(i + 1) - 1), i);
        }
        assert_eq!(index(u64::MAX), BUCKETS - 1);
    }

    /// Percentiles of 1..=100,000 µs are within the bucket error of the truth.
    #[test]
    fn quantiles_of_a_uniform_distribution() {
        let mut h = Hist::default();
        for v in 1..=100_000 {
            h.record(v);
        }
        for (q, truth) in [(0.5, 50_000.0), (0.95, 95_000.0), (0.99, 99_000.0)] {
            let got = h.quantile(q) as f64;
            assert!((got - truth).abs() / truth <= 1.0 / 32.0, "q{q}: {got}");
        }
        assert_eq!(
            (h.quantile(1.0), h.max(), h.count()),
            (100_000, 100_000, 100_000)
        );
        assert_eq!(h.quantile(0.0), 1);
    }
}
