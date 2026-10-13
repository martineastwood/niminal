use niminal_pattern::{Hap, Hit, Param, Pattern, Rational, Span, euclid_steps, grid, parse, parse_with};

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
    assert_eq!(values("1/8beat x", cycles(0, 1)), ["1/8beat", "x"]);
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
    assert_eq!(err("a*x").message, "`*` needs a number from above 0 to 1024, found ``");
    assert_eq!(err("a*0").message, "`*` needs a number from above 0 to 1024, found `0`");
    assert_eq!(err("<a | b>").message, "`|` isn't supported inside `< >`");
    assert_eq!(err("a@0").message, "`@` needs a number above 0, found `0`");
    assert_eq!(err("{a b").message, "this `{` is never closed");
    assert_eq!(err("a(3)").message, "a Euclidean rhythm is `(hits, steps)` or `(hits, steps, rotation)`");
    assert_eq!(err("[a, b | c]").message, "`,` and `|` can't be mixed here; put one inside brackets");
    assert_eq!(err("! a").message, "a `!` repeats the step before it, but there isn't one");
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

#[test]
fn slash_slows_a_step() {
    let e = words("a/2 b", cycles(0, 2));
    assert_eq!(e.iter().map(|e| e.2.as_str()).collect::<Vec<_>>(), ["a", "b", "b"], "a lasts the first half, plus a half beyond");
    assert_eq!(words("[a b]/2", cycles(0, 2)).len(), 2);
    // a fraction in a number is still one atom
    assert_eq!(values("1/8beat a", cycles(0, 1)), ["1/8beat", "a"]);
}

#[test]
fn bang_repeats_and_at_weights() {
    assert_eq!(values("a!3 b", cycles(0, 1)), ["a", "a", "a", "b"]);
    assert_eq!(values("a! b", cycles(0, 1)), ["a", "a", "b"]);
    assert_eq!(values("a b ! !", cycles(0, 1)), ["a", "b", "b", "b"]);
    let e = words("a@3 b", cycles(0, 1));
    assert_eq!((e[0].1, e[1].0), (r(3, 4), r(3, 4)));
    let e = words("a ~@2 b", cycles(0, 1));
    assert_eq!(e[1].0, r(3, 4));
    // inside alternation they stretch over cycles
    assert_eq!(values("<a@2 b>", cycles(0, 3)), ["a", "b"]);
    assert_eq!(values("<a!2 b>", cycles(0, 3)), ["a", "a", "b"]);
}

#[test]
fn euclid_in_the_notation() {
    let e = words("a(3,8)", cycles(0, 1));
    assert_eq!(e.iter().map(|e| e.0).collect::<Vec<_>>(), [r(0, 1), r(3, 8), r(3, 4)]);
    let e = words("a(3,8,2)", cycles(0, 1));
    assert_eq!(e[0].0, r(1, 8));
    assert_eq!(words("a(-3,8)", cycles(0, 1)).len(), 5, "a negative count swaps hits and rests");
    // the numbers can be patterns
    assert_eq!(words("a(<3 5>,8)", cycles(0, 2)).len(), 8);
    assert_eq!(words("[a b](2,4)", cycles(0, 1)).len(), 2);
}

#[test]
fn question_mark_drops_events_the_same_way_every_time() {
    let n = |src: &str| words(src, cycles(0, 40)).len();
    let kept = n("a*8?");
    assert!((100..220).contains(&kept), "{kept}");
    assert_eq!(kept, n("a*8?"));
    assert_eq!(n("a*8?0"), 320);
    assert_eq!(n("a*8?1"), 0);
    // two question marks drop different events
    let both: Vec<_> = words("a*8? a*8?", cycles(0, 1)).iter().map(|e| e.0 * Rational::int(2)).collect();
    assert!(both.len() < 16);
}

#[test]
fn a_bar_picks_one_each_cycle() {
    let v = values("[a | b | c]", cycles(0, 60));
    assert_eq!(v.len(), 60);
    for x in ["a", "b", "c"] {
        assert!(v.iter().any(|y| y == x), "{x} never chosen");
    }
    assert_eq!(v, values("[a | b | c]", cycles(0, 60)), "reproducible");
    assert!(words("a b | c", cycles(0, 1)).len() <= 2);
}

#[test]
fn polymeter_reads_lists_at_a_shared_step_rate() {
    // three steps a cycle taken from a five-step list wrap around over cycles
    assert_eq!(values("{a b c d e}%3", cycles(0, 2)), ["a", "b", "c", "d", "e", "a"][..6].iter().map(|s| s.to_string()).collect::<Vec<_>>());
    // two lists: the first sets the pace
    let e = words("{a b, x y z}", cycles(0, 1));
    assert_eq!(e.len(), 2 + 2, "the second list is squeezed to two steps a cycle");
    // without a %, the first list's length is the pace and it plays as written
    assert_eq!(values("{a b c}", cycles(0, 1)), ["a", "b", "c"]);
    assert_eq!(values("{a b c}%<2 4>", cycles(0, 2)).len(), 2 + 4);
}

