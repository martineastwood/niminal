//! Numeric behavior at compiler/runtime boundaries, not only compile success.
use niminal_engine::{BLOCK, Buses, Voice};
use niminal_lang::{Program, compile};

fn program(source: &str) -> Program {
    compile(source).unwrap_or_else(|errors| {
        panic!(
            "{}",
            errors
                .iter()
                .map(|e| e.render("arrays", source))
                .collect::<String>()
        )
    })
}

fn render(source: &str, frames: usize, block: usize) -> Vec<Vec<f32>> {
    let p = program(source);
    let graph = p.instruments[0].graph.clone();
    let channels = graph.channels();
    let mut voice = Voice::new(graph, 48_000.0);
    let mut buses = Buses::new(&[]);
    let mut result = vec![Vec::new(); channels];
    for start in (0..frames).step_by(block) {
        let n = block.min(frames - start);
        let mut out = vec![[0.0; BLOCK]; channels];
        voice.process_blocks(&mut out, n, &mut buses);
        for (result, out) in result.iter_mut().zip(out) {
            result.extend_from_slice(&out[..n]);
        }
    }
    result
}

fn constant(source: &str, expected: f32) {
    for value in &render(source, 65, 13)[0] {
        assert!(
            (value - expected).abs() < 1e-5,
            "expected {expected}, got {value}"
        );
    }
}

#[test]
fn operations_do_not_depend_on_storage() {
    for operation in [
        "xs.sum",
        "xs.min",
        "xs.max",
        "xs.map(it * 2).sum",
        "(-xs).sum",
        "(xs * 2 + xs).sum",
        "xs[1..<3].sum",
        "xs.at(-0.5)",
    ] {
        let expected = match operation {
            "xs.sum" => 6.0,
            "xs.min" => 1.0,
            "xs.max" => 3.0,
            "xs.map(it * 2).sum" => 12.0,
            "(-xs).sum" => -6.0,
            "(xs * 2 + xs).sum" => 18.0,
            "xs[1..<3].sum" => 5.0,
            _ => 2.0,
        };
        constant(
            &format!("instr a() {{ xs = [1, 2, 3]\n{operation} }}"),
            expected,
        );
        constant(
            &format!(
                "const xs = [1, 2, 3]\nfn f() {{ state unused = 0\n{operation} }}\ninstr a() {{ f() }}"
            ),
            expected,
        );
        constant(
            &format!("fn f() {{ state xs = [1, 2, 3]\n{operation} }}\ninstr a() {{ f() }}"),
            expected,
        );
        constant(
            &format!(
                "fn f(xs: [num]) {{ state unused = 0\n{operation} }}\ninstr a(x: num = 1) {{ f([x, x * 2, x * 3]) }}"
            ),
            expected,
        );
    }
}

#[test]
fn array_arguments_and_results_exceed_audio_channel_limits() {
    constant(
        "fn f(xs: [num]) { state unused = 0\nxs * 2 }\ninstr a(x: num = 1) { xs = (0..<64).map(x + it)\nf(xs).sum }",
        4160.0,
    );
    constant(
        "fn f() { state xs = zeros(128)\nxs[127] = 7\nxs }\ninstr a() { f().sum }",
        7.0,
    );
}

#[test]
fn computed_arrays_are_directly_indexed_and_interpolated() {
    for (interp, expected) in [("none", 2.0), ("linear", 3.0), ("cubic", 2.8125)] {
        let source = format!(
            "instr a(x: num = 1, pos: num = 1.5) {{ [x, x * 2, x * 4, x * 8].at(pos, interp: {interp}) }}"
        );
        constant(&source, expected);
        let source = format!(
            "fn f(xs: [num], pos) {{ state unused = 0\nxs.at(pos, interp: {interp}) }}\ninstr a(x: num = 1, pos: num = 1.5) {{ f([x, x * 2, x * 4, x * 8], pos) }}"
        );
        constant(&source, expected);
    }
    constant(
        "fn f(xs: [num], index) { state unused = 0\nxs[index] }\ninstr a(x: num = 2, index: num = -1) { f((0..<64).map(x + it), index) }",
        65.0,
    );
}

