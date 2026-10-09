use std::sync::Arc;

use crate::Rational;

/// A stretch of cycle time, `start` up to `end`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Span {
    pub start: Rational,
    pub end: Rational,
}

impl Span {
    pub fn new(start: Rational, end: Rational) -> Self {
        Span { start, end }
    }

    pub fn len(self) -> Rational {
        self.end - self.start
    }

    /// A span that is a single instant.
    pub fn is_instant(self) -> bool {
        self.start == self.end
    }

    /// The pieces of this span within each whole cycle. An instant is its own
    /// single piece.
    pub fn cycles(self) -> Vec<Span> {
        if self.is_instant() {
            return vec![self];
        }
        let mut out = Vec::new();
        let mut start = self.start;
        while start < self.end {
            let next = Rational::int(start.floor() + 1).min(self.end);
            out.push(Span::new(start, next));
            start = next;
        }
        out
    }

    /// The overlap of two spans, if any. Spans that only touch overlap in
    /// nothing, except that an instant inside a span overlaps it.
    pub fn intersection(self, other: Span) -> Option<Span> {
        let start = self.start.max(other.start);
        let end = self.end.min(other.end);
        if start < end {
            Some(Span::new(start, end))
        } else if start == end && (self.is_instant() || other.is_instant()) {
            // an instant belongs to the span that starts at or before it and ends after it
            let inside = |s: Span| s.is_instant() || (s.start <= start && start < s.end);
            (inside(self) && inside(other)).then_some(Span::new(start, end))
        } else {
            None
        }
    }

    pub fn map(self, f: impl Fn(Rational) -> Rational) -> Span {
        Span::new(f(self.start), f(self.end))
    }
}

/// One event: what happens, over what `whole` stretch of time, and which `part`
/// of that was asked for. A query that cuts an event in half returns the same
/// `whole` with different `part`s; only the piece starting where the whole
/// starts has the onset.
#[derive(Clone, Debug, PartialEq)]
pub struct Hap<T> {
    pub whole: Option<Span>,
    pub part: Span,
    pub value: T,
}

impl<T> Hap<T> {
    pub fn has_onset(&self) -> bool {
        self.whole.is_some_and(|w| w.start == self.part.start)
    }

    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Hap<U> {
        Hap { whole: self.whole, part: self.part, value: f(self.value) }
    }

    fn map_times(self, f: impl Fn(Rational) -> Rational) -> Hap<T> {
        Hap { whole: self.whole.map(|w| w.map(&f)), part: self.part.map(&f), value: self.value }
    }
}

type Query<T> = dyn Fn(Span) -> Vec<Hap<T>> + Send + Sync;

/// Values arranged over cycles of time. A pattern is a function from a span of
/// time to the events in it, so it can be queried for any stretch, however far
/// along, without being unrolled.
pub struct Pattern<T> {
    query: Arc<Query<T>>,
}

impl<T> Clone for Pattern<T> {
    fn clone(&self) -> Self {
        Pattern { query: self.query.clone() }
    }
}

