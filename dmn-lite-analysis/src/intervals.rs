//! Interval arithmetic over `i64` for static range analysis.
//!
//! An `IntervalSet` is a sorted, disjoint union of intervals.  Operations
//! (union, intersect, complement) preserve normalisation: the resulting set is
//! always sorted with no overlap and no adjacency.
//!
//! Profile v0.1 supports `i64` only.  Decimal (`f64`) intervals are deferred to
//! Profile v0.3; decimal-typed fields are marked opaque in higher-level analyses.

use std::cmp::Ordering;

/// A single interval over `i64`.
///
/// `lower <= upper` is an invariant of well-formed intervals (after normalisation
/// in `IntervalSet`).  Inclusive/exclusive bounds are explicit on each side; an
/// `Unbounded` bound represents `-∞` (lower) or `+∞` (upper).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Interval {
    /// Lower bound.
    pub lower: Bound,
    /// Upper bound.
    pub upper: Bound,
}

/// Bound of an interval.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bound {
    /// Inclusive bound at this integer value.
    Inclusive(i64),
    /// Exclusive bound at this integer value.
    Exclusive(i64),
    /// Unbounded (`-∞` for lower, `+∞` for upper).
    Unbounded,
}

/// Sorted, disjoint union of `Interval`s.
///
/// Invariants:
/// - Intervals are sorted by their lower bound.
/// - Adjacent intervals are merged (no two intervals share or touch a boundary).
/// - Empty intervals are not stored.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct IntervalSet {
    /// The intervals, in ascending order with no overlap or adjacency.
    pub intervals: Vec<Interval>,
}

impl IntervalSet {
    /// The empty set.
    pub fn empty() -> Self {
        Self {
            intervals: Vec::new(),
        }
    }

    /// The full set `(-∞, +∞)`.
    pub fn full() -> Self {
        Self {
            intervals: vec![Interval {
                lower: Bound::Unbounded,
                upper: Bound::Unbounded,
            }],
        }
    }

    /// A set containing exactly one integer value.
    pub fn singleton(v: i64) -> Self {
        Self {
            intervals: vec![Interval {
                lower: Bound::Inclusive(v),
                upper: Bound::Inclusive(v),
            }],
        }
    }

    /// A set containing one closed/open-ended interval.
    pub fn from_range(
        lower: Option<i64>,
        upper: Option<i64>,
        lower_inclusive: bool,
        upper_inclusive: bool,
    ) -> Self {
        let lo = match lower {
            None => Bound::Unbounded,
            Some(v) if lower_inclusive => Bound::Inclusive(v),
            Some(v) => Bound::Exclusive(v),
        };
        let up = match upper {
            None => Bound::Unbounded,
            Some(v) if upper_inclusive => Bound::Inclusive(v),
            Some(v) => Bound::Exclusive(v),
        };
        let i = Interval {
            lower: lo,
            upper: up,
        };
        if interval_is_empty(&i) {
            Self::empty()
        } else {
            Self { intervals: vec![i] }
        }
    }

    /// True when the set has no values.
    pub fn is_empty(&self) -> bool {
        self.intervals.is_empty()
    }

    /// True when `value` falls within any interval.
    pub fn contains(&self, value: i64) -> bool {
        self.intervals.iter().any(|i| interval_contains(i, value))
    }

    /// Union of two interval sets.
    pub fn union(&self, other: &Self) -> Self {
        let mut all: Vec<Interval> = self
            .intervals
            .iter()
            .chain(other.intervals.iter())
            .cloned()
            .collect();
        if all.is_empty() {
            return Self::empty();
        }
        all.sort_by(|a, b| cmp_lower_bound(&a.lower, &b.lower));
        let mut out: Vec<Interval> = Vec::with_capacity(all.len());
        for next in all {
            if let Some(last) = out.last_mut()
                && intervals_touch_or_overlap(last, &next)
            {
                last.upper = max_upper_bound(&last.upper, &next.upper);
                continue;
            }
            out.push(next);
        }
        Self { intervals: out }
    }