#[test]
fn buffers_read_current_values_and_keep_state_across_blocks() {
    let source = "fn f() { state xs = [0, 0, 0]\nstate pos = 0\nxs[pos] = pos + 1\npos = (pos + 1).mod(3)\nxs.sum }\ninstr a() { f() }";
    let a = render(source, 70, 32);
    let b = render(source, 70, 7);
    assert_eq!(a, b);
    assert_eq!(&a[0][..5], &[1.0, 3.0, 6.0, 6.0, 6.0]);
    assert_eq!(
        a,
        render(source, 70, 32),
        "a new voice starts with its own buffer"
    );
}

#[test]
fn nested_arrays_preserve_shape_through_functions_and_branches() {
    constant(
        "fn f(xs: [[num]], choose) { state unused = 0\nif choose > 0 { xs * 2 } else { xs * 3 } }\ninstr a(x: num = 1) { f([[x, 2 * x], [3 * x, 4 * x]], x)[1].sum }",
        14.0,
    );
    constant(
        "instr a(choose: num = 1) { rows = if choose > 0 { [[1, 2], [3, 4]] } else { [[5, 6], [7, 8]] }\nrows[1].sum }",
        7.0,
    );
    constant(
        "fn f(index) { state unused = 0\n[[1, 2], [3, 4]][index].sum }\ninstr a(index: num = -1) { f(index) }",
        7.0,
    );
    constant(
        "instr a(index: num = -1) { [[1, 2], [3, 4]][index].sum }",
        7.0,
    );
    constant(
        "fn f() { state unused = 0\n[[1, 2], [3, 4]].sum.sum }\ninstr a() { f() }",
        10.0,
    );
    constant("instr a() { [[1, 2], [3, 4]].sum.sum }", 10.0);
}

#[test]
fn named_array_results_preserve_shape() {
    constant(
        "fn f(x) -> (bins, level) { state unused = 0\nbins = [x, x * 2, x * 3]\nlevel = bins.sum }\ninstr a(x: num = 2) { f(x).bins[1] }",
        4.0,
    );
    constant(
        "fn f(x) -> (bins, level) { bins = [x, x * 2, x * 3]\nlevel = bins.sum }\ninstr a(x: num = 2) { f(x).bins[1] }",
        4.0,
    );
}

#[test]
fn signal_channels_offer_array_operations() {
    let source = "config { channels: stereo }\ninstr a() { signal = [1, 2].to_layout(stereo)\n[signal[1], signal.map(it * 3).sum] }";
    let result = render(source, 8, 8);
    assert_eq!(result, vec![vec![2.0; 8], vec![9.0; 8]]);
    constant(
        "fn f(xs: [num]) { state unused = 0\nxs.sum }\ninstr a() { f(3) }",
        3.0,
    );
}

#[test]
fn array_builtin_expansion_does_not_become_audio_channels() {
    program("instr a(f: hz) { osc(saw, (0..<64).map(f * (1 + it * 0.001))).sum }");
    program(
        "fn f(freqs: [hz]) { state unused = 0\nosc(saw, freqs) }\ninstr a(freq: hz) { f((0..<64).map(freq * (1 + it * 0.001))).sum }",
    );
    constant("instr a(x: num = 2) { tanh((0..<64).map(x)).len }", 64.0);
}

#[test]
fn element_types_shapes_and_positions_are_checked() {
    for source in [
        "fn f(xs: [hz]) { xs.sum }\ninstr a() { f([1, 2]) }",
        "fn f(xs: [hz]) { state unused = 0\nxs.sum }\ninstr a() { f([1, 2]) }",
        "fn f() { state xs = [0, 0]\nxs[1hz] = 1\nxs.sum }\ninstr a() { f() }",
        "fn f() { state xs = [0, 0]\nxs.at(1hz) }\ninstr a() { f() }",
        "instr a(x: num) { if x > 0 { [1, 2] } else { [3, 4, 5] }.sum }",
        "instr a() { ([1, 2] + [3, 4, 5]).sum }",
    ] {
        assert!(compile(source).is_err(), "should reject: {source}");
    }
}