impl<T: Clone + Send + Sync + 'static> Pattern<T> {
    pub fn new(query: impl Fn(Span) -> Vec<Hap<T>> + Send + Sync + 'static) -> Self {
        Pattern { query: Arc::new(query) }
    }

    pub fn query(&self, span: Span) -> Vec<Hap<T>> {
        (self.query)(span)
    }

    /// Only the events that start within `span`.
    pub fn onsets(&self, span: Span) -> Vec<Hap<T>> {
        self.query(span).into_iter().filter(Hap::has_onset).collect()
    }

    pub fn silence() -> Self {
        Pattern::new(|_| Vec::new())
    }

    /// One event per cycle.
    pub fn pure(value: T) -> Self {
        Pattern::new(move |span| {
            span.cycles()
                .into_iter()
                .map(|part| {
                    let cycle = part.start.floor();
                    Hap {
                        whole: Some(Span::new(Rational::int(cycle), Rational::int(cycle + 1))),
                        part,
                        value: value.clone(),
                    }
                })
                .collect()
        })
    }

    pub fn map<U: Clone + Send + Sync + 'static>(
        &self,
        f: impl Fn(T) -> U + Send + Sync + 'static,
    ) -> Pattern<U> {
        let p = self.clone();
        Pattern::new(move |span| p.query(span).into_iter().map(|h| h.map(&f)).collect())
    }

    pub fn filter(&self, keep: impl Fn(&T) -> bool + Send + Sync + 'static) -> Pattern<T> {
        let p = self.clone();
        Pattern::new(move |span| p.query(span).into_iter().filter(|h| keep(&h.value)).collect())
    }

    pub fn filter_map<U: Clone + Send + Sync + 'static>(
        &self,
        f: impl Fn(T) -> Option<U> + Send + Sync + 'static,
    ) -> Pattern<U> {
        let p = self.clone();
        Pattern::new(move |span| {
            p.query(span)
                .into_iter()
                .filter_map(|h| {
                    let (whole, part) = (h.whole, h.part);
                    f(h.value).map(|value| Hap { whole, part, value })
                })
                .collect()
        })
    }

    // ---- combining -------------------------------------------------------

    /// All of these at once.
    pub fn stack(patterns: Vec<Pattern<T>>) -> Pattern<T> {
        Pattern::new(move |span| patterns.iter().flat_map(|p| p.query(span)).collect())
    }

    /// One pattern per cycle, in turn: `<a b c>`.
    pub fn slowcat(patterns: Vec<Pattern<T>>) -> Pattern<T> {
        if patterns.is_empty() {
            return Pattern::silence();
        }
        let n = patterns.len() as i64;
        Pattern::new(move |span| {
            span.cycles()
                .into_iter()
                .flat_map(|part| {
                    let cycle = part.start.floor();
                    let which = cycle.rem_euclid(n);
                    // Each pattern advances one of its own cycles every n of ours.
                    let offset = Rational::int(cycle - (cycle - which) / n);
                    patterns[which as usize]
                        .query(part.map(|t| t - offset))
                        .into_iter()
                        .map(|h| h.map_times(|t| t + offset))
                        .collect::<Vec<_>>()
                })
                .collect()
        })
    }

    /// Squeeze all of these into one cycle, one after another: `a b c`.
    pub fn fastcat(patterns: Vec<Pattern<T>>) -> Pattern<T> {
        let n = patterns.len() as i64;
        Pattern::slowcat(patterns).fast(Rational::int(n))
    }

    /// Like [`Pattern::fastcat`] but each step takes a share of the cycle in
    /// proportion to its weight.
    pub fn timecat(steps: Vec<(Rational, Pattern<T>)>) -> Pattern<T> {
        let total = steps.iter().fold(Rational::ZERO, |t, (w, _)| t + *w);
        if total.is_zero() {
            return Pattern::silence();
        }
        let mut at = Rational::ZERO;
        let mut slots = Vec::with_capacity(steps.len());
        for (weight, pattern) in steps {
            slots.push(pattern.compress(at / total, (at + weight) / total));
            at = at + weight;
        }
        Pattern::stack(slots)
    }

    /// Play each cycle of this pattern within the slice `a..b` of the cycle,
    /// and nothing outside it.
    fn compress(&self, a: Rational, b: Rational) -> Pattern<T> {
        let p = self.clone();
        let len = b - a;
        Pattern::new(move |span| {
            let mut out = Vec::new();
            for part in span.cycles() {
                let cycle = Rational::int(part.start.floor());
                let slot = Span::new(cycle + a, cycle + b);
                let asked = if part.is_instant() {
                    // slots are half-open, so a boundary instant belongs to the later one
                    if slot.start <= part.start && part.start < slot.end { part } else { continue }
                } else {
                    match part.intersection(slot) {
                        Some(s) => s,
                        None => continue,
                    }
                };
                let inward = |t: Rational| cycle + (t - cycle - a) / len;
                let outward = |t: Rational| cycle + a + (t - cycle) * len;
                for hap in p.query(asked.map(inward)) {
                    out.push(hap.map_times(outward));
                }
            }
            out
        })
    }

    // ---- time ------------------------------------------------------------

    fn with_query_time(&self, f: impl Fn(Rational) -> Rational + Send + Sync + 'static) -> Pattern<T> {
        let p = self.clone();
        Pattern::new(move |span| p.query(span.map(&f)))
    }

    fn with_hap_time(&self, f: impl Fn(Rational) -> Rational + Send + Sync + 'static) -> Pattern<T> {
        let p = self.clone();
        Pattern::new(move |span| p.query(span).into_iter().map(|h| h.map_times(&f)).collect())
    }

    /// Play `factor` times as fast. A factor of zero or less is silence.
    pub fn fast(&self, factor: Rational) -> Pattern<T> {
        if factor <= Rational::ZERO {
            return Pattern::silence();
        }
        self.with_query_time(move |t| t * factor).with_hap_time(move |t| t / factor)
    }

    pub fn slow(&self, factor: Rational) -> Pattern<T> {
        if factor <= Rational::ZERO {
            return Pattern::silence();
        }
        self.fast(Rational::ONE / factor)
    }

    /// Move later by `by` cycles (earlier if negative).
    pub fn shift(&self, by: Rational) -> Pattern<T> {
        self.with_query_time(move |t| t - by).with_hap_time(move |t| t + by)
    }

    /// Play each cycle backwards.
    pub fn reverse(&self) -> Pattern<T> {
        let p = self.clone();
        Pattern::new(move |span| {
            let mut out = Vec::new();
            for part in span.cycles() {
                let cycle = Rational::int(part.start.floor());
                let sum = cycle + cycle + Rational::ONE;
                let reflect = move |s: Span| Span::new(sum - s.end, sum - s.start);
                for h in p.query(reflect(part)) {
                    out.push(Hap { whole: h.whole.map(reflect), part: reflect(h.part), value: h.value });
                }
            }
            out
        })
    }

    /// Apply `f` on the cycles where `when` is true.
    pub fn when_cycle(
        &self,
        when: impl Fn(i64) -> bool + Send + Sync + 'static,
        f: impl Fn(Pattern<T>) -> Pattern<T>,
    ) -> Pattern<T> {
        let plain = self.clone();
        let changed = f(self.clone());
        Pattern::new(move |span| {
            span.cycles()
                .into_iter()
                .flat_map(|part| {
                    if when(part.start.floor()) { changed.query(part) } else { plain.query(part) }
                })
                .collect()
        })
    }

    /// Apply `f` every `n`th cycle, starting with the first.
    pub fn every(&self, n: i64, f: impl Fn(Pattern<T>) -> Pattern<T>) -> Pattern<T> {
        if n <= 0 {
            return self.clone();
        }
        self.when_cycle(move |c| c.rem_euclid(n) == 0, f)
    }

    /// Randomly drop events: each is kept with probability `1 - drop`. The
    /// choice depends only on `seed` and when the event starts, so the same
    /// pattern always thins the same way.
    pub fn thin(&self, drop: f64, seed: u64) -> Pattern<T> {
        let p = self.clone();
        Pattern::new(move |span| {
            p.query(span)
                .into_iter()
                .filter(|h| h.whole.is_none_or(|w| unit_random(seed, w.start) >= drop))
                .collect()
        })
    }

    /// This pattern, plus a copy transformed by `f` and moved later by `after`.
    pub fn layer(&self, after: Rational, f: impl Fn(Pattern<T>) -> Pattern<T>) -> Pattern<T> {
        Pattern::stack(vec![self.clone(), f(self.clone()).shift(after)])
    }

    /// Repeat every event `n` times within its own length.
    pub fn repeat_each(&self, n: i64) -> Pattern<T> {
        if n < 1 {
            return Pattern::silence();
        }
        let p = self.clone();
        Pattern::new(move |span| {
            let mut out = Vec::new();
            for h in p.query(span) {
                let Some(whole) = h.whole else {
                    out.push(h);
                    continue;
                };
                let step = whole.len() / Rational::int(n);
                for i in 0..n {
                    let start = whole.start + step * Rational::int(i);
                    let piece = Span::new(start, start + step);
                    if let Some(part) = h.part.intersection(piece) {
                        out.push(Hap { whole: Some(piece), part, value: h.value.clone() });
                    }
                }
            }
            out
        })
    }

    /// Give these values the rhythm of `structure`: wherever it has a `true`
    /// event, play whatever this pattern has at that moment.
    pub fn rhythm(&self, structure: &Pattern<bool>) -> Pattern<T> {
        let (values, structure) = (self.clone(), structure.clone());
        Pattern::new(move |span| {
            let mut out = Vec::new();
            for s in structure.query(span).into_iter().filter(|h| h.value) {
                let Some(whole) = s.whole else { continue };
                for v in values.query(whole) {
                    if let Some(part) = s.part.intersection(v.part) {
                        out.push(Hap { whole: s.whole, part, value: v.value });
                    }
                }
            }
            out
        })
    }

    /// Keep only the parts of events where `mask` has a `true` event.
    pub fn mask(&self, mask: &Pattern<bool>) -> Pattern<T> {
        let (p, mask) = (self.clone(), mask.clone());
        Pattern::new(move |span| {
            let mut out = Vec::new();
            for h in p.query(span) {
                for m in mask.query(h.part).into_iter().filter(|m| m.value) {
                    if let Some(part) = h.part.intersection(m.part) {
                        out.push(Hap { whole: h.whole, part, value: h.value.clone() });
                    }
                }
            }
            out
        })
    }
}

