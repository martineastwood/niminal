use niminal_pattern::{Hap, Hit, Pattern, Rational, Span, euclid_steps, grid, parse, parse_with};

fn r(n: i64, d: i64) -> Rational {
    Rational::new(n, d)
}

fn cycles(from: i64, to: i64) -> Span {
    Span::new(Rational::int(from), Rational::int(to))
}

/// (start, end, value) of every event that starts in `span`, in time order.
fn onsets<T: Clone + Send + Sync + 'static>(p: &Pattern<T>, span: Span) -> Vec<(Rational, Rational, T)> {
    let mut v: Vec<_> = p
        .onsets(span)
        .into_iter()
        .map(|h: Hap<T>| {
            let w = h.whole.unwrap();
            (w.start, w.end, h.value)
        })
        .collect();
    v.sort_by_key(|e| e.0);
    v
}

fn words(src: &str, span: Span) -> Vec<(Rational, Rational, String)> {
    onsets(&parse(src).unwrap(), span)
}

fn values(src: &str, span: Span) -> Vec<String> {
    words(src, span).into_iter().map(|e| e.2).collect()
}

#[test]
fn pure_repeats_every_cycle() {
    let p = Pattern::pure("a");
    let e = onsets(&p, cycles(0, 3));
    assert_eq!(e.len(), 3);
    assert_eq!((e[1].0, e[1].1), (Rational::int(1), Rational::int(2)));
}

#[test]
fn a_sequence_divides_the_cycle_evenly() {
    let e = words("a b c d", cycles(0, 1));
    assert_eq!(e.len(), 4);
    assert_eq!((e[1].0, e[1].1, e[1].2.as_str()), (r(1, 4), r(1, 2), "b"));
    // thirds are exact
    let e = words("a b c", cycles(0, 1));
    assert_eq!((e[1].0, e[1].1), (r(1, 3), r(2, 3)));
    assert_eq!(e[2].1, Rational::ONE);
}

#[test]
fn rests_take_a_step_and_play_nothing() {
    let e = words("a ~ c ~", cycles(0, 1));
    assert_eq!(e.iter().map(|e| e.2.as_str()).collect::<Vec<_>>(), ["a", "c"]);
    assert_eq!(e[1].0, r(1, 2));
}

#[test]
fn brackets_squeeze_a_sequence_into_one_step() {
    let e = words("a [b c] d", cycles(0, 1));
    assert_eq!(e.len(), 4);
    assert_eq!((e[1].0, e[1].1, e[1].2.as_str()), (r(1, 3), r(1, 2), "b"));
    assert_eq!((e[2].0, e[2].1, e[2].2.as_str()), (r(1, 2), r(2, 3), "c"));
    let nested = words("a [b [c d]]", cycles(0, 1));
    assert_eq!((nested[3].0, nested[3].1), (r(7, 8), Rational::ONE));
}

#[test]
fn a_comma_plays_sequences_together() {
    let e = words("[c4,e4,g4] a", cycles(0, 1));
    assert_eq!(e.len(), 4);
    let chord: Vec<_> = e.iter().filter(|x| x.0 == Rational::ZERO).map(|x| x.2.as_str()).collect();
    assert_eq!(chord.len(), 3);
    for note in ["c4", "e4", "g4"] {
        assert!(chord.contains(&note));
    }
    // top level commas work without brackets
    assert_eq!(words("a b, c", cycles(0, 1)).len(), 3);
}

#[test]
fn angle_brackets_alternate_one_per_cycle() {
    assert_eq!(values("<a b c>", cycles(0, 6)), ["a", "b", "c", "a", "b", "c"]);
    assert_eq!(values("x <a b>", cycles(0, 2)), ["x", "a", "x", "b"]);
    // an alternative can be a whole sequence
    let e = words("<a [b c]>", cycles(0, 2));
    assert_eq!(e.len(), 3);
    assert_eq!(e[2].0, r(3, 2));
    // and a rest
    assert_eq!(values("<a ~ b>", cycles(0, 3)), ["a", "b"]);
    // negative cycles work too
    assert_eq!(values("<a b>", cycles(-2, 0)), ["a", "b"]);
}

