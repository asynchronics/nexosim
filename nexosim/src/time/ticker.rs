use std::time::Duration;

use crate::time::MonotonicTime;

/// A simulation time step generator.
///
/// When simulations run in "tick-less" mode, operations like
/// [`Simulation::step`](crate::simulation::Simulation::step) will advance
/// simulation time directly to the next scheduled event.
///
/// While efficient, this approach causes the simulation to block (sleep) during
/// idle periods between events. In real-time or scaled-time scenarios, long
/// idle periods can make the simulation unresponsive to:
///
/// - *commands:* instructions like
///   [`Scheduler::halt`](crate::simulation::Scheduler::halt) are postponed
///   until the next event triggers a wake-up;
/// - *external scheduling:* the [`Scheduler`](crate::simulation::Scheduler)
///   prohibits the scheduling of new events into the current idle window;
/// - *injected events*: the processing of external events in the injector queue
///   is delayed.
///
/// A `Ticker` forces the synchronization clock to wake up at regular intervals,
/// even if no events are scheduled. This ensures that the simulation regularly
/// yields control back to the simulation controller.
///
/// *Note*: ticks are functionally similar to recurring scheduler events but are
/// significantly more efficient as they only run the executor when required.
///
/// A ticker can be attached to a simulation using
/// [`SimInit::with_clock`](crate::simulation::SimInit::with_clock).
pub trait Ticker: Send + 'static {
    /// Returns the next clock tick strictly after the provided time.
    ///
    /// It is a logical error to return a tick that is not in the future of the
    /// provided time.
    fn next_tick(&mut self, time: MonotonicTime) -> MonotonicTime;
}

/// A ticker that generates ticks at fixed intervals.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct PeriodicTicker {
    /// Tick period.
    period: Duration,
    /// The reference time for the ticks.
    ///
    /// If `None`, the reference is set upon the first call to `next_tick`.
    reference: Option<MonotonicTime>,
}

impl PeriodicTicker {
    /// Creates a ticker with a time reference determined automatically from the
    /// first call to [`Ticker::next_tick`].
    ///
    /// The first tick returned will be in the future of the time provided to
    /// the first call to [`Ticker::next_tick`] by an amount of `period`.
    ///
    /// # Panics
    ///
    /// This constructor will panic if the period is zero.
    pub fn new(period: Duration) -> Self {
        assert!(!period.is_zero(), "the tick period must be non-zero");

        Self {
            period,
            reference: None,
        }
    }

    /// Creates a ticker anchored to a specific point in time.
    ///
    /// Ticks are calculated as `origin + k * period`, where the origin can be
    /// freely set in the past or in the future. This ensures that ticks always
    /// fall on the boundaries defined by the origin, regardless of when the
    /// simulation starts or when `next_tick` is called.
    ///
    /// # Panics
    ///
    /// This constructor will panic if the period is null.
    ///
    /// # Example
    ///
    /// To tick at every full second:
    ///
    /// ```
    /// # use std::time::Duration;
    /// # use nexosim::time::MonotonicTime;
    /// # use nexosim::time::PeriodicTicker;
    /// PeriodicTicker::anchored_at(Duration::from_secs(1), MonotonicTime::EPOCH);
    /// ```
    pub fn anchored_at(period: Duration, reference: MonotonicTime) -> Self {
        assert!(!period.is_zero(), "the tick period must be non-zero");

        Self {
            period,
            reference: Some(reference),
        }
    }
}

impl Ticker for PeriodicTicker {
    fn next_tick(&mut self, time: MonotonicTime) -> MonotonicTime {
        // Since this method is typically called at monotonically increasing
        // times and at intervals less than `period`, the reference time is
        // reset at each call to match the new tick and allow the fast-path
        // computation to be used at the next call.

        let reference = *self.reference.get_or_insert(time);

        let tick = if time >= reference {
            // The time is in the future of the reference.
            let elapsed = time.duration_since(reference);

            if elapsed < self.period {
                // Fast path.
                reference + self.period
            } else {
                let period_ns = self.period.as_nanos();
                let remainder_ns = elapsed.as_nanos() % period_ns;
                let time_to_next_ns = period_ns - remainder_ns;

                time + duration_from_nanos_u128(time_to_next_ns)
            }
        } else {
            // The time is in the strict past of the reference.
            let period_ns = self.period.as_nanos();
            let gap = reference.duration_since(time);
            let dist_to_next_ns = gap.as_nanos() % period_ns;

            if dist_to_next_ns == 0 {
                time + self.period
            } else {
                time + duration_from_nanos_u128(dist_to_next_ns)
            }
        };

        self.reference = Some(tick);

        tick
    }
}