    /// Intersection of two interval sets.
    pub fn intersect(&self, other: &Self) -> Self {
        let mut out: Vec<Interval> = Vec::new();
        for a in &self.intervals {
            for b in &other.intervals {
                if let Some(i) = interval_intersect(a, b) {
                    out.push(i);
                }
            }
        }
        // Normalize (merge adjacent/overlapping) — possible if same bound appears.
        if out.is_empty() {
            return Self::empty();
        }
        out.sort_by(|x, y| cmp_lower_bound(&x.lower, &y.lower));
        let mut merged: Vec<Interval> = Vec::with_capacity(out.len());
        for next in out {
            if let Some(last) = merged.last_mut()
                && intervals_touch_or_overlap(last, &next)
            {
                last.upper = max_upper_bound(&last.upper, &next.upper);
                continue;
            }
            merged.push(next);
        }
        Self { intervals: merged }
    }

    /// Complement (over the universe `i64`).
    pub fn complement(&self) -> Self {
        if self.intervals.is_empty() {
            return Self::full();
        }
        let mut out: Vec<Interval> = Vec::new();
        // Piece before the first interval: `(-∞, intervals[0].lower)`.
        if let Some(first) = self.intervals.first()
            && first.lower != Bound::Unbounded
        {
            let piece = Interval {
                lower: Bound::Unbounded,
                upper: flip_lower_to_upper(first.lower),
            };
            if !interval_is_empty(&piece) {
                out.push(piece);
            }
        }
        // Pieces between consecutive intervals.
        for w in self.intervals.windows(2) {
            let prev_upper = w[0].upper;
            let next_lower = w[1].lower;
            if prev_upper != Bound::Unbounded && next_lower != Bound::Unbounded {
                let piece = Interval {
                    lower: flip_upper_to_lower(prev_upper),
                    upper: flip_lower_to_upper(next_lower),
                };
                if !interval_is_empty(&piece) {
                    out.push(piece);
                }
            }
        }
        // Piece after the last interval: `(intervals[n].upper, +∞)`.
        if let Some(last) = self.intervals.last()
            && last.upper != Bound::Unbounded
        {
            let piece = Interval {
                lower: flip_upper_to_lower(last.upper),
                upper: Bound::Unbounded,
            };
            if !interval_is_empty(&piece) {
                out.push(piece);
            }
        }
        Self { intervals: out }
    }
}

// ── Single-interval helpers ───────────────────────────────────────────────────

fn interval_contains(iv: &Interval, v: i64) -> bool {
    let lower_ok = match iv.lower {
        Bound::Unbounded => true,
        Bound::Inclusive(b) => v >= b,
        Bound::Exclusive(b) => v > b,
    };
    let upper_ok = match iv.upper {
        Bound::Unbounded => true,
        Bound::Inclusive(b) => v <= b,
        Bound::Exclusive(b) => v < b,
    };
    lower_ok && upper_ok
}

fn interval_is_empty(iv: &Interval) -> bool {
    match (iv.lower, iv.upper) {
        (Bound::Inclusive(a), Bound::Inclusive(b)) => a > b,
        (Bound::Inclusive(a), Bound::Exclusive(b)) => a >= b,
        (Bound::Exclusive(a), Bound::Inclusive(b)) => a >= b,
        (Bound::Exclusive(a), Bound::Exclusive(b)) => a + 1 >= b,
        _ => false,
    }
}

fn interval_intersect(a: &Interval, b: &Interval) -> Option<Interval> {
    let lower = max_lower_bound(&a.lower, &b.lower);
    let upper = min_upper_bound(&a.upper, &b.upper);
    let out = Interval { lower, upper };
    if interval_is_empty(&out) {
        None
    } else {
        Some(out)
    }
}