#[test]
fn star_repeats_within_the_step() {
    let e = words("a*2 b", cycles(0, 1));
    assert_eq!(e.iter().map(|e| (e.0, e.2.as_str())).collect::<Vec<_>>(), [(r(0, 1), "a"), (r(1, 4), "a"), (r(1, 2), "b")]);
    assert_eq!(words("[a b]*2", cycles(0, 1)).len(), 4);
    assert_eq!(words("a*3/2", cycles(0, 2)).len(), 3);
    assert_eq!(words("a*0.5", cycles(0, 2)).len(), 1);
}

#[test]
fn a_dot_holds_the_previous_step() {
    let e = words("a . b .", cycles(0, 1));
    assert_eq!(e.len(), 2);
    assert_eq!((e[0].0, e[0].1), (Rational::ZERO, r(1, 2)));
    assert_eq!((e[1].0, e[1].1), (r(1, 2), Rational::ONE));
    let e = words("a . . b", cycles(0, 1));
    assert_eq!(e[0].1, r(3, 4));
    assert_eq!(e[1].0, r(3, 4));
}

#[test]
fn atoms_can_carry_units_and_note_names() {
    assert_eq!(values("c#4 -6db 440hz 1/8beat 0.5", cycles(0, 1)), ["c#4", "-6db", "440hz", "1/8beat", "0.5"]);
    assert_eq!(values("(1/8 beat) x", cycles(0, 1)), ["1/8beat", "x"]);
}

#[test]
fn mini_notation_errors_point_at_the_problem() {
    let err = |s: &str| parse(s).err().unwrap();
    assert_eq!(err("[a b").message, "this `[` is never closed");
    assert_eq!(err("a b]").message, "unexpected `]`");
    assert_eq!(err("a b]").offset, 3);
    assert_eq!(err(". a").message, "a `.` holds the step before it, but there isn't one");
    assert_eq!(err("<a b").message, "this `<` is never closed");
    assert_eq!(err("<>").message, "`< >` needs at least one alternative");
    assert_eq!(err("a*x").message, "`*` needs a positive number, found ``");
    assert_eq!(err("a*0").message, "`*` needs a positive number, found `0`");
    assert_eq!(err("<a, b>").message, "commas aren't supported inside `< >`");
    assert_eq!(err("(a b").message, "this `(` is never closed");
    assert!(parse("").is_ok(), "an empty pattern is silence");
    assert!(onsets(&parse("").unwrap(), cycles(0, 4)).is_empty());
}

#[test]
fn querying_part_of_an_event_returns_the_same_whole() {
    let p = parse("a b").unwrap();
    let haps = p.query(Span::new(r(1, 4), r(3, 4)));
    assert_eq!(haps.len(), 2);
    let a = haps.iter().find(|h| h.value == "a").unwrap();
    assert_eq!(a.whole, Some(Span::new(Rational::ZERO, r(1, 2))));
    assert_eq!(a.part, Span::new(r(1, 4), r(1, 2)));
    assert!(!a.has_onset());
    assert!(haps.iter().find(|h| h.value == "b").unwrap().has_onset());
}

#[test]
fn an_instant_query_finds_what_is_sounding() {
    let p = parse("a b").unwrap();
    let at = |t: Rational| p.query(Span::new(t, t)).into_iter().map(|h| h.value).collect::<Vec<_>>();
    assert_eq!(at(Rational::ZERO), ["a"]);
    assert_eq!(at(r(1, 2)), ["b"]);
    assert_eq!(at(r(1, 4)), ["a"]);
    assert_eq!(at(Rational::ONE), ["a"], "the start of the next cycle");
}

#[test]
fn fast_and_slow() {
    let p = parse("a b").unwrap();
    assert_eq!(onsets(&p.fast(Rational::int(2)), cycles(0, 1)).len(), 4);
    let slow = onsets(&p.slow(Rational::int(2)), cycles(0, 2));
    assert_eq!(slow.len(), 2);
    assert_eq!((slow[1].0, slow[1].1), (Rational::ONE, Rational::int(2)));
    assert!(onsets(&p.fast(Rational::ZERO), cycles(0, 1)).is_empty());
    assert!(onsets(&p.slow(r(-1, 1)), cycles(0, 1)).is_empty());
    assert_eq!(onsets(&p.fast(r(3, 2)), cycles(0, 2)).len(), 6);
}

#[test]
fn shift_moves_events_later_or_earlier() {
    let p = parse("a b").unwrap();
    let later = onsets(&p.shift(r(1, 4)), cycles(0, 1));
    assert_eq!(later.iter().map(|e| e.0).collect::<Vec<_>>(), [r(1, 4), r(3, 4)]);
    let earlier = onsets(&p.shift(r(-1, 4)), cycles(0, 1));
    assert_eq!(earlier.iter().map(|e| (e.0, e.2.as_str())).collect::<Vec<_>>(), [(r(1, 4), "b"), (r(3, 4), "a")]);
}

