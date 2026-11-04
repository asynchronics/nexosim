//! Formal verification support for path segment ranges.
//!
//! This module models the range normalization performed by
//! [`Path::subpath`](crate::path::Path::subpath) and proves that any
//! normalized range is valid (`start <= end <= len`).

#![allow(unreachable_pub)]
#![allow(dead_code)]

use vstd::prelude::*;

verus! {

/// Model of a range bound for `Path::subpath`.
pub enum RangeBound {
    Included(usize),
    Excluded(usize),
    Unbounded,
}

/// Model of `Path::subpath` range normalization.
pub open spec fn normalize_range(len: usize, start: RangeBound, end: RangeBound) -> Option<(usize, usize)> {
    let s = match start {
        RangeBound::Included(v) => v as int,
        RangeBound::Excluded(v) => (v as int) + 1,
        RangeBound::Unbounded => 0,
    };
    let e = match end {
        RangeBound::Included(v) => (v as int) + 1,
        RangeBound::Excluded(v) => v as int,
        RangeBound::Unbounded => len as int,
    };
    if s <= e && e <= len as int {
        Some((s as usize, e as usize))
    } else {
        None
    }
}

/// Any normalized range returned by the model is valid.
pub proof fn lemma_normalize_range_valid(len: usize, start: RangeBound, end: RangeBound)
    ensures
        match normalize_range(len, start, end) {
            None => true,
            Some((s, e)) => s <= e && e <= len,
        }
{
}

} // verus!