impl Pattern<bool> {
    /// `hits` events spread as evenly as possible over `steps`, rotated by
    /// `rotation` steps: `euclid(3, 8)` is `x..x..x.`.
    pub fn euclid(hits: usize, steps: usize, rotation: usize) -> Pattern<bool> {
        let mut rhythm = euclid_steps(hits, steps);
        if !rhythm.is_empty() {
            let r = rotation % rhythm.len();
            rhythm.rotate_left(r);
        }
        let slots = rhythm
            .into_iter()
            .map(|hit| if hit { Pattern::pure(true) } else { Pattern::silence() })
            .collect();
        Pattern::fastcat(slots)
    }
}

/// The Euclidean rhythm of `hits` over `steps` (Bjorklund's algorithm).
pub fn euclid_steps(hits: usize, steps: usize) -> Vec<bool> {
    if steps == 0 {
        return Vec::new();
    }
    if hits >= steps {
        return vec![true; steps];
    }
    if hits == 0 {
        return vec![false; steps];
    }
    let mut a: Vec<Vec<bool>> = vec![vec![true]; hits];
    let mut b: Vec<Vec<bool>> = vec![vec![false]; steps - hits];
    while b.len() > 1 {
        let paired = a.len().min(b.len());
        let joined: Vec<Vec<bool>> = (0..paired)
            .map(|i| a[i].iter().chain(&b[i]).copied().collect())
            .collect();
        let rest: Vec<Vec<bool>> = if a.len() > b.len() { a[paired..].to_vec() } else { b[paired..].to_vec() };
        a = joined;
        b = rest;
    }
    a.into_iter().chain(b).flatten().collect()
}

/// A number in `0..1` that depends only on `seed` and `at`.
fn unit_random(seed: u64, at: Rational) -> f64 {
    let mut x = seed
        ^ (at.numer() as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (at.denom() as u64).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    // splitmix64
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^= x >> 31;
    (x >> 11) as f64 / (1u64 << 53) as f64
}
