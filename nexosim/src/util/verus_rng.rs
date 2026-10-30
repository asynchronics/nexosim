//! Formal verification support for the pseudo-random number generator.
//!
//! This module contains a model of the fast multiply-shift reduction used by
//! [`Rng::rand_bounded`](crate::util::rng::Rng::rand_bounded) and a proof that
//! the reduction always yields a value strictly smaller than the requested
//! upper bound.

#![allow(unreachable_pub)]

use vstd::prelude::*;

verus! {

/// 2^64, i.e. the modulus corresponding to a 64-bit unsigned integer.
pub open spec fn u64_modulus() -> nat {
    18446744073709551616
}

/// Specification of the multiply-shift reduction used by `Rng::rand_bounded`.
pub closed spec fn mul_shift_spec(value: u64, upper_bound: u64) -> u64 {
    (((value as nat) * (upper_bound as nat)) / u64_modulus()) as u64
}

/// The multiply-shift reduction is always strictly smaller than the bound.
pub proof fn lemma_mul_shift_bound(value: u64, upper_bound: u64)
    requires
        upper_bound > 0,
    ensures
        mul_shift_spec(value, upper_bound) < upper_bound,
{
    broadcast use vstd::arithmetic::mul::lemma_mul_strict_inequality;
    broadcast use vstd::arithmetic::div_mod::lemma_multiply_divide_lt;

    assert((value as nat) < u64_modulus());
    assert(((value as nat) * (upper_bound as nat)) < u64_modulus() * (upper_bound as nat));
    assert(((value as nat) * (upper_bound as nat)) / u64_modulus() < (upper_bound as nat));
}

} // verus!