// `Duration::from_nanos_u128` was stabilized in Rust 1.93 and can be used
// instead once we bump the MSRV.
fn duration_from_nanos_u128(nanos: u128) -> Duration {
    const NANOS_PER_SEC: u128 = 1_000_000_000;

    let secs = u64::try_from(nanos / NANOS_PER_SEC).unwrap();
    let subsec_nanos = (nanos % NANOS_PER_SEC) as u32;

    Duration::new(secs, subsec_nanos)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticker_basic_stepping() {
        const PERIOD: Duration = Duration::from_secs(1);
        const START: MonotonicTime = MonotonicTime::new(123, 456_789_012).unwrap();

        let mut ticker = PeriodicTicker::new(PERIOD);

        assert_eq!(ticker.next_tick(START), START + PERIOD);

        assert_eq!(
            ticker.next_tick(MonotonicTime::new(0, 500_000_000).unwrap()),
            MonotonicTime::new(1, 456_789_012).unwrap()
        );

        assert_eq!(
            ticker.next_tick(MonotonicTime::new(1, 456_789_012).unwrap()),
            MonotonicTime::new(2, 456_789_012).unwrap()
        );
    }

    #[test]
    fn ticker_sub_tick_increment() {
        const PERIOD: Duration = Duration::from_secs(10);
        const START: MonotonicTime = MonotonicTime::new(123, 456_789_012).unwrap();

        let mut ticker = PeriodicTicker::anchored_at(PERIOD, START);

        let next = ticker.next_tick(START + PERIOD / 2);

        assert_eq!(next, START + PERIOD);
    }

    #[test]
    fn ticker_exact_tick_increment() {
        const PERIOD: Duration = Duration::from_secs(10);
        const START: MonotonicTime = MonotonicTime::new(123, 456_789_012).unwrap();

        let mut ticker = PeriodicTicker::anchored_at(PERIOD, START);

        let next = ticker.next_tick(START + PERIOD);

        assert_eq!(next, START + PERIOD * 2);
    }

    #[test]
    fn ticker_past_reference() {
        const PERIOD: Duration = Duration::from_secs(10);
        const FUTURE_ANCHOR: MonotonicTime = MonotonicTime::new(100, 0).unwrap();

        let mut ticker = PeriodicTicker::anchored_at(PERIOD, FUTURE_ANCHOR);

        assert_eq!(
            ticker.next_tick(MonotonicTime::new(95, 0).unwrap()),
            MonotonicTime::new(100, 0).unwrap()
        );

        assert_eq!(
            ticker.next_tick(MonotonicTime::new(85, 0).unwrap()),
            MonotonicTime::new(90, 0).unwrap()
        );

        assert_eq!(
            ticker.next_tick(MonotonicTime::new(80, 0).unwrap()),
            MonotonicTime::new(90, 0).unwrap()
        );
    }

    #[test]
    fn ticker_very_large_step() {
        const PERIOD: Duration = Duration::from_secs(2);
        const START: MonotonicTime = MonotonicTime::new(123, 456_789_012).unwrap();

        let mut ticker = PeriodicTicker::new(PERIOD);

        // Initialize.
        ticker.next_tick(START);

        // Jump forward 1001 seconds (500.5 periods).
        assert_eq!(
            ticker.next_tick(START + Duration::new(1001, 0)),
            START + Duration::new(1002, 0)
        );

        // Jump to 1002 (Exact boundary, fast path).
        assert_eq!(
            ticker.next_tick(START + Duration::new(1002, 0)),
            START + Duration::new(1004, 0)
        );

        // Jump to 1200 (Exact boundary, slow path).
        assert_eq!(
            ticker.next_tick(START + Duration::new(1200, 0)),
            START + Duration::new(1202, 0)
        );
    }
}