#[test]
fn reverse_plays_each_cycle_backwards() {
    let p = parse("a b c d").unwrap().reverse();
    assert_eq!(values_of(&p, cycles(0, 1)), ["d", "c", "b", "a"]);
    let alt = parse("<a b> c").unwrap().reverse();
    assert_eq!(values_of(&alt, cycles(0, 2)), ["c", "a", "c", "b"]);
    // the times land where the reflected events belong
    let e = onsets(&parse("a b c").unwrap().reverse(), cycles(0, 1));
    assert_eq!(e[0].0, Rational::ZERO);
    assert_eq!(e[0].2, "c");
}

fn values_of<T: Clone + Send + Sync + 'static>(p: &Pattern<T>, span: Span) -> Vec<T> {
    onsets(p, span).into_iter().map(|e| e.2).collect()
}

#[test]
fn every_applies_a_change_on_every_nth_cycle() {
    let p = parse("a b").unwrap().every(3, |p| p.reverse());
    assert_eq!(values_of(&p, cycles(0, 6)), ["b", "a", "a", "b", "a", "b", "b", "a", "a", "b", "a", "b"]);
    let unchanged = parse("a b").unwrap().every(0, |p| p.reverse());
    assert_eq!(values_of(&unchanged, cycles(0, 1)), ["a", "b"]);
}

#[test]
fn thinning_is_random_looking_but_reproducible() {
    let p = parse("a*16").unwrap();
    let thin = |seed| onsets(&p.thin(0.5, seed), cycles(0, 50)).len();
    let kept = thin(1);
    assert!((300..500).contains(&kept), "about half of 800 survive: {kept}");
    assert_eq!(kept, thin(1), "the same seed thins the same events");
    assert_ne!(
        onsets(&p.thin(0.5, 1), cycles(0, 4)).iter().map(|e| e.0).collect::<Vec<_>>(),
        onsets(&p.thin(0.5, 2), cycles(0, 4)).iter().map(|e| e.0).collect::<Vec<_>>(),
        "different seeds choose differently"
    );
    assert_eq!(onsets(&p.thin(0.0, 1), cycles(0, 2)).len(), 32);
    assert!(onsets(&p.thin(1.0, 1), cycles(0, 2)).is_empty());
    // a query cutting through an event doesn't change whether it survives
    let t = p.thin(0.5, 7);
    let whole: Vec<_> = onsets(&t, cycles(0, 2)).iter().map(|e| e.0).collect();
    let halves: Vec<_> = onsets(&t, cycles(0, 1)).into_iter().chain(onsets(&t, cycles(1, 2))).map(|e| e.0).collect();
    assert_eq!(whole, halves);
}

#[test]
fn euclidean_rhythms_match_the_classics() {
    let show = |k, n| euclid_steps(k, n).into_iter().map(|b| if b { 'x' } else { '.' }).collect::<String>();
    assert_eq!(show(3, 8), "x..x..x.");
    assert_eq!(show(5, 8), "x.xx.xx.");
    assert_eq!(show(2, 5), "x.x..");
    assert_eq!(show(4, 4), "xxxx");
    assert_eq!(show(0, 4), "....");
    assert_eq!(show(9, 4), "xxxx");
    assert_eq!(show(1, 1), "x");
    assert!(euclid_steps(3, 0).is_empty());

    let p = Pattern::<bool>::euclid(3, 8, 0);
    let e = onsets(&p, cycles(0, 1));
    assert_eq!(e.iter().map(|e| e.0).collect::<Vec<_>>(), [r(0, 1), r(3, 8), r(3, 4)]);
    let rotated = Pattern::<bool>::euclid(3, 8, 1);
    assert_eq!(onsets(&rotated, cycles(0, 1))[0].0, r(1, 4), "rotating by one step moves the hit that began at step 3 to step 2");
}

#[test]
fn layer_adds_a_delayed_transformed_copy() {
    let p = parse("a b").unwrap().layer(r(1, 8), |p| p.fast(Rational::int(2)));
    let e = onsets(&p, cycles(0, 1));
    assert_eq!(e.len(), 6);
}

