//! Formal verification support for the simulation ticker.
//!
//! This module models the nanosecond-level step computation of
//! [`PeriodicTicker::next_tick`](crate::time::ticker::PeriodicTicker::next_tick)
//! and proves that each computed tick is strictly in the future of the
//! provided time, provided the tick period is non-zero.

#![allow(unreachable_pub)]

use vstd::prelude::*;

verus! {

/// Specification of `PeriodicTicker::next_tick` in terms of integer
/// nanoseconds.
pub open spec fn next_tick_ns(time: u128, reference: u128, period: u128) -> int {
    let t = time as int;
    let r = reference as int;
    let p = period as int;
    if t >= r {
        let e = t - r;
        if e < p {
            r + p
        } else {
            let rem = e % p;
            t + (p - rem)
        }
    } else {
        let g = r - t;
        let d = g % p;
        if d == 0 {
            t + p
        } else {
            t + d
        }
    }
}

/// A non-zero tick period guarantees that the next tick is strictly in the
/// future of the provided time.
pub proof fn lemma_next_tick_is_future(time: u128, reference: u128, period: u128)
    requires
        period > 0,
    ensures
        next_tick_ns(time, reference, period) > (time as int),
{
    broadcast use vstd::arithmetic::div_mod::lemma_mod_bound;

    let t = time as int;
    let r = reference as int;
    let p = period as int;

    if t >= r {
        let e = t - r;
        assert(e >= 0);
        if e < p {
            assert(next_tick_ns(time, reference, period) == r + p);
            assert(r + p > t);
        } else {
            let rem = e % p;
            assert(0 <= rem < p);
            assert(next_tick_ns(time, reference, period) == t + (p - rem));
            assert(p - rem > 0);
            assert(t + (p - rem) > t);
        }
    } else {
        let g = r - t;
        assert(g > 0);
        let d = g % p;
        assert(0 <= d < p);
        if d == 0 {
            assert(next_tick_ns(time, reference, period) == t + p);
            assert(t + p > t);
        } else {
            assert(next_tick_ns(time, reference, period) == t + d);
            assert(d > 0);
            assert(t + d > t);
        }
    }
}

} // verus!
