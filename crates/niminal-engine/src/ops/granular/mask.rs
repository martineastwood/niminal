//! Grain-rate sequences. Headers hold inclusive loop bounds; mix entries have five gains.
use crate::Table;
use std::sync::Arc;

#[derive(Clone, PartialEq)]
pub(super) struct Mask {
    table: Arc<Table>,
    start: usize,
    end: usize,
    stride: usize,
}

impl Mask {
    pub fn new(table: Arc<Table>, stride: usize) -> Result<Self, String> {
        let data = table.data();
        if data.len() < 2 + stride || data.iter().any(|v| !v.is_finite()) {
            return Err(format!(
                "a grain mask needs two loop bounds followed by groups of {stride} finite values"
            ));
        }
        let (start, end) = (data[0], data[1]);
        let count = (data.len() - 2) / stride;
        if start < 0.0
            || end < start
            || start.fract() != 0.0
            || end.fract() != 0.0
            || end as usize >= count
        {
            return Err("grain mask loop bounds must be whole indices within its entries, with start <= end".into());
        }
        Ok(Self {
            table,
            start: start as usize,
            end: end as usize,
            stride,
        })
    }

    pub fn read(&self, cursor: &mut usize) -> [f32; 5] {
        if *cursor < self.start || *cursor > self.end {
            *cursor = self.start;
        }
        let mut result = [1.0; 5];
        let offset = 2 + *cursor * self.stride;
        result[..self.stride].copy_from_slice(&self.table.data()[offset..offset + self.stride]);
        *cursor += 1;
        result
    }
}
