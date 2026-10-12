use std::cell::Cell;
use std::sync::Arc;

use crate::Rational;

/// The most a step can be repeated or divided into.
pub const MAX_REPEATS: i64 = 1024;

/// The most cycles one query looks through.
const MAX_CYCLES_PER_QUERY: usize = 4096;

thread_local! {
    /// How many more queries (and events) this thread may spend, if limited.
    static BUDGET: Cell<Option<usize>> = const { Cell::new(None) };
}

/// Run `f` allowing patterns at most `work` queries and events between them.
/// Past that every query returns nothing, so a pattern that asks for an
/// absurd number of events costs a bounded amount of time. Also says whether
/// the budget ran out.
pub fn with_budget<R>(work: usize, f: impl FnOnce() -> R) -> (R, bool) {
    let before = BUDGET.replace(Some(work));
    let result = f();
    let exhausted = BUDGET.get() == Some(0);
    BUDGET.set(before);
    (result, exhausted)
}

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
        // A query this long (a pattern sped up a billion times, say) is cut
        // short instead of running for ever.
        while start < self.end && out.len() < MAX_CYCLES_PER_QUERY {
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
        let left = BUDGET.get();
        if left == Some(0) {
            return Vec::new();
        }
        let out = (self.query)(span);
        if let Some(left) = left {
            BUDGET.set(Some(left.saturating_sub(out.len().max(1))));
        }
        out
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
        self.switch_cycles(f(self.clone()), when)
    }

    /// `changed` on the cycles where `when` is true, this pattern on the rest.
    fn switch_cycles(&self, changed: Pattern<T>, when: impl Fn(i64) -> bool + Send + Sync + 'static) -> Pattern<T> {
        let plain = self.clone();
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

    /// Like [`Pattern::every`], with an `n` that can change from cycle to cycle.
    pub fn every_by(&self, n: &Param<Rational>, f: impl Fn(Pattern<T>) -> Pattern<T>) -> Pattern<T> {
        let changed = f(self.clone());
        n.apply(self, move |p, n| match n.floor() {
            n if n <= 0 => p.clone(),
            n => p.switch_cycles(changed.clone(), move |c| c.rem_euclid(n) == 0),
        })
    }

    /// Randomly drop events: each is kept with probability `1 - drop`. The
    /// choice depends only on `seed` and when the event starts, so the same
    /// pattern always thins the same way.
    pub fn thin(&self, drop: f64, seed: u64) -> Pattern<T> {
        self.pick(seed, move |r| r >= drop)
    }

    /// The events whose random number (from `seed` and when they start) passes `keep`.
    fn pick(&self, seed: u64, keep: impl Fn(f64) -> bool + Send + Sync + 'static) -> Pattern<T> {
        self.filter_haps(move |h| h.whole.is_none_or(|w| keep(unit_random(seed, w.start))))
    }

    fn filter_haps(&self, keep: impl Fn(&Hap<T>) -> bool + Send + Sync + 'static) -> Pattern<T> {
        let p = self.clone();
        Pattern::new(move |span| p.query(span).into_iter().filter(|h| keep(h)).collect())
    }

    /// Change a random `chance` of the events with `f`, leaving the rest
    /// alone. The same seed picks the same events.
    pub fn sometimes(&self, chance: f64, seed: u64, f: impl Fn(Pattern<T>) -> Pattern<T>) -> Pattern<T> {
        let chosen = self.pick(seed, move |r| r < chance);
        let rest = self.pick(seed, move |r| r >= chance);
        Pattern::stack(vec![rest, f(chosen)])
    }

    /// One of these each cycle, chosen at random: `[a | b | c]`.
    pub fn randcat(patterns: Vec<Pattern<T>>, seed: u64) -> Pattern<T> {
        if patterns.is_empty() {
            return Pattern::silence();
        }
        Pattern::new(move |span| {
            span.cycles()
                .into_iter()
                .flat_map(|part| {
                    let which = (random(seed, part.start.floor(), 0) * patterns.len() as f64) as usize;
                    patterns[which.min(patterns.len() - 1)].query(part)
                })
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
        let n = n.min(MAX_REPEATS);
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
        let steps = steps.min(MAX_REPEATS as usize);
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

/// The `i`th random number for a cycle, in `0..1`.
fn random(seed: u64, cycle: i64, i: u64) -> f64 {
    unit_random(seed ^ i.wrapping_mul(0xD6E8_FEB8_6659_FD93), Rational::int(cycle))
}

/// A number in `0..1` that depends only on `seed` and `at`.
pub fn unit_random(seed: u64, at: Rational) -> f64 {
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

// ---- numbers that are patterns ---------------------------------------------

/// Play `f(param)` wherever `param` has a value: the structure comes from the
/// result, the value of `param` from whichever of its events covers it.
pub fn inner_join<P, T>(param: &Pattern<P>, f: impl Fn(P) -> Pattern<T> + Send + Sync + 'static) -> Pattern<T>
where
    P: Clone + Send + Sync + 'static,
    T: Clone + Send + Sync + 'static,
{
    let param = param.clone();
    Pattern::new(move |span| param.query(span).into_iter().flat_map(|h| f(h.value).query(h.part)).collect())
}

/// An argument that is a fixed value or a pattern of values, as in `fast(2)`
/// and `fast(<2 4>)`.
#[derive(Clone)]
pub enum Param<P> {
    Const(P),
    Pat(Pattern<P>),
}

impl<P: Clone + Send + Sync + 'static> Param<P> {
    pub fn pattern(&self) -> Pattern<P> {
        match self {
            Param::Const(v) => Pattern::pure(v.clone()),
            Param::Pat(p) => p.clone(),
        }
    }

    /// `f(p, value)` for the value in force at each moment.
    pub fn apply<T: Clone + Send + Sync + 'static>(
        &self,
        p: &Pattern<T>,
        f: impl Fn(&Pattern<T>, P) -> Pattern<T> + Send + Sync + 'static,
    ) -> Pattern<T> {
        match self {
            Param::Const(v) => f(p, v.clone()),
            Param::Pat(param) => {
                let p = p.clone();
                inner_join(param, move |v| f(&p, v))
            }
        }
    }
}

// ---- more time and structure ------------------------------------------------

impl<T: Clone + Send + Sync + 'static> Pattern<T> {
    /// Change only the events that start in `from..to` of each cycle.
    pub fn within(&self, from: Rational, to: Rational, f: impl Fn(Pattern<T>) -> Pattern<T>) -> Pattern<T> {
        let changed = f(self.clone());
        let inside = move |h: &Hap<T>| h.whole.is_some_and(|w| from <= w.start.fract() && w.start.fract() < to);
        Pattern::stack(vec![changed.filter_haps(inside), self.filter_haps(move |h| !inside(h))])
    }

    /// Delay every other slot of `1/slots` of a cycle by `amount` of a slot.
    pub fn swing(&self, amount: Rational, slots: Rational) -> Pattern<T> {
        if slots <= Rational::ZERO {
            return self.clone();
        }
        let slotted = self.slow(slots);
        let late = |h: &Hap<T>| h.whole.is_some_and(|w| w.start.fract() >= Rational::new(1, 2));
        Pattern::stack(vec![slotted.filter_haps(move |h| !late(h)), slotted.filter_haps(late).shift(amount)]).fast(slots)
    }

    /// Start `1/n` of a cycle later each cycle, wrapping after `n` cycles.
    pub fn rotate(&self, n: i64) -> Pattern<T> {
        if n <= 1 {
            return self.clone();
        }
        let n = n.min(MAX_REPEATS);
        let p = self.clone();
        Pattern::new(move |span| {
            span.cycles()
                .into_iter()
                .flat_map(|part| p.shift(Rational::new(-part.start.floor().rem_euclid(n), n)).query(part))
                .collect::<Vec<_>>()
        })
    }

    /// Change a different `1/n` of the cycle each cycle, in turn.
    pub fn in_turn(&self, n: i64, f: impl Fn(Pattern<T>) -> Pattern<T>) -> Pattern<T> {
        if n < 1 {
            return self.clone();
        }
        let n = n.min(MAX_REPEATS);
        let variants: Vec<Pattern<T>> =
            (0..n).map(|k| self.within(Rational::new(k, n), Rational::new(k + 1, n), &f)).collect();
        Pattern::new(move |span| {
            span.cycles()
                .into_iter()
                .flat_map(|part| variants[part.start.floor().rem_euclid(n) as usize].query(part))
                .collect()
        })
    }

    /// Cut each cycle into `n` parts and play them in a different order each cycle, none twice.
    pub fn shuffle(&self, n: i64, seed: u64) -> Pattern<T> {
        self.rearrange(n, move |cycle, n| {
            let mut order: Vec<i64> = (0..n).collect();
            for i in (1..n as usize).rev() {
                let j = (random(seed, cycle, i as u64) * (i + 1) as f64) as usize;
                order.swap(i, j.min(i));
            }
            order
        })
    }

    /// Cut each cycle into `n` parts and fill each slot with a randomly chosen part, repeats allowed.
    pub fn pick_random(&self, n: i64, seed: u64) -> Pattern<T> {
        self.rearrange(n, move |cycle, n| {
            (0..n).map(|i| ((random(seed, cycle, i as u64) * n as f64) as i64).min(n - 1)).collect()
        })
    }

    /// Slot `i` of each cycle plays part `order(cycle)[i]` of that cycle.
    fn rearrange(&self, n: i64, order: impl Fn(i64, i64) -> Vec<i64> + Send + Sync + 'static) -> Pattern<T> {
        if n < 2 {
            return self.clone();
        }
        let n = n.min(MAX_REPEATS);
        let p = self.clone();
        Pattern::new(move |span| {
            let mut out = Vec::new();
            for part in span.cycles() {
                let cycle = part.start.floor();
                let order = order(cycle, n);
                let from = ((part.start - Rational::int(cycle)) * Rational::int(n)).floor().clamp(0, n - 1);
                let to = ((part.end - Rational::int(cycle)) * Rational::int(n)).ceil().clamp(from + 1, n);
                for i in from..to {
                    let slot = Span::new(Rational::int(cycle) + Rational::new(i, n), Rational::int(cycle) + Rational::new(i + 1, n));
                    let Some(inside) = part.intersection(slot) else { continue };
                    let by = Rational::new(order[i as usize] - i, n);
                    for hap in p.query(inside.map(|t| t + by)) {
                        out.push(hap.map_times(|t| t - by));
                    }
                }
            }
            out
        })
    }

    /// Loop the first `fraction` of each cycle to fill it.
    pub fn repeat_start(&self, fraction: Rational) -> Pattern<T> {
        if fraction <= Rational::ZERO || fraction >= Rational::ONE {
            return self.clone();
        }
        let fraction = fraction.max(Rational::new(1, MAX_REPEATS));
        let p = self.clone();
        Pattern::new(move |span| {
            let mut out = Vec::new();
            for part in span.cycles() {
                let cycle = Rational::int(part.start.floor());
                let from = ((part.start - cycle) / fraction).floor();
                let to = ((part.end - cycle) / fraction).ceil().max(from + 1);
                for j in from..to {
                    let by = fraction * Rational::int(j);
                    let repeat = Span::new(cycle + by, (cycle + by + fraction).min(cycle + Rational::ONE));
                    let Some(inside) = part.intersection(repeat) else { continue };
                    let source = Span::new(cycle, cycle + fraction);
                    for hap in p.query(inside.map(|t| t - by)) {
                        // an event is cut off where its repeat ends
                        let whole = hap.whole.map(|w| Span::new(w.start, w.end.min(source.end)));
                        out.push(Hap { whole, part: hap.part, value: hap.value }.map_times(|t| t + by));
                    }
                }
            }
            out
        })
    }

    /// Forwards, then backwards, a cycle each.
    pub fn palindrome(&self) -> Pattern<T> {
        Pattern::slowcat(vec![self.clone(), self.reverse()])
    }

    /// An event where this has none, in the gaps between its events (cut at
    /// the finest step they use): swaps hits and rests.
    pub fn invert(&self, hit: T) -> Pattern<T> {
        let p = self.clone();
        Pattern::new(move |span| {
            let mut out = Vec::new();
            for part in span.cycles() {
                let cycle = part.start.floor();
                let whole = Span::new(Rational::int(cycle), Rational::int(cycle + 1));
                let events: Vec<Span> = p.onsets(whole).into_iter().filter_map(|h| h.whole).collect();
                // the step: the largest fraction of a cycle that every event starts and ends on
                let denom = events.iter().fold(1i64, |l, e| lcm(lcm(l, e.start.denom()), e.end.denom()));
                let whole_steps = events.iter().fold(denom, |g, e| {
                    let at = |t: Rational| (t - Rational::int(cycle)).numer() * (denom / t.denom());
                    gcd(gcd(g, at(e.start)), at(e.end))
                });
                let steps = (denom / whole_steps.max(1)).clamp(1, MAX_REPEATS);
                for i in 0..steps {
                    let slot = Span::new(
                        Rational::int(cycle) + Rational::new(i, steps),
                        Rational::int(cycle) + Rational::new(i + 1, steps),
                    );
                    if events.iter().any(|e| e.start < slot.end && slot.start < e.end) {
                        continue;
                    }
                    if let Some(inside) = part.intersection(slot) {
                        out.push(Hap { whole: Some(slot), part: inside, value: hit.clone() });
                    }
                }
            }
            out
        })
    }

    /// Each value becomes several at once, all over the same stretch of time: chords.
    pub fn expand(&self, f: impl Fn(T) -> Vec<T> + Send + Sync + 'static) -> Pattern<T> {
        let p = self.clone();
        Pattern::new(move |span| {
            p.query(span)
                .into_iter()
                .flat_map(|h| f(h.value).into_iter().map(move |value| Hap { whole: h.whole, part: h.part, value }).collect::<Vec<_>>())
                .collect()
        })
    }

    /// Events that start and end together are played one after another across
    /// that stretch instead, in the order `arrange` gives them: arpeggios.
    pub fn spread(&self, arrange: impl Fn(Vec<T>) -> Vec<T> + Send + Sync + 'static) -> Pattern<T> {
        let p = self.clone();
        Pattern::new(move |span| {
            let mut haps = p.query(span);
            let mut out = Vec::new();
            haps.retain(|h| {
                let keep = h.whole.is_some();
                if !keep {
                    out.push(h.clone());
                }
                keep
            });
            haps.sort_by_key(|h| h.whole.map(|w| (w.start, w.end)));
            let mut i = 0;
            while i < haps.len() {
                let whole = haps[i].whole.expect("filtered above");
                let mut j = i;
                while j < haps.len() && haps[j].whole == Some(whole) {
                    j += 1;
                }
                let part = haps[i].part;
                let values = arrange(haps[i..j].iter().map(|h| h.value.clone()).collect());
                let count = values.len() as i64;
                for (k, value) in values.into_iter().enumerate() {
                    let step = whole.len() / Rational::int(count);
                    let slot = Span::new(whole.start + step * Rational::int(k as i64), whole.start + step * Rational::int(k as i64 + 1));
                    if let Some(inside) = part.intersection(slot) {
                        out.push(Hap { whole: Some(slot), part: inside, value });
                    }
                }
                i = j;
            }
            out
        })
    }

    /// Give each cycle's events new values, all at once: `rewrite` sees the
    /// events that start in cycle `c`, in time order, and gives one value per event.
    pub fn rewrite_cycles<U: Clone + Send + Sync + 'static>(
        &self,
        rewrite: impl Fn(&[Hap<T>], i64) -> Vec<U> + Send + Sync + 'static,
    ) -> Pattern<U> {
        let p = self.clone();
        Pattern::new(move |span| {
            let mut out = Vec::new();
            for part in span.cycles() {
                let cycle = part.start.floor();
                let whole = Span::new(Rational::int(cycle), Rational::int(cycle + 1));
                let mut events = p.onsets(whole);
                events.sort_by_key(|h| h.whole.map(|w| w.start));
                let values = rewrite(&events, cycle);
                for (h, value) in events.into_iter().zip(values) {
                    if let Some(inside) = h.part.intersection(part) {
                        out.push(Hap { whole: h.whole, part: inside, value });
                    }
                }
            }
            out
        })
    }

    /// A value read off a function of time: no events, just whatever it is at the moment asked.
    pub fn signal(f: impl Fn(Rational) -> T + Send + Sync + 'static) -> Pattern<T> {
        Pattern::new(move |span| {
            vec![Hap { whole: None, part: span, value: f((span.start + span.end) / Rational::int(2)) }]
        })
    }
}

fn gcd(a: i64, b: i64) -> i64 {
    if b == 0 { a.abs() } else { gcd(b, a % b) }
}

fn lcm(a: i64, b: i64) -> i64 {
    // past this many steps the exact answer no longer matters
    (a / gcd(a, b).max(1)).saturating_mul(b).min(1 << 20)
}

impl Pattern<f64> {
    /// A wave from 0 to 1 once a cycle.
    pub fn sine() -> Self {
        Pattern::signal(|t| ((t.to_f64() * std::f64::consts::TAU).sin() + 1.0) / 2.0)
    }

    /// Rises from 0 to 1 each cycle.
    pub fn saw() -> Self {
        Pattern::signal(|t| t.fract().to_f64())
    }

    /// Up from 0 to 1 and back each cycle.
    pub fn tri() -> Self {
        Pattern::signal(|t| {
            let x = t.fract().to_f64();
            if x < 0.5 { x * 2.0 } else { 2.0 - x * 2.0 }
        })
    }

    /// 0 for the first half of each cycle, 1 for the second.
    pub fn square() -> Self {
        Pattern::signal(|t| if t.fract() < Rational::new(1, 2) { 0.0 } else { 1.0 })
    }

    /// Noise in `0..1`, the same every time it is asked for the same moment.
    pub fn rand() -> Self {
        Pattern::signal(|t| unit_random(0x5EED, t))
    }

    /// Smooth noise in `0..1`: a random value each cycle, eased from one to the next.
    pub fn perlin() -> Self {
        Pattern::signal(|t| {
            let (cycle, x) = (t.floor(), t.fract().to_f64());
            let (a, b) = (random(0x9E41, cycle, 0), random(0x9E41, cycle + 1, 0));
            a + (b - a) * (6.0 * x.powi(5) - 15.0 * x.powi(4) + 10.0 * x.powi(3))
        })
    }
}

impl Pattern<bool> {
    /// Like [`Pattern::euclid`], with hits, steps and rotation that can change
    /// from cycle to cycle. Fewer hits than none (`-3`) swaps hits and rests.
    pub fn euclid_with(hits: &Param<Rational>, steps: &Param<Rational>, rotation: &Param<Rational>) -> Pattern<bool> {
        fn once(hits: Rational, steps: Rational, rotation: Rational) -> Pattern<bool> {
            let steps = steps.floor().clamp(0, MAX_REPEATS) as usize;
            let rotation = rotation.floor().rem_euclid(steps.max(1) as i64) as usize;
            let hits = hits.floor();
            let mut rhythm = euclid_steps(hits.unsigned_abs() as usize, steps);
            if hits < 0 {
                rhythm.iter_mut().for_each(|hit| *hit = !*hit);
            }
            if !rhythm.is_empty() {
                rhythm.rotate_left(rotation);
            }
            Pattern::fastcat(rhythm.into_iter().map(|hit| if hit { Pattern::pure(true) } else { Pattern::silence() }).collect())
        }
        if let (Param::Const(h), Param::Const(s), Param::Const(r)) = (hits, steps, rotation) {
            return once(*h, *s, *r);
        }
        let (steps, rotation) = (steps.pattern(), rotation.pattern());
        inner_join(&hits.pattern(), move |h| {
            let rotation = rotation.clone();
            inner_join(&steps, move |s| inner_join(&rotation, move |r| once(h, s, r)))
        })
    }
}