#[test]
fn math_units_comparisons_and_clamping_preserve_arrays() {
    for (operation, expected) in [
        ("xs.pow(2).sum", 14.0),
        ("xs.mod(2).sum", 2.0),
        ("xs.min(2).sum", 5.0),
        ("xs.max(2).sum", 7.0),
        ("xs.clamp(1.5..2.5).sum", 6.0),
        ("(xs > 1).sum", 2.0),
        ("xs.hz.num.sum", 6.0),
        ("xs.init.sum", 6.0),
    ] {
        constant(
            &format!("instr a() {{ xs = [1, 2, 3]\n{operation} }}"),
            expected,
        );
        constant(
            &format!("fn f() {{ state xs = [1, 2, 3]\n{operation} }}\ninstr a() {{ f() }}"),
            expected,
        );
    }
}

#[test]
fn buffers_initialize_from_note_start_array_values() {
    constant(
        "fn f(xs: [num]) { state buf = xs\nbuf.sum }\ninstr a(x: num = 2) { f([x, x * 2, x * 3]) }",
        12.0,
    );
}

#[test]
fn interpolated_constants_can_build_other_constants() {
    constant(
        "const xs = [1, 2, 4, 8]\nconst value = xs.at(1.5, interp: cubic)\ninstr a() { value }",
        2.8125,
    );
}

#[test]
fn named_array_results_work_inside_stateful_callers() {
    constant(
        "fn split(x) -> (bins, level) { state unused = 0\nbins = [x, x * 2]\nlevel = bins.sum }\nfn f(x) { split(x).bins.sum }\ninstr a(x: num = 2) { f(x) }",
        6.0,
    );
}

#[test]
fn empty_arrays_are_values_and_invalid_reads_are_diagnosed() {
    for expression in [
        "[].len",
        "zeros(0).len",
        "[].map(it * 2).len",
        "[1, 2][0..<0].len",
        "[][0..<0].len",
        "(0..<0).map(it * 2).len",
    ] {
        constant(&format!("instr a() {{ {expression} }}"), 0.0);
        constant(
            &format!("fn f() {{ state unused = 0\n{expression} }}\ninstr a() {{ f() }}"),
            0.0,
        );
    }
    constant("fn f() { state xs = []\nxs.len }\ninstr a() { f() }", 0.0);
    constant(
        "fn f() { state unused = 0\n[] }\ninstr a() { f().len }",
        0.0,
    );
    constant("instr a() { [1, 2,].sum }", 3.0);
    for expression in ["[][0]", "[].sum", "[].min", "[].at(0)"] {
        assert!(compile(&format!("instr a() {{ {expression} }}")).is_err());
        assert!(
            compile(&format!(
                "fn f() {{ state unused = 0\n{expression} }}\ninstr a() {{ f() }}"
            ))
            .is_err()
        );
    }
    assert!(
        compile("fn f() { state xs = zeros(0)\nxs[0] = 1\nxs.len }\ninstr a() { f() }").is_err()
    );
}

#[test]
fn signal_mapping_and_array_coefficients_preserve_channel_layout() {
    program(
        "config { channels: surround(5.1) }\ninstr a(freq: hz) { signal = osc(saw, freq).pan\nsignal.map(it * 0.5).to_layout(stereo).sum }",
    );
    program(
        "config { channels: surround(5.1) }\ninstr a(freq: hz) { signal = osc(saw, freq).pan\n(signal * [1, 1, 1, 1, 1, 1]).to_layout(stereo).sum }",
    );
    program(
        "config { channels: surround(5.1) }\ninstr a(freq: hz) { signal = osc(saw, freq).pan\nsignal.lpf(cutoff: [1khz, 1khz, 1khz, 1khz, 1khz, 1khz]).to_layout(stereo).sum }",
    );
}

