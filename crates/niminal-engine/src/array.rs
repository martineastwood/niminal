//! Fixed-length storage shared by DSP arrays and signal channel bundles.
//!
//! Length is chosen when the array is built. DSP processing mutates elements
//! in place; the storage cannot grow.

use std::ops::{Deref, DerefMut};

#[derive(Clone, Debug, PartialEq)]
pub struct Array<T>(Box<[T]>);

impl<T> Array<T> {
    pub fn into_vec(self) -> Vec<T> {
        self.0.into_vec()
    }
}

impl<T> From<Vec<T>> for Array<T> {
    fn from(values: Vec<T>) -> Self {
        Self(values.into_boxed_slice())
    }
}

impl<T> FromIterator<T> for Array<T> {
    fn from_iter<I: IntoIterator<Item = T>>(values: I) -> Self {
        Vec::from_iter(values).into()
    }
}

impl<T> Deref for Array<T> {
    type Target = [T];
    fn deref(&self) -> &[T] {
        &self.0
    }
}

impl<T> DerefMut for Array<T> {
    fn deref_mut(&mut self) -> &mut [T] {
        &mut self.0
    }
}

impl<T> IntoIterator for Array<T> {
    type Item = T;
    type IntoIter = std::vec::IntoIter<T>;
    fn into_iter(self) -> Self::IntoIter {
        self.into_vec().into_iter()
    }
}

impl<'a, T> IntoIterator for &'a Array<T> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

/// DSP indexing floors fractional indices and wraps in both directions.
/// Non-finite positions select element zero.
pub fn wrapped_index(index: f64, len: usize) -> usize {
    assert!(len > 0, "cannot index an empty array");
    if !index.is_finite() {
        return 0;
    }
    index.floor().rem_euclid(len as f64) as usize % len
}