#[test]
fn commas_inside_angle_brackets_run_alternations_together() {
    let e = words("<a b, c d e>", cycles(0, 3));
    assert_eq!(e.iter().map(|e| e.2.as_str()).collect::<Vec<_>>().len(), 6);
    let mut cs: Vec<_> = e.iter().filter(|x| x.2 == "e").collect();
    assert_eq!(cs.pop().map(|x| x.0), Some(Rational::int(2)));
}

#[test]
fn star_and_slash_take_patterns() {
    assert_eq!(words("a*<2 3>", cycles(0, 2)).len(), 5);
    assert_eq!(words("a*[2 4]", cycles(0, 1)).len(), 3);
}

#[test]
fn within_changes_part_of_the_cycle() {
    let p = parse("a b c d").unwrap().within(Rational::ZERO, r(1, 2), |p| p.fast(Rational::int(2)));
    let e = onsets(&p, cycles(0, 1));
    assert_eq!(e.len(), 6, "the first two events are doubled up, the rest untouched");
    assert!(e.iter().filter(|x| x.0 >= r(1, 2)).count() == 2);
}

#[test]
fn swing_delays_every_other_slot() {
    let p = parse("a*8").unwrap().swing(r(1, 3), Rational::int(4));
    let starts: Vec<_> = onsets(&p, cycles(0, 1)).iter().map(|e| e.0).collect();
    // the second event of each pair of eighths moves a third of a quarter later
    assert_eq!(starts[0], Rational::ZERO);
    assert_eq!(starts[1], r(1, 8) + r(1, 12));
}

#[test]
fn rotate_starts_one_part_later_each_cycle() {
    let p = parse("a b c d").unwrap().rotate(4);
    assert_eq!(values_of(&p, cycles(0, 2)), ["a", "b", "c", "d", "b", "c", "d", "a"]);
}

#[test]
fn in_turn_changes_each_part_in_turn() {
    let p = parse("a b c d").unwrap().in_turn(4, |p| p.fast(Rational::int(2)));
    assert_eq!(onsets(&p, cycles(0, 1)).len(), 5, "the first quarter holds a twice-as-fast cycle, so a and the start of b");
    assert_eq!(onsets(&p, cycles(1, 2)).len(), 5);
}

#[test]
fn shuffle_plays_every_part_once_and_scramble_may_repeat() {
    let p = parse("a b c d").unwrap();
    for c in 0..20 {
        let mut v = values_of(&p.shuffle(4, 1), cycles(c, c + 1));
        v.sort();
        assert_eq!(v, ["a", "b", "c", "d"]);
    }
    let orders: std::collections::HashSet<_> = (0..20).map(|c| values_of(&p.shuffle(4, 1), cycles(c, c + 1))).collect();
    assert!(orders.len() > 3);
    let repeats = (0..40).any(|c| {
        let mut v = values_of(&p.pick_random(4, 1), cycles(c, c + 1));
        v.sort();
        v.dedup();
        v.len() < 4
    });
    assert!(repeats);
    assert_eq!(values_of(&p.shuffle(4, 1), cycles(3, 4)), values_of(&p.shuffle(4, 1), cycles(3, 4)));
}

#[test]
fn repeat_start_loops_the_front_of_the_cycle() {
    let p = parse("a b c d").unwrap().repeat_start(r(1, 2));
    assert_eq!(values_of(&p, cycles(0, 1)), ["a", "b", "a", "b"]);
    let p = parse("a b c d").unwrap().repeat_start(r(1, 4));
    assert_eq!(values_of(&p, cycles(0, 1)), ["a", "a", "a", "a"]);
}

#[test]
fn invert_swaps_hits_and_rests() {
    let g = grid("x.x. x...").unwrap();
    let e = onsets(&g.invert(Hit::Normal), cycles(0, 1));
    assert_eq!(e.iter().map(|x| x.0).collect::<Vec<_>>(), [r(1, 8), r(3, 8), r(5, 8), r(6, 8), r(7, 8)]);
    assert_eq!(onsets(&Pattern::<Hit>::silence().invert(Hit::Normal), cycles(0, 1)).len(), 1);
}

#[test]
fn palindrome_goes_there_and_back() {
    let p = parse("a b c").unwrap().palindrome();
    assert_eq!(values_of(&p, cycles(0, 2)), ["a", "b", "c", "c", "b", "a"]);
}

#[test]
fn sometimes_changes_a_random_share_of_events() {
    let p = parse("a*8").unwrap().sometimes(0.5, 3, |p| p.fast(Rational::int(2)));
    let n = onsets(&p, cycles(0, 40)).len();
    assert!(n > 320 && n < 640, "{n}");
    assert_eq!(onsets(&parse("a*8").unwrap().sometimes(0.0, 3, |p| p.fast(Rational::int(2))), cycles(0, 1)).len(), 8);
}