#[test]
fn range_mapping_is_an_array_operation() {
    constant(
        "instr a() { [0, 0.5, 1].range(100hz..200hz).num.sum }",
        450.0,
    );
    constant(
        "fn f() { state xs = [0, 0.5, 1]\nxs.range(100hz..200hz).num.sum }\ninstr a() { f() }",
        450.0,
    );
    constant("instr a() { [0, 0.5, 1].range(-12db..0db).num.sum }", -18.0);
    constant(
        "fn f() { state xs = [0, 0.5, 1]\nxs.range(-12db..0db).num.sum }\ninstr a() { f() }",
        -18.0,
    );
}

#[test]
fn array_values_snapshot_buffers_and_only_state_names_are_writable() {
    constant(
        "fn f() { state buf = [1, 2]\nbuf[0] = 1\nsaved = buf\nbuf[0] = 5\nsaved.sum + buf.sum }\ninstr a() { f() }",
        10.0,
    );
    constant(
        "fn f() { state buf = [1, 2]\nbuf[0] = 1\nsaved = [buf]\nbuf[0] = 5\nsaved[0].sum + buf.sum }\ninstr a() { f() }",
        10.0,
    );
    for source in [
        "fn f() { state buf = [1, 2]\nalias = buf\nalias[0] = 5\nbuf.sum }\ninstr a() { f() }",
        "fn f(xs: [num]) { state unused = 0\nxs[0] = 5\nxs.sum }\ninstr a() { f([1, 2]) }",
    ] {
        assert!(compile(source).is_err());
    }
}

#[test]
fn mono_signals_support_array_slices_in_both_compilers() {
    constant("instr a(x: num = 2) { x[0..<1].sum }", 2.0);
    constant(
        "fn f(x) { state unused = 0\nx[0..<1].sum }\ninstr a(x: num = 2) { f(x) }",
        2.0,
    );
}

#[test]
fn carried_buffer_state_is_not_reset_by_note_start_initialization() {
    let source = |value| {
        format!(
            "fn f(x) {{ state buf = [x, x * 2]\nbuf.sum }}\ninstr a(x: num = {value}) {{ f(x) }}"
        )
    };
    let old = program(&source(2));
    let new = program(&source(5));
    let mut old_voice = Voice::new(old.instruments[0].graph.clone(), 48_000.0);
    let mut voice = Voice::new(new.instruments[0].graph.clone(), 48_000.0);
    let mut fresh = Voice::new(new.instruments[0].graph.clone(), 48_000.0);
    let mut buses = Buses::new(&[]);
    let mut output = [[0.0; BLOCK]; 1];
    old_voice.process_blocks(&mut output, BLOCK, &mut buses);
    assert_eq!(output[0], [6.0; BLOCK]);
    voice.carry_state(&mut old_voice);
    voice.process_blocks(&mut output, BLOCK, &mut buses);
    assert_eq!(output[0], [6.0; BLOCK]);
    fresh.process_blocks(&mut output, BLOCK, &mut buses);
    assert_eq!(output[0], [15.0; BLOCK]);
}

#[test]
fn unused_array_elements_do_not_add_running_dsp_nodes() {
    let reference = program("instr a(freq: hz) { osc(sine, freq) * 0.1 }");
    let with_unused = program(
        "instr a(freq: hz) { unused = osc(saw, (0..<128).map(freq * (1 + it * 0.001)))\nosc(sine, freq) * 0.1 }",
    );
    assert_eq!(
        reference.instruments[0].graph.node_count(),
        with_unused.instruments[0].graph.node_count()
    );
    let selected = program(
        "instr a(freq: hz) { bank = osc(sine, [freq, freq * 2, freq * 3])\nbank[0] * 0.1 }",
    );
    assert_eq!(
        reference.instruments[0].graph.node_count(),
        selected.instruments[0].graph.node_count()
    );
}
