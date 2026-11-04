//! Smoke tests for the Verus toolchain integration.

#![allow(dead_code)]

use vstd::prelude::*;

verus! {

/// A trivial proof used to confirm that the Verus toolchain is wired correctly.
pub(crate) proof fn lemma_addition_is_commutative(a: int, b: int)
    ensures a + b == b + a
{
}

/// A tiny executable function with a verified postcondition.
pub(crate) fn checked_add(x: u32, y: u32) -> (sum: u32)
    requires
        x < 100,
        y < 100,
    ensures
        sum == x + y,
{
    x + y
}

/// A tiny async executable function with a verified postcondition.
pub(crate) async fn checked_async_add(x: u64, y: u64) -> (sum: u64)
    requires
        x < u64::MAX,
        y < u64::MAX,
        x + y < u64::MAX,
    ensures
        sum == x + y,
{
    x + y
}

} // verus!