fn intervals_touch_or_overlap(a: &Interval, b: &Interval) -> bool {
    // Assumes a comes before b in lower-bound order. They touch if b.lower
    // <= a.upper, or if a.upper is exclusive at v and b.lower is inclusive at v.
    match (a.upper, b.lower) {
        (Bound::Unbounded, _) => true,
        (_, Bound::Unbounded) => false, // b lower is -∞ but a is finite; can't reach here if sorted
        (Bound::Inclusive(au), Bound::Inclusive(bl)) => bl <= au + 1, // adjacent integers also merge
        (Bound::Inclusive(au), Bound::Exclusive(bl)) => bl <= au,
        (Bound::Exclusive(au), Bound::Inclusive(bl)) => bl <= au,
        (Bound::Exclusive(au), Bound::Exclusive(bl)) => bl < au,
    }
}

// ── Bound comparison ──────────────────────────────────────────────────────────

fn cmp_lower_bound(a: &Bound, b: &Bound) -> Ordering {
    match (a, b) {
        (Bound::Unbounded, Bound::Unbounded) => Ordering::Equal,
        (Bound::Unbounded, _) => Ordering::Less,
        (_, Bound::Unbounded) => Ordering::Greater,
        (Bound::Inclusive(x), Bound::Inclusive(y)) | (Bound::Exclusive(x), Bound::Exclusive(y)) => {
            x.cmp(y)
        }
        // Inclusive at v comes before Exclusive at v (Inclusive includes v in the set,
        // so its lower bound is "lower" on the number line).
        (Bound::Inclusive(x), Bound::Exclusive(y)) => x.cmp(y).then(Ordering::Less),
        (Bound::Exclusive(x), Bound::Inclusive(y)) => x.cmp(y).then(Ordering::Greater),
    }
}

fn max_lower_bound(a: &Bound, b: &Bound) -> Bound {
    // Largest lower bound (tightest constraint).
    match cmp_lower_bound(a, b) {
        Ordering::Less => *b,
        _ => *a,
    }
}

fn min_upper_bound(a: &Bound, b: &Bound) -> Bound {
    // Smallest upper bound (tightest constraint).
    // For upper bounds, Inclusive(v) is *greater* than Exclusive(v) since the
    // inclusive version includes v.
    match cmp_upper_bound(a, b) {
        Ordering::Less => *a,
        _ => *b,
    }
}

fn max_upper_bound(a: &Bound, b: &Bound) -> Bound {
    match cmp_upper_bound(a, b) {
        Ordering::Greater => *a,
        _ => *b,
    }
}

fn cmp_upper_bound(a: &Bound, b: &Bound) -> Ordering {
    match (a, b) {
        (Bound::Unbounded, Bound::Unbounded) => Ordering::Equal,
        (Bound::Unbounded, _) => Ordering::Greater,
        (_, Bound::Unbounded) => Ordering::Less,
        (Bound::Inclusive(x), Bound::Inclusive(y)) | (Bound::Exclusive(x), Bound::Exclusive(y)) => {
            x.cmp(y)
        }
        (Bound::Inclusive(x), Bound::Exclusive(y)) => x.cmp(y).then(Ordering::Greater),
        (Bound::Exclusive(x), Bound::Inclusive(y)) => x.cmp(y).then(Ordering::Less),
    }
}

/// Convert an upper bound `≤ v` / `< v` into the corresponding lower bound `> v` / `≥ v`.
fn flip_upper_to_lower(b: Bound) -> Bound {
    match b {
        Bound::Unbounded => Bound::Unbounded,
        Bound::Inclusive(v) => Bound::Exclusive(v),
        Bound::Exclusive(v) => Bound::Inclusive(v),
    }
}

/// Convert a lower bound `≥ v` / `> v` into the corresponding upper bound `< v` / `≤ v`.
fn flip_lower_to_upper(b: Bound) -> Bound {
    match b {
        Bound::Unbounded => Bound::Unbounded,
        Bound::Inclusive(v) => Bound::Exclusive(v),
        Bound::Exclusive(v) => Bound::Inclusive(v),
    }
}

// ── Pick a witness ────────────────────────────────────────────────────────────