#[test]
fn repeat_each_subdivides_every_event() {
    let p = parse("a b").unwrap().repeat_each(2);
    let e = onsets(&p, cycles(0, 1));
    assert_eq!(e.iter().map(|e| (e.0, e.2.as_str())).collect::<Vec<_>>(), [(r(0, 1), "a"), (r(1, 4), "a"), (r(1, 2), "b"), (r(3, 4), "b")]);
    assert!(onsets(&p.repeat_each(0), cycles(0, 1)).is_empty());
}

#[test]
fn rhythm_gives_values_the_structure_of_a_mask() {
    let notes = parse("c e g").unwrap();
    let structure = Pattern::<bool>::euclid(3, 8, 0);
    let e = onsets(&notes.rhythm(&structure), cycles(0, 1));
    // three hits, at the euclid positions, sampling the notes beneath them
    assert_eq!(e.iter().map(|e| e.0).collect::<Vec<_>>(), [r(0, 1), r(3, 8), r(3, 4)]);
    assert_eq!(e.iter().map(|e| e.2.as_str()).collect::<Vec<_>>(), ["c", "e", "g"]);
    assert_eq!(e[0].1, r(1, 8), "the rhythm decides how long each lasts");
}

#[test]
fn mask_silences_where_the_mask_is_empty() {
    let p = parse("a b c d").unwrap();
    let mask = Pattern::fastcat(vec![Pattern::pure(true), Pattern::silence(), Pattern::pure(true), Pattern::silence()]);
    assert_eq!(values_of(&p.mask(&mask), cycles(0, 1)), ["a", "c"]);
}

#[test]
fn map_and_filter_work_on_values() {
    let p = parse("1 2 3 4").unwrap().map(|s| s.parse::<i32>().unwrap()).filter(|n| n % 2 == 0).map(|n| n * 10);
    assert_eq!(values_of(&p, cycles(0, 1)), [20, 40]);
    let q = parse("1 x 3").unwrap().filter_map(|s| s.parse::<i32>().ok());
    assert_eq!(values_of(&q, cycles(0, 1)), [1, 3]);
}

#[test]
fn long_range_queries_cost_only_what_they_ask_for() {
    // a million cycles in, with no unrolling
    let p = parse("<a b c> [d e]").unwrap();
    let e = onsets(&p, cycles(1_000_000, 1_000_001));
    assert_eq!(e.len(), 3);
    assert_eq!(e[0].2, "b", "1_000_000 mod 3 = 1");
}

#[test]
fn grids_read_hits_accents_and_ghosts() {
    let g = grid("x.o. XooX").unwrap();
    let e = onsets(&g, cycles(0, 1));
    assert_eq!(e.iter().map(|e| e.2).collect::<Vec<_>>(), [Hit::Normal, Hit::Ghost, Hit::Accent, Hit::Ghost, Hit::Ghost, Hit::Accent]);
    assert_eq!(e[2].0, r(1, 2), "eight steps, the fifth is half way");
    assert_eq!(e[0].1, r(1, 8));
    let err = grid("x.q").err().unwrap();
    assert_eq!(err.message, "`q` isn't a grid step: use x, X, o or .");
    assert_eq!(err.offset, 2);
    assert!(onsets(&grid("").unwrap(), cycles(0, 1)).is_empty());
}

#[test]
fn atoms_are_converted_as_they_are_read_and_bad_ones_are_located() {
    let numbers = parse_with("1 2 [3 4]*2", |a| a.parse::<i32>().map_err(|_| format!("`{a}` isn't a number"))).unwrap();
    assert_eq!(values_of(&numbers, cycles(0, 1)), [1, 2, 3, 4, 3, 4]);

    let err = parse_with("1 [2 x] 3", |a| a.parse::<i32>().map_err(|_| format!("`{a}` isn't a number"))).err().unwrap();
    assert_eq!(err.message, "`x` isn't a number");
    assert_eq!(err.offset, 5, "at the atom, not the end of the pattern");

    // atoms inside an alternation are checked even though they only play in later cycles
    let err = parse_with("<1 2 oops>", |a| a.parse::<i32>().map_err(|_| format!("`{a}` isn't a number"))).err().unwrap();
    assert_eq!(err.offset, 5);
    // and in parenthesised groups
    let err = parse_with("(1 q)", |a| a.parse::<i32>().map_err(|_| "bad".to_string())).err().unwrap();
    assert_eq!(err.offset, 0);
}
