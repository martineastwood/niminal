use niminal_engine::ops::{Interp, read_array, read_at};
use niminal_engine::{Array, wrapped_index};

#[test]
fn fixed_storage_can_mutate_elements_and_clones_own_their_contents() {
    let mut values: Array<f64> = vec![1.0, 2.0, 3.0].into();
    let copy = values.clone();
    values[1] = 7.0;
    assert_eq!(&*values, &[1.0, 7.0, 3.0]);
    assert_eq!(&*copy, &[1.0, 2.0, 3.0]);
    assert_eq!(values.len(), copy.len());
    assert_eq!(Array::<f64>::from(Vec::new()).len(), 0);
}

#[test]
fn wrapping_and_interpolation_are_shared_for_every_storage_accessor() {
    let values = [1.0, 2.0, 4.0, 8.0];
    for (position, index) in [
        (0.0, 0),
        (4.0, 0),
        (-1.0, 3),
        (-0.1, 3),
        (1.9, 1),
        (f64::INFINITY, 0),
        (f64::NAN, 0),
    ] {
        assert_eq!(wrapped_index(position, 4), index);
    }
    for interp in [Interp::None, Interp::Linear, Interp::Cubic] {
        for position in [-9.75, -0.25, 0.0, 1.5, 3.75, 9.1, f64::INFINITY, f64::NAN] {
            assert_eq!(
                read_array(4, position, interp, |i| values[i]),
                read_at(&values, position, interp)
            );
        }
    }
    assert_eq!(read_at(&values, 1.5, Interp::Cubic), 2.8125);
    assert_eq!(read_at(&values, -0.5, Interp::Linear), 4.5);
}