impl IntervalSet {
    /// Pick a representative integer from the set, or `None` if empty.
    /// Used as a gap witness when integer fields have uncovered regions.
    pub fn pick_witness(&self) -> Option<i64> {
        for iv in &self.intervals {
            if let Some(v) = interval_witness(iv) {
                return Some(v);
            }
        }
        None
    }
}

fn interval_witness(iv: &Interval) -> Option<i64> {
    if interval_is_empty(iv) {
        return None;
    }
    match (iv.lower, iv.upper) {
        (Bound::Inclusive(v), _) => Some(v),
        (Bound::Exclusive(v), _) => Some(v.checked_add(1)?),
        (Bound::Unbounded, Bound::Inclusive(v)) => Some(v),
        (Bound::Unbounded, Bound::Exclusive(v)) => Some(v.checked_sub(1)?),
        (Bound::Unbounded, Bound::Unbounded) => Some(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn singleton_contains_only_value() {
        let s = IntervalSet::singleton(5);
        assert!(s.contains(5));
        assert!(!s.contains(4));
        assert!(!s.contains(6));
    }

    #[test]
    fn range_inclusive_both_ends() {
        let s = IntervalSet::from_range(Some(5), Some(10), true, true);
        assert!(s.contains(5));
        assert!(s.contains(7));
        assert!(s.contains(10));
        assert!(!s.contains(4));
        assert!(!s.contains(11));
    }

    #[test]
    fn range_exclusive_upper() {
        let s = IntervalSet::from_range(Some(5), Some(10), true, false);
        assert!(s.contains(5));
        assert!(s.contains(9));
        assert!(!s.contains(10));
    }

    #[test]
    fn full_contains_everything() {
        let s = IntervalSet::full();
        assert!(s.contains(i64::MIN));
        assert!(s.contains(0));
        assert!(s.contains(i64::MAX));
    }

    #[test]
    fn empty_is_empty() {
        let s = IntervalSet::empty();
        assert!(s.is_empty());
        assert!(!s.contains(0));
    }

    #[test]
    fn union_disjoint() {
        let a = IntervalSet::from_range(Some(0), Some(10), true, true);
        let b = IntervalSet::from_range(Some(20), Some(30), true, true);
        let u = a.union(&b);
        assert!(u.contains(5));
        assert!(u.contains(25));
        assert!(!u.contains(15));
        assert_eq!(u.intervals.len(), 2);
    }

    #[test]
    fn union_merges_adjacent_inclusive() {
        let a = IntervalSet::from_range(Some(0), Some(10), true, true);
        let b = IntervalSet::from_range(Some(11), Some(20), true, true);
        let u = a.union(&b);
        assert_eq!(u.intervals.len(), 1);
        assert!(u.contains(0));
        assert!(u.contains(20));
    }

    #[test]
    fn intersect_overlap() {
        let a = IntervalSet::from_range(Some(0), Some(10), true, true);
        let b = IntervalSet::from_range(Some(5), Some(15), true, true);
        let i = a.intersect(&b);
        assert!(i.contains(5));
        assert!(i.contains(10));
        assert!(!i.contains(11));
        assert!(!i.contains(4));
    }

    #[test]
    fn intersect_disjoint_is_empty() {
        let a = IntervalSet::from_range(Some(0), Some(10), true, true);
        let b = IntervalSet::from_range(Some(20), Some(30), true, true);
        let i = a.intersect(&b);
        assert!(i.is_empty());
    }

    #[test]
    fn complement_of_finite_range() {
        let a = IntervalSet::from_range(Some(0), Some(10), true, true);
        let c = a.complement();
        assert!(c.contains(-1));
        assert!(c.contains(11));
        assert!(!c.contains(0));
        assert!(!c.contains(5));
        assert!(!c.contains(10));
    }

    #[test]
    fn complement_of_full_is_empty() {
        let f = IntervalSet::full();
        assert!(f.complement().is_empty());
    }

    #[test]
    fn pick_witness_finite() {
        let s = IntervalSet::from_range(Some(5), Some(10), true, true);
        assert_eq!(s.pick_witness(), Some(5));
    }
}