#[test]
fn signals_have_no_events_but_a_value_at_every_moment() {
    let sine = Pattern::<f64>::sine();
    let at = |p: &Pattern<f64>, t: Rational| p.query(Span::new(t, t))[0].value;
    assert!((at(&sine, Rational::ZERO) - 0.5).abs() < 1e-9);
    assert!((at(&sine, r(1, 4)) - 1.0).abs() < 1e-9);
    assert!((at(&Pattern::<f64>::saw(), r(3, 4)) - 0.75).abs() < 1e-9);
    assert!((at(&Pattern::<f64>::tri(), r(1, 2)) - 1.0).abs() < 1e-9);
    assert_eq!(at(&Pattern::<f64>::square(), r(3, 4)), 1.0);
    let rand = Pattern::<f64>::rand();
    assert_eq!(at(&rand, r(1, 3)), at(&rand, r(1, 3)));
    assert_ne!(at(&rand, r(1, 3)), at(&rand, r(2, 3)));
    let perlin = Pattern::<f64>::perlin();
    assert!((at(&perlin, r(5, 7)) - at(&perlin, r(5, 7) + r(1, 1000))).abs() < 0.05, "smooth");
    // sampling a signal into steps gives events
    let steps = sine.rhythm(&Pattern::pure(true).fast(Rational::int(4)));
    assert_eq!(onsets(&steps, cycles(0, 1)).len(), 4);
}

#[test]
fn expand_and_spread_make_chords_and_arpeggios() {
    let chord = parse("c").unwrap().expand(|s| vec![s.clone(), format!("{s}3"), format!("{s}5")]);
    assert_eq!(onsets(&chord, cycles(0, 1)).len(), 3);
    let arp = chord.spread(|mut v| {
        v.sort();
        v
    });
    let e = onsets(&arp, cycles(0, 1));
    assert_eq!(e.iter().map(|x| (x.0, x.2.as_str())).collect::<Vec<_>>(), [(r(0, 1), "c"), (r(1, 3), "c3"), (r(2, 3), "c5")]);
}

#[test]
fn params_can_be_patterns_or_numbers() {
    let p = parse("a b").unwrap();
    let fixed = Param::Const(Rational::int(2)).apply(&p, |p, f| p.fast(f));
    assert_eq!(onsets(&fixed, cycles(0, 1)).len(), 4);
    let varying = Param::Pat(parse_with("<1 2>", |a| a.parse::<i64>().map(Rational::int).map_err(|e| e.to_string())).unwrap()).apply(&p, |p, f| p.fast(f));
    assert_eq!(onsets(&varying, cycles(0, 2)).len(), 2 + 4);
}

#[test]
fn rewrite_cycles_sees_each_cycles_events_in_order() {
    let p = parse("a b c").unwrap().rewrite_cycles(|events, cycle| events.iter().enumerate().map(|(i, e)| format!("{}{i}{cycle}", e.value)).collect());
    assert_eq!(values_of(&p, cycles(1, 2)), ["a01", "b11", "c21"]);
}

#[test]
fn euclid_with_pattern_arguments() {
    let hits = Param::Pat(parse_with("<3 5>", |a| a.parse::<i64>().map(Rational::int).map_err(|e| e.to_string())).unwrap());
    let p = Pattern::<bool>::euclid_with(&hits, &Param::Const(Rational::int(8)), &Param::Const(Rational::ZERO));
    assert_eq!(onsets(&p, cycles(0, 1)).len(), 3);
    assert_eq!(onsets(&p, cycles(1, 2)).len(), 5);
}

#[test]
fn pieces_cut_events_up_and_say_which_piece_each_is() {
    let p = parse("a b").unwrap().pieces(2, |v, i, n| format!("{v}{i}/{n}"));
    let e = onsets(&p, cycles(0, 1));
    assert_eq!(e.iter().map(|x| (x.0, x.2.as_str())).collect::<Vec<_>>(), [(r(0, 1), "a0/2"), (r(1, 4), "a1/2"), (r(1, 2), "b0/2"), (r(3, 4), "b1/2")]);
    assert!(onsets(&parse("a").unwrap().pieces(0, |v, _, _| v.clone()), cycles(0, 1)).is_empty());
}

#[test]
fn set_from_takes_a_value_where_each_event_starts() {
    let notes = parse("a b c d").unwrap();
    let levels = parse("1 2").unwrap();
    let p = notes.set_from(&levels, |v, l| format!("{v}{l}"));
    assert_eq!(values_of(&p, cycles(0, 1)), ["a1", "b1", "c2", "d2"]);
    // where the other pattern is silent, the event is left alone
    let sparse = parse("1 ~").unwrap();
    assert_eq!(values_of(&notes.set_from(&sparse, |v, l| format!("{v}{l}")), cycles(0, 1)), ["a1", "b1", "c", "d"]);
}
