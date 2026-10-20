use niminal_engine::{BLOCK, Buses, Voice};
use niminal_compose::compile;

fn render(voice: &mut Voice, frames: usize, block: usize) -> Vec<f32> {
    let mut buses = Buses::new(&[]);
    let mut result = Vec::new();
    for start in (0..frames).step_by(block) {
        let n = block.min(frames - start);
        let mut out = [[0.0; BLOCK]; 1];
        voice.process_blocks(&mut out, n, &mut buses);
        result.extend_from_slice(&out[0][..n]);
    }
    result
}

#[test]
fn live_arrays_and_buffer_references_accept_source_note_arguments() {
    let source = "const data = [0.5, 0.25]\nbuffer one = zeros(2)\nbuffer two = data\ninstr a(weights: [num] = [1, 2], source: buffer = one) { weights.sum + source[0] }\na(weights: [0.25, 0.5], source: two) for 10ms";
    let p = compile(source).unwrap();
    let plan = p.plan(&p.notes[0]).unwrap();
    let mut v = Voice::new(p.instruments[0].graph.clone(), 48000.0);
    for (index, value) in plan.params {
        v.set_param(index, value);
    }
    assert_eq!(render(&mut v, 8, 8), [1.25; 8]);
}
